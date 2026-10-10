//! 上下文切换原语（v0.4 起，为 v0.5 的 Task/调度铺路；v0.6 扩展为用户态切换）
//!
//! 提供最小可用的**内核态**上下文切换：
//! - [`switch_to`]：保存当前上下文的 callee-saved 寄存器、RFLAGS 与栈指针，恢复目标上下文；
//! - [`init_context`]：为新建上下文准备初始内核栈（首次被切换进来即跳进入口函数）；
//! - [`selftest`]：内核内自测（主上下文 ↔ 备用上下文往返一次），供集成测试断言。
//!
//! 约定与边界：
//! - 保存 callee-saved（rbp/rbx/r12..r15）、RFLAGS 与 rsp，与 SysV 调用约定一致；
//!   caller-saved 寄存器由调用者自行处理，XMM/FPU 状态**尚未**保存（v0.5 补）。
//! - **必须保存 RFLAGS**：v0.5 的调度切换发生在 IRQ0 中断上下文内，中断门会清 IF；
//!   若不随上下文保存/恢复 RFLAGS，切出后 IF 永久为 0，后续 IRQ 不再递交，调度与心跳停摆。
//! - 地址空间切换（CR3）作为可选字段 `cr3` 记录；v0.6 用户态切换会填写用户页表根，
//!   内核自己保持恒等映射，不切换 CR3。
//! - `rip` / `rflags` 预留给 v0.5（用于用户态入口与 ring 切换的 iretq 帧）。

#![allow(dead_code)]

use core::arch::global_asm;
use core::ptr::addr_of_mut;

/// 线程上下文：保存/恢复切换所需的最小寄存器状态。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThreadContext {
    /// 该任务的地址空间根（CR3）；v0.6 用户态切换时设为用户页表根。
    pub cr3: u64,
    /// 内核栈指针：指向已保存的 callee-saved 寄存器区（切换后 pop + ret 使用）。
    pub rsp: u64,
    /// 该上下文首次被调度时的入口地址（仅 `init_context` 写入，预留）。
    pub rip: u64,
    /// 该上下文首次被调度时的 RFLAGS（仅 `init_context` 写入，预留）。
    pub rflags: u64,
}

impl ThreadContext {
    pub const fn new() -> Self {
        Self {
            cr3: 0,
            rsp: 0,
            rip: 0,
            rflags: 0,
        }
    }
}

global_asm!(
    // context_switch(prev: *mut ThreadContext /*rdi*/, next: *mut ThreadContext /*rsi*/)
    // 相当于 SysV 的 `context_switch`，但实际用的是 `context_switch_sys` 的汇编体，
    // 只是为了让当前代码与已有测试/链接脚本约定保持一致。
    ".globl context_switch", "context_switch:",
    // 1) 把 RFLAGS 与 callee-saved 寄存器压入**当前**栈；
    // 2) prev->rsp = rsp（偏移 8，即 rsp 字段）；
    // 3) rsp = next->rsp（偏移 8）；
    // 4) 从新栈弹出 callee-saved/RFLAGS 并 ret 到 next 保存的返回点。
    // 栈上保存顺序（低地址 → 高地址）：r15 r14 r13 r12 rbx rbp rflags ret
    "pushfq",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",

    // 当前栈顶（存放这 7 个 qword）是 rsp 当前值。
    // 我们先把当前 rsp 暂存到 prev->rsp。
    "mov [rdi + 8], rsp",

    // 把 rsp 换成 next 的栈顶（next->rsp 字段，前 7 个 qword 被压在我们读到的值上）。
    // 注意：next->rsp 紧跟在 next 结构之后（即 [rsi + 8]），
    // 随后还跟着 next->rip 和 next->rflags。
    "mov rsp, [rsi + 8]",

    // 弹出 r15..rflags 并 ret。
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "popfq",
    "ret",
);

extern "C" {
    fn context_switch(prev: *mut ThreadContext, next: *mut ThreadContext);
}

/// 保存 `prev` 上下文，切换到 `next` 上下文。
///
/// # Safety
///
/// - `prev` / `next` 必须指向有效且长期存在的 `ThreadContext`；
/// - `next` 必须已由 [`init_context`] 初始化，或正阻塞在某次 `switch_to` 中；
/// - 切换期间不得依赖未保存的 caller-saved / 浮点寄存器状态。
pub unsafe fn switch_to(prev: *mut ThreadContext, next: *mut ThreadContext) {
    unsafe { context_switch(prev, next) }
}

