//! 用户态 ↔ 内核态边界（v0.6 建立，v0.9 第三步扩展为「完整用户帧 + 可恢复运行」）
//!
//! 本模块提供用户态路径的全部汇编入口：
//!
//! 1. [`syscall_entry`]：`IA32_LSTAR` 指向的 `syscall` 指令入口。`syscall` 只保存
//!    RIP→RCX、RFLAGS→R11，**不切换栈**，因此入口第一件事是把用户 RSP 存起来、
//!    换到当前进程的系统调用栈，再把**完整寄存器帧**压栈后调用 Rust 分发函数；
//! 2. [`run_user_frame`]：把一个保存好的 [`UserFrame`] 恢复成 ring3 现场（`iretq`），
//!    同时把「回到内核的位置」写进 [`RunCtx`]（这是 `fork`/`wait4` 的关键：
//!    内核可以停在某个进程的 `sys_wait` 里，去跑另一个进程，再回到原地）；
//! 3. [`enter_user_frame`]：只恢复现场、**不**记录返回点 —— `execve` 用（它换了
//!    映像但返回点仍是父进程的 `wait4`，不能被覆盖）；
//! 4. [`return_to_suspended`]：从用户态回到暂停在 [`RunCtx`] 上的内核调用者
//!    （`sys_exit` 与 ring3 异常终止路径）。
//!
//! 约定（与 `syscall/mod.rs` 一致）：`rax` = syscall 号，`rdi/rsi/rdx/r10/r8/r9` =
//! 第 1..6 参数（`r9` 是 v1.0 起的扩展槽），返回值在 `rax`。
//!
//! **寄存器保存规则（与 Linux syscall ABI 一致，用户态可以依赖）**：一次 `syscall`
//! 往返后，除 `rax`（返回值）、`rcx`（被指令写成用户 RIP）、`r11`（用户 RFLAGS）
//! 之外，全部通用寄存器保持进入前的值。这条规则不是“顺手为之”：Rust 编译器在用户态
//! 会把活跃值放在任意 caller-saved 寄存器（实测踩中 `r9`），不恢复就会在 `sysretq`
//! 之后写到一个垃圾地址（v0.7 实测 `cr2=0x105` 的 ring3 #PF）。
//!
//! ## 为什么连 callee-saved 寄存器也要压栈（v0.9 第三步）
//!
//! 单进程往返时 `rbx/rbp/r12..r15` 由 C 调用约定保证不变，不必保存。但 `fork`
//! 要**克隆父进程停在 `syscall` 里的现场**：子进程恢复后要继续执行 `fork()` 之后的
//! 代码，而那段代码可能把活跃值放在这些寄存器里。因此入口把 15 个通用寄存器
//! **全部**压栈，形成 [`UserFrame`]；`fork` 直接复制这一帧并把 `rax` 改成 0。
//!
//! ## 栈模型
//!
//! - 用户态执行期间的中断/异常由 CPU 依据 `TSS.RSP0` 切到**陷入栈**；
//! - `syscall` 指令切到**系统调用栈**（`CER_SYSCALL_STACK_TOP`）。
//!
//! 两条栈都是**每进程**的（`kernel/src/process/process.rs`）：父进程停在 `sys_wait`
//! 里时，子进程的 syscall 必须落在另一条栈上，否则会覆盖父进程的帧 —— 这正是
//! v0.9 第二步留下的「进程模型」缺口。

use core::arch::global_asm;

/// 一次 `syscall` 在系统调用栈上保存的**完整用户寄存器帧**。
///
/// 布局与 `syscall_entry` 的 push 顺序严格对应（栈顶 = `user_rsp`）。任何一侧
/// 改动都必须同步另一侧 —— 偏移在汇编里是硬编码的。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UserFrame {
    /// 用户 RSP（入口第一件事存进 `CER_USER_RSP`，最后才压栈 → 位于帧底）
    pub user_rsp: u64, // +0
    pub r15: u64,      // +8
    pub r14: u64,      // +16
    pub r13: u64,      // +24
    pub r12: u64,      // +32
    pub rbp: u64,      // +40
    pub rbx: u64,      // +48
    pub rdi: u64,      // +56  ← &args[0]
    pub rsi: u64,      // +64
    pub rdx: u64,      // +72
    pub r10: u64,      // +80
    pub r8: u64,       // +88
    pub r9: u64,       // +96
    pub rax: u64,      // +104 ← syscall 号 / 返回值
    pub rflags: u64,   // +112
    pub rip: u64,      // +120
}

/// 用户帧字节数（汇编按这个值计算帧位置）。
pub const USER_FRAME_SIZE: u64 = core::mem::size_of::<UserFrame>() as u64;
const _: () = assert!(USER_FRAME_SIZE == 128);

impl UserFrame {
    /// 新建「从 `entry` 开始、用户栈顶 `rsp`」的初始帧。
    ///
    /// `argc`/`argv`/`envc`/`envp` 按本内核的入口 ABI 放在寄存器里：
    ///
    /// ```text
    /// rdi = argc   rsi = argv   rdx = envc   r10 = envp
    /// ```
    ///
    /// `libc` 的 `_start` 把 `r10`（`envp`）搬到 `rcx` 后调用 `cer_start`，于是四个值
    /// 正好落在 C 调用约定的前四个整数寄存器（`rdi, rsi, rdx, rcx`）上。选寄存器而不是
    /// 栈，是因为总入口是我们自己的 `_start`（Linux 把 `argv`/`envp` 放在用户栈顶，
    /// 两种对用户程序都透明）。
    ///
    /// **为什么 `envp` 用 `r10` 而不是 `rcx`**：`rcx` 在 `syscall`/`sysret` 往返里被
    /// 硬件当作用户 RIP，帧恢复时它载入的是入口地址（见 `UserFrame` 布局）——拿它当
    /// 参数会和返回地址打架。`r10` 是普通易失寄存器，恢复后就是我们要的值。
    pub const fn new_user(
        entry: u64,
        rsp: u64,
        argc: u64,
        argv: u64,
        envc: u64,
        envp: u64,
    ) -> Self {
        Self {
            user_rsp: rsp,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            rbp: 0,
            rbx: 0,
            rdi: argc,
            rsi: argv,
            rdx: envc,
            r10: envp,
            r8: 0,
            r9: 0,
            rax: 0,
            rflags: 0x202, // bit1 固定 1，bit9 = IF
            rip: entry,
        }
    }
}

/// 进程表下标里的「无当前进程」哨兵（与 `process::NO_PROCESS` 一致）。
pub const NO_PROCESS: u64 = u64::MAX;

/// 运行上下文：从用户态回到内核所需的最小状态。
///
/// 每个进程在**被运行前**填写「谁在等我」的那一半（`cr3`…`current`），
/// `krsp` 由 [`run_user_frame`] 的汇编在进入用户态前写入。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RunCtx {
    /// 内核栈指针（**必须位于偏移 0**：`return_to_suspended` 直接 `mov rsp,[rdi]`）
    pub krsp: u64,
    /// 恢复时要写回 CR3 的页表根
    pub cr3: u64,
    /// 恢复时要写回 `TSS.RSP0` 的陷入栈顶
    pub rsp0: u64,
    /// 恢复时要写回的 `CER_SYSCALL_STACK_TOP`
    pub syscall_top: u64,
    /// 恢复时要写回的 `CER_USER_SPACE`
    pub space: u64,
    /// 恢复时要写回的当前进程下标（[`NO_PROCESS`] = 回到内核启动上下文）
    pub current: u64,
}

// 以下静态量供汇编直接以 `[rip + SYMBOL]` 访问，因此必须 `#[no_mangle]`。
/// `syscall` 进入时保存的用户 RSP
#[no_mangle]
pub static mut CER_USER_RSP: u64 = 0;
/// 当前用户地址空间指针（作为 `sys_enter` 的 `space` 参数传入）
#[no_mangle]
pub static mut CER_USER_SPACE: u64 = 0;
/// 当前进程的系统调用栈顶（每进程不同，切换进程时由 Rust 侧改写）
#[no_mangle]
pub static mut CER_SYSCALL_STACK_TOP: u64 = 0;