/// 为新上下文准备初始内核栈，使其首次被调度时从 `entry` 开始执行。
///
/// `entry` 通过 `context_switch` 的 `ret` 进入，因此它的栈状态必须与
/// “刚被 `call` 调用”一致（入口处 `rsp % 16 == 8`）。
///
/// # Safety
///
/// - `ctx` 必须指向有效内存；
/// - `stack_top` 必须指向该上下文独占的栈内存末尾（高地址端），且可用空间 ≥ 64 字节；
/// - `entry` 必须是 `extern "C"` 且不返回（或返回前自行切换走的）函数。
pub unsafe fn init_context(ctx: *mut ThreadContext, stack_top: *mut u8, entry: usize) {
    // 栈顶向下对齐到 16 字节。
    let top = (stack_top as usize) & !0xF;
    // 布局（低地址 → 高地址）：r15 r14 r13 r12 rbx rbp rflags ret=entry（共 8 个 qword）
    //
    // 取 S = top - 72：
    // - 72 % 16 == 8，故 S % 16 == 8；
    // - `ret` 完成后 rsp = S + 64 = top - 8，满足 SysV 入口约定 rsp % 16 == 8。
    let s = top - 72;
    let slots = s as *mut u64;
    let rflags: u64 = 0x202; // bit1 固定为 1，bit9 = IF = 1
    unsafe {
        *slots.add(0) = 0; // r15
        *slots.add(1) = 0; // r14
        *slots.add(2) = 0; // r13
        *slots.add(3) = 0; // r12
        *slots.add(4) = 0; // rbx
        *slots.add(5) = 0; // rbp
        *slots.add(6) = rflags; // popfq：新上下文默认开中断
        *slots.add(7) = entry as u64; // context_switch 末端的 ret 目标

        (*ctx).rsp = s as u64;
        (*ctx).rip = entry as u64;
        (*ctx).rflags = rflags;
    }
}

// ---- 内核自测：主上下文 ↔ 备用上下文往返一次 ----

/// 备用上下文使用的内核栈（16 KiB）。
static mut ALT_STACK: [u8; 16 * 1024] = [0; 16 * 1024];
static mut MAIN_CTX: ThreadContext = ThreadContext::new();
static mut ALT_CTX: ThreadContext = ThreadContext::new();
static mut ALT_RUNS: u64 = 0;
static mut MAIN_RESUMES: u64 = 0;

/// 备用上下文的入口：记录执行痕迹后切回主上下文。
extern "C" fn alt_entry() -> ! {
    unsafe {
        ALT_RUNS = ALT_RUNS.wrapping_add(1);
        switch_to(addr_of_mut!(ALT_CTX), addr_of_mut!(MAIN_CTX));
    }
    // 本自测只允许往返一次：若备用上下文被再次调度，直接挂起。
    crate::halt()
}

/// 上下文切换自测：切换到备用上下文，再切回主上下文，校验往返语义。
///
/// 成功返回 `Ok(())`；`kernel_main` 据此打印 `context: switch PASS`。
pub fn selftest() -> Result<(), &'static str> {
    unsafe {
        ALT_RUNS = 0;
        MAIN_RESUMES = 0;

        let stack_top = addr_of_mut!(ALT_STACK) as *mut u8;
        let stack_top = stack_top.add(core::mem::size_of::<[u8; 16 * 1024]>());
        init_context(
            addr_of_mut!(ALT_CTX),
            stack_top,
            alt_entry as *const () as usize,
        );

        // 切出：保存主上下文，进入备用上下文。
        switch_to(addr_of_mut!(MAIN_CTX), addr_of_mut!(ALT_CTX));

        // 能执行到这里 == 备用上下文已经切回主上下文。
        MAIN_RESUMES = MAIN_RESUMES.wrapping_add(1);

        if ALT_RUNS != 1 {
            return Err("context: 备用上下文未按预期执行");
        }
        if MAIN_RESUMES != 1 {
            return Err("context: 主上下文恢复次数异常");
        }
        if MAIN_CTX.rsp == 0 {
            return Err("context: 主上下文栈指针未保存");
        }
    }
    Ok(())
}