global_asm!(
    // ---------------------------------------------------------------------
    // syscall 入口（IA32_LSTAR）
    // ---------------------------------------------------------------------
    // 进入时：rcx = 用户 RIP，r11 = 用户 RFLAGS，rsp 仍是用户栈。
    //
    // 压栈顺序（先压的在**高地址**）：
    //   rcx r11 rax r9 r8 r10 rdx rsi rdi rbx rbp r12 r13 r14 r15 用户rsp
    // 因此帧基址 F = CER_SYSCALL_STACK_TOP - 128，且
    //   F+0 用户rsp  F+8 r15  … F+56 rdi  … F+104 rax  F+112 r11  F+120 rcx
    // —— 与 Rust 侧 `UserFrame` 的字段顺序逐项对应。
    ".global syscall_entry",
    "syscall_entry:",
    "mov [rip + CER_USER_RSP], rsp",
    "mov rsp, [rip + CER_SYSCALL_STACK_TOP]",
    "push rcx",
    "push r11",
    "push rax",
    "push r9",
    "push r8",
    "push r10",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "push qword ptr [rip + CER_USER_RSP]",
    // 分发参数：rdi = 号（F+104），rsi = &args[0] = &rdi（F+56），rdx = space
    "lea rsi, [rsp + 56]",
    "mov rdi, [rsp + 104]",
    "mov rdx, [rip + CER_USER_SPACE]",
    // 帧基址 16 字节对齐（栈顶对齐 + 128 是 16 的倍数）→ `call` 前满足 SysV
    "call syscall_dispatch",
    // 返回值写回帧里的 rax 槽，然后逐项恢复（顺序与压栈严格相反）
    "mov rsp, [rip + CER_SYSCALL_STACK_TOP]",
    "sub rsp, 128",
    "mov [rsp + 104], rax",
    "pop qword ptr [rip + CER_USER_RSP]",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop r10",
    "pop r8",
    "pop r9",
    "pop rax",
    "pop r11",
    "pop rcx",
    "mov rsp, [rip + CER_USER_RSP]",
    "sysretq",
    "\n",

    // ---------------------------------------------------------------------
    // 恢复一个保存好的用户帧并进入 ring3，同时记录返回点
    // ---------------------------------------------------------------------
    // rdi = *const UserFrame，rsi = *mut RunCtx
    //
    // `cer_run_resume` 只在被 `return_to_suspended` 拉回时经过：此时栈指针
    // 已经是 `ctx->krsp`，`ret` 正好返回到 `run_user_frame` 的调用者。
    ".global run_user_frame",
    "run_user_frame:",
    "mov [rsi], rsp",
    ".global cer_run_resume",
    "cer_run_resume:",
    // ---- 落空到 enter_user_frame（同一段恢复代码）----
    ".global enter_user_frame",
    "enter_user_frame:",
    // 构造 iretq 帧：SS / RSP / RFLAGS / CS / RIP
    "mov rax, 0x1b",
    "push rax",
    "push qword ptr [rdi + 0]",
    "push qword ptr [rdi + 112]",
    "mov rax, 0x23",
    "push rax",
    "push qword ptr [rdi + 120]",
    // 通用寄存器：rdi 最后加载（它是帧基址）
    "mov r15, [rdi + 8]",
    "mov r14, [rdi + 16]",
    "mov r13, [rdi + 24]",
    "mov r12, [rdi + 32]",
    "mov rbp, [rdi + 40]",
    "mov rbx, [rdi + 48]",
    "mov rsi, [rdi + 64]",
    "mov rdx, [rdi + 72]",
    "mov r10, [rdi + 80]",
    "mov r8, [rdi + 88]",
    "mov r9, [rdi + 96]",
    "mov rax, [rdi + 104]",
    "mov r11, [rdi + 112]",
    "mov rcx, [rdi + 120]",
    "mov rdi, [rdi + 56]",
    "iretq",
    "\n",

    // ---------------------------------------------------------------------
    // 回到暂停在 RunCtx 上的内核调用者（不返回用户态）
    // ---------------------------------------------------------------------
    // rdi = *const RunCtx
    ".global return_to_suspended",
    "return_to_suspended:",
    "mov rsp, [rdi]",
    "ret",
    "\n",
);

// 汇编里 `enter_user_frame` 用的是立即数 0x1b / 0x23（用户数据段/代码段 | RPL3）。
// 这里把两个字面量与 GDT 常量绑定，任何一侧改动都会在编译期被发现，
// 而不是等到 ring3 返回时以 #GP 的形式炸在运行期。
const _: () = assert!(crate::arch::x86_64::gdt::USER_DATA_RPL3 == 0x1b);
const _: () = assert!(crate::arch::x86_64::gdt::USER_CODE_RPL3 == 0x23);

extern "C" {
    /// `syscall` 指令入口（地址写入 IA32_LSTAR）。
    fn syscall_entry();
    /// 恢复用户帧进入 ring3，并把返回点写进 `ctx`；被 [`return_to_suspended`] 拉回前不返回。
    fn run_user_frame(frame: *const UserFrame, ctx: *mut RunCtx);
    /// 恢复用户帧进入 ring3（不记录返回点）；不返回。
    fn enter_user_frame(frame: *const UserFrame) -> !;
    /// 回到暂停在 `ctx` 上的内核调用者；不返回。
    ///
    /// 符号名与 Rust 包装函数同名，因此这里用不同的绑定名 + `link_name`。
    #[link_name = "return_to_suspended"]
    fn return_to_suspended_asm(ctx: *const RunCtx) -> !;
}

/// `syscall` 指令入口地址（写入 IA32_LSTAR）。
pub fn syscall_entry_addr() -> u64 {
    syscall_entry as *const () as u64
}

/// 当前 `syscall` 保存在系统调用栈上的用户帧指针。
///
/// 只在**当前进程的 syscall** 里有效（分发函数与 handler 都在这个窗口内）。
pub fn current_frame() -> *mut UserFrame {
    let top = unsafe { CER_SYSCALL_STACK_TOP };
    (top - USER_FRAME_SIZE) as *mut UserFrame
}

/// 当前进程的系统调用栈顶。
pub fn syscall_stack_top() -> u64 {
    unsafe { CER_SYSCALL_STACK_TOP }
}

/// 切换到进程 `top` 的系统调用栈（进入/恢复用户态前调用，返回内核时恢复）。
///
/// # Safety
/// `top` 必须是该进程独占的系统调用栈顶端。
pub unsafe fn set_syscall_stack_top(top: u64) {
    unsafe {
        CER_SYSCALL_STACK_TOP = top;
    }
}

/// 记录当前用户地址空间指针，供 `syscall` 分发时作为 `space` 参数。
///
/// # Safety
/// `space` 必须指向在用户态运行期间持续有效的 `UserSpace`（`0` = 无）。
pub unsafe fn set_user_space(space: u64) {
    unsafe {
        CER_USER_SPACE = space;
    }
}

/// 当前用户地址空间指针（`0` = 内核上下文）。
pub fn user_space_ptr() -> u64 {
    unsafe { CER_USER_SPACE }
}

/// 恢复 `frame` 描述的用户现场并进入 ring3；返回点写进 `ctx`。
///
/// # Safety
/// 调用前必须已完成：`write_cr3(该进程页表根)`、`TSS.RSP0 = 该进程陷入栈顶`、
/// `set_syscall_stack_top(该进程系统调用栈顶)`、`set_user_space(该进程地址空间)`，
/// 且 `frame`/`ctx` 在调用期间持续有效（`frame` 的页在用户地址空间中已映射）。
pub unsafe fn run(frame: *const UserFrame, ctx: *mut RunCtx) {
    unsafe { run_user_frame(frame, ctx) }
}

/// 恢复 `frame` 描述的用户现场并进入 ring3，**不**记录返回点。
///
/// `execve` 用它：它换了进程映像，但返回点应保持为父进程 `wait4` 的位置
/// （在 [`RunCtx`] 里），不能被本次进入覆盖。
///
/// # Safety
/// 同 [`run`]，但调用方必须已经为该进程准备好可用的 [`RunCtx`]。
pub unsafe fn enter(frame: *const UserFrame) -> ! {
    unsafe { enter_user_frame(frame) }
}

/// 回到暂停在 `ctx` 上的内核调用者（`sys_exit` / ring3 异常终止路径）。
///
/// # Safety
/// 调用前必须已把 `ctx` 里的 `cr3`/`rsp0`/`syscall_top`/`space`/`current`
/// 恢复完毕（见 `syscall::process::resume_kernel_side`）。
pub unsafe fn return_to_suspended(ctx: *const RunCtx) -> ! {
    unsafe { return_to_suspended_asm(ctx) }
}
