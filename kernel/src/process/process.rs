//! 进程对象（PCB）与进程表（v0.9 第三步）
//!
//! v0.5 的 `Process` 只是一行登记（pid + 名字），v0.6/v0.7 的用户态是「一次跑一个
//! 程序，跑完回内核」。这里把它换成真正的进程对象 —— `fork`/`execve`/`wait4` 的
//! 全部前置条件：
//!
//! | 字段 | 为什么必须有它 |
//! | ---- | ---- |
//! | `space` | 每个进程一棵独立页表（`fork` 复制、`execve` 替换、退出时整棵归还） |
//! | `frame` | 停在 `syscall` 里的**完整用户寄存器帧**：`fork` 克隆它并令子进程 `rax=0` |
//! | 系统调用栈 / 陷入栈（每进程） | 父进程停在 `sys_wait` 里时，子进程的 `syscall` 与中断不能再落到同一条栈上 |
//! | `return_to` | 「谁在等我」：子进程退出时据此恢复父进程的 CR3/TSS/栈顶/地址空间/进程下标 |
//! | `exit_code` / `state` | `wait4` 的回收语义（Ready → Running → Zombie） |
//!
//! ## 一次用户态运行的完整状态转移
//!
//! ```text
//! run_process(P)                  ┌── P.frame 恢复 → ring3 ──────────────┐
//!   P.return_to = 当前内核现场     │                                     │
//!   切 CR3/TSS.RSP0/syscall_top ──▶│  P 执行（syscall 落在 P 的栈上）     │
//!   run_user_frame(&P.frame)       │                                     │
//!                                  └── sys_exit / ring3 异常 ─────────────┘
//!                                        P.state = Zombie, P.exit_code = n
//!                                        resume_kernel_side(P.return_to)
//!                                        return_to_suspended(&P.return_to)
//!                                     ↓
//!                              run_user_frame 的调用者继续（父进程的 sys_wait）
//! ```
//!
//! ## fork 的调度语义（写清楚，因为它和 Linux 不同）
//!
//! 本内核是**单 CPU、无用户态抢占**的：`fork()` 在父进程里返回子进程 PID，
//! 子进程**不会立刻运行** —— 它停在 `fork` 的那一帧里，等父进程 `wait4(child)`
//! 时才被恢复（于是 `fork()` 在子进程里返回 0，子进程接着走 `execve` 分支）。
//! 因此 `user/shell` 的 `let pid = fork(); if pid == 0 { execve(..) } else { wait4(pid) }`
//! 是**原样可用**的，而不是「需要改写以适配内核」。
//!
//! ## 已知限制（不是 bug，写在文档里）
//!
//! - 文件描述符表仍是全局的（`fs::file`），`fork` 不复制 fd、`execve` 前会重置；
//!   真正的每进程 fd 表依赖后续版本的「fd 表按进程持有」；
//! - 没有信号、没有进程组、没有 `wait4` 的 `WNOHANG`；
//! - 用户态没有抢占：一个进程会一直跑到 `exit`/`yield`/异常。

#![allow(dead_code)]

use crate::syscall::address_space::UserSpace;
use crate::syscall::trampoline::{RunCtx, UserFrame, NO_PROCESS as TRAMPOLINE_NO_PROCESS};

/// 进程表容量（同时决定静态内核栈的总量）。
pub const MAX_PROC: usize = 8;
/// `comm` 字段的容量（16 字节，与 Linux `task_struct.comm` 同宽）。
///
/// `ps` 读的就是它：`name` 是给串口日志看的 `&'static str`（`"fork-child"` /
/// `"exec"`），而 `comm` 是**定长**、可随 `execve` 改写为程序名（`"ls"`）的
/// 字段 —— 后者才是「这个进程现在是什么程序」的正确答案。
pub const PROC_COMM_LEN: usize = 16;
/// 「无当前进程」哨兵值（与 `trampoline::NO_PROCESS` 同值）。
pub const NO_PROCESS: usize = usize::MAX;
/// 每个进程的内核栈大小。
///
/// **为什么是 64 KiB 而不是 16 KiB**：v0.9 第三步实测踩过一次坑 —— `fork` 路径上
/// `UserSpace`（约 3 KiB）在 **debug 构建**里会被搬来搬去（`Self::new()` 的返回、
/// `clone_for_fork` 的局部量、`sys_fork` 的临时量……），实测栈用量约 32 KiB。
/// 16 KiB 的栈会在**不报错**的情况下越出栈底，踩掉相邻的 `.bss`（帧分配器的
/// `INITIALIZED` 被清零 ⇒ 之后所有 `frame::allocate()` 都返回 `None`）。
/// 这种故障只在串口上表现为「莫名其妙的 ENOMEM」，因此现在有一条
/// [栈底哨兵](STACK_CANARY) 把「栈溢出」变成可以立刻定位的告警。
const KSTACK_SIZE: usize = 64 * 1024;

/// 栈底哨兵（每进程内核栈的第一个 8 字节）。
///
/// 栈从高往低长，溢出的第一件事就是踩掉它。v0.9 第三步在一次真实的溢出上
/// 损失了很多时间：栈溢出**不会** #PF（内核栈在恒等映射里，下面全是合法内存），
/// 它只会静静地改写相邻的静态量。有了哨兵，[`release`] 会立刻报出来。
const STACK_CANARY: u64 = 0xC0DE_1E55_C0DE_1E55;

// 编译期约束：`RunCtx.current` 用 u64 传「无进程」，两个哨兵必须一致。
const _: () = assert!(NO_PROCESS as u64 == TRAMPOLINE_NO_PROCESS);

/// 进程状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcState {
    /// 已创建、尚未运行（`fork` 之后、`wait4` 之前的子进程）
    Ready,
    /// 正在 ring3 运行
    Running,
    /// 已退出、等待回收（`wait4` 取走退出码后释放）
    Zombie,
}

impl ProcState {
    /// 状态名（串口日志用）。
    pub const fn name(self) -> &'static str {
        match self {
            ProcState::Ready => "READY",
            ProcState::Running => "RUNNING",
            ProcState::Zombie => "ZOMBIE",
        }
    }
}

/// 进程控制块。
pub struct Process {
    /// 进程号
    pub pid: u64,
    /// 调试名（`/bin/sh`、`fork:/bin/ls` 之类）
    pub name: &'static str,
    /// 定长命令名（`ps` 读它；`create` 用 `name` 填充，`execve` 改写为程序名）。
    pub comm: [u8; PROC_COMM_LEN],
    /// 父进程 pid（0 = 内核启动上下文）
    pub parent_pid: u64,
    /// 父进程在进程表中的下标（[`NO_PROCESS`] = 内核启动上下文）
    pub parent: usize,
    /// 本进程在进程表中的下标
    pub slot: usize,
    /// 状态
    pub state: ProcState,
    /// 退出码（`state == Zombie` 时有效）
    pub exit_code: i64,
    /// 停在 `syscall` 里的用户寄存器帧（`fork` 克隆的正是它）
    pub frame: UserFrame,
    /// 地址空间（`None` = 内核上下文）
    pub space: Option<UserSpace>,
    /// 该进程被运行前填写的「回到谁那里去」
    pub return_to: RunCtx,
    /// 进程凭证（v1.0 + CKM-1）：uid/gid/能力位。
    ///
    /// 它**只由内核写**：`create` 给初始值，`fork` 沿袭父进程，`execve` 在命中
    /// CKM 组件表时换掉（`docs/ckm.md` 3.1）。用户态没有任何系统调用能读改它 ——
    /// 「我是谁」由内核决定，这正是第一版权限模型没有提权路径的原因。
    pub cred: crate::security::Cred,
}

impl Process {
    /// 该进程的内核栈顶（同时写进 `TSS.RSP0` 与 `CER_SYSCALL_STACK_TOP`）。
    pub fn kstack_top(&self) -> u64 {
        kstack_top(self.slot)
    }

    /// 该进程的内核栈底哨兵是否完好。
    pub fn stack_intact(&self) -> bool {
        stack_intact(self.slot)
    }

    /// 用 `name`（超出 [`PROC_COMM_LEN`] 的部分截断）改写 [`Process::comm`]。
    ///
    /// `execve` 成功后调用它，于是 `ps` 看到的是程序名而不是 `"exec"`。
    pub fn set_comm(&mut self, name: &str) {
        self.comm = comm_of(name);
    }
}

/// 把名字编码成定长 `comm`（不足部分补 0；不使用 NUL 终止符，长度固定 16）。
pub fn comm_of(name: &str) -> [u8; PROC_COMM_LEN] {
    let mut comm = [0u8; PROC_COMM_LEN];
    let bytes = name.as_bytes();
    let n = bytes.len().min(PROC_COMM_LEN);
    comm[..n].copy_from_slice(&bytes[..n]);
    comm
}

/// 进程表（`None` = 空闲槽位）。
static mut TABLE: [Option<Process>; MAX_PROC] = [const { None }; MAX_PROC];
/// 每进程的内核栈。
///
/// **一条栈同时服务 syscall 与 ring3 中断/异常**（与 Linux 相同）：
/// - `syscall` 指令不切栈，入口汇编显式 `mov rsp, [CER_SYSCALL_STACK_TOP]`；
/// - ring3 中断/异常由 CPU 依据 `TSS.RSP0` 切过来，两者写同一个栈顶。
///
/// 之所以需要「每进程」，而不是像 v0.6/v0.7 那样全局一条：父进程停在 `sys_wait`
/// 里时，子进程的 syscall 必须落在**另一条**栈上（见模块头注释）。
static mut KSTACKS: [[u8; KSTACK_SIZE]; MAX_PROC] = [[0; KSTACK_SIZE]; MAX_PROC];
/// 当前进程下标（[`NO_PROCESS`] = 内核启动上下文）。
static mut CURRENT: usize = NO_PROCESS;

#[inline]
fn current_ptr() -> *mut usize {
    core::ptr::addr_of_mut!(CURRENT)
}

#[inline]
fn table_base() -> *mut Option<Process> {
    core::ptr::addr_of_mut!(TABLE) as *mut Option<Process>
}

#[inline]
fn kstacks_base() -> *mut u8 {
    core::ptr::addr_of_mut!(KSTACKS) as *mut u8
}

/// 槽位 `slot` 的系统调用栈顶。
///
/// 栈顶**向下对齐到 16 字节**，这一点是必需的而不只是好看：
///
/// - `syscall` 入口把 6 个参数按 `[u64; 6]` 放在 `栈顶 - 128` 处，内核侧再把它
///   当成 `&[u64; 6]` 使用（见 `syscall::trampoline`）；
/// - `TSS.RSP0` 也指向这里，ring3 中断/异常由 CPU 在此压入 trap frame；
///
/// `KSTACKS` 是 `[[u8; N]; M]`（**对齐 1**），它在 `.bss` 里的实际地址由链接器
/// 决定 —— 只要前面新增了一个奇数长度的静态量，栈顶就会变成奇地址，那个
/// `&[u64; 6]` 随即成为 UB（debug 构建的 UB 检查会直接 panic：
/// "unsafe precondition(s) violated: slice::from_raw_parts requires ... alignment"）。
/// 因此在对齐上**不能指望运气**，这里显式对齐。
fn kstack_top(slot: usize) -> u64 {
    let top = unsafe { kstacks_base().add(slot * KSTACK_SIZE).add(KSTACK_SIZE) as u64 };
    top & !0xF
}

/// 清空进程表（内核启动时调用一次）并给每条内核栈打上栈底哨兵。
pub fn init() {
    unsafe {
        for i in 0..MAX_PROC {
            table_base().add(i).write(None);
            // 哨兵写在栈区间的**最低** 8 字节：栈溢出会先踩掉它
            (kstacks_base().add(i * KSTACK_SIZE) as *mut u64).write_unaligned(STACK_CANARY);
        }
        *current_ptr() = NO_PROCESS;
    }
}

/// 槽位 `slot` 的内核栈底哨兵是否完好（= 没有发生栈溢出）。
pub fn stack_intact(slot: usize) -> bool {
    if slot >= MAX_PROC {
        return true;
    }
    unsafe {
        (kstacks_base().add(slot * KSTACK_SIZE) as *const u64).read_unaligned() == STACK_CANARY
    }
}

/// 当前进程下标（[`NO_PROCESS`] = 内核启动上下文）。
pub fn current_index() -> usize {
    unsafe { *current_ptr() }
}

/// 设置当前进程下标。
pub fn set_current_index(slot: usize) {
    unsafe {
        *current_ptr() = slot;
    }
}

/// 当前进程的不可变引用。
pub fn current() -> Option<&'static Process> {
    let idx = current_index();
    if idx == NO_PROCESS {
        return None;
    }
    unsafe { table_base().add(idx).as_ref().and_then(|slot| slot.as_ref()) }
}

/// 当前进程的可变引用。
///
/// # Safety 语义
/// 返回 `&'static mut` 是刻意的：本内核单 CPU、用户态没有抢占，syscall 期间
/// 不会有人并发访问同一个进程。调用方必须保证不在持有该引用的同时再做一次
/// [`current_mut`]（否则就是别名可变引用）。
pub fn current_mut() -> Option<&'static mut Process> {
    let idx = current_index();
    if idx == NO_PROCESS {
        return None;
    }
    process_mut(idx)
}

/// 取槽位 `slot` 的进程（可变引用；`None` = 空闲）。
pub fn process_mut(slot: usize) -> Option<&'static mut Process> {
    if slot >= MAX_PROC {
        return None;
    }
    unsafe { table_base().add(slot).as_mut().and_then(|entry| entry.as_mut()) }
}

/// 取槽位 `slot` 的进程的裸指针（`None` = 空闲）。
///
/// syscall 层需要「长期持有某个进程的地址」，同时又要在别的调用里继续访问
/// 进程表 —— 因此这里给裸指针，而不是 `&mut`。
pub fn process_ptr(slot: usize) -> *mut Process {
    if slot >= MAX_PROC {
        return core::ptr::null_mut();
    }
    unsafe {
        match table_base().add(slot).as_mut() {
            Some(Some(p)) => p as *mut Process,
            _ => core::ptr::null_mut(),
        }
    }
}

/// 找一个空闲槽位。
pub fn free_slot() -> Option<usize> {
    for i in 0..MAX_PROC {
        if process_ptr(i).is_null() {
            return Some(i);
        }
    }
    None
}

/// 在槽位 `slot` 上登记一个新进程（`space`/`frame` 由调用方随后写入）。
///
/// 返回该进程的可变引用；槽位被占用则返回 `None`。
pub fn create(
    slot: usize,
    pid: u64,
    name: &'static str,
    parent_pid: u64,
    parent: usize,
) -> Option<&'static mut Process> {
    if slot >= MAX_PROC {
        return None;
    }
    unsafe {
        let entry = table_base().add(slot);
        if entry.as_ref().map(|s| s.is_some()).unwrap_or(true) {
            return None;
        }
        entry.write(Some(Process {
            pid,
            name,
            comm: comm_of(name),
            parent_pid,
            parent,
            slot,
            state: ProcState::Ready,
            exit_code: 0,
            frame: UserFrame::new_user(0, 0, 0, 0, 0, 0),
            space: None,
            return_to: RunCtx::default(),
            // 初始凭证是内核凭证：调用方（顶层程序运行 / `fork` / `execve`）
            // 负责随后把它改成正确的值。不复用「上一次跑过的进程」的凭证，
            // 否则释放-复用槽位会变成一条提权路径。
            cred: crate::security::Cred::kernel(),
        }));
        entry.as_mut().and_then(|s| s.as_mut())
    }
}

/// 释放槽位（退出码已被 `wait4` 取走后调用）；会 `Drop` 掉地址空间。
///
/// 同时检查该进程内核栈的栈底哨兵：栈溢出会静默踩坏相邻静态量（见 [`STACK_CANARY`]），
/// 所以这件事必须显式报出来，而不是等到帧分配器「莫名其妙地 ENOMEM」。
pub fn release(slot: usize) {
    if slot >= MAX_PROC {
        return;
    }
    if !stack_intact(slot) {
        crate::driver::serial::print("user: KERNEL STACK OVERFLOW detected in slot ");
        crate::driver::serial::print_dec(slot as u64);
        crate::driver::serial::print(" (per-process kernel stack is ");
        crate::driver::serial::print_dec((KSTACK_SIZE / 1024) as u64);
        crate::driver::serial::println(" KiB; enlarge it or reduce stack usage)");
    }
    unsafe {
        let entry = table_base().add(slot);
        // 先取走 space 并显式 drop（归还页帧），再清空槽位
        if let Some(Some(p)) = entry.as_mut() {
            p.space.take();
        }
        entry.write(None);
    }
    if current_index() == slot {
        set_current_index(NO_PROCESS);
    }
}

/// 在进程表里按 pid 找进程下标（可选限定父进程）。
pub fn find_by_pid(pid: u64, parent_pid: Option<u64>) -> Option<usize> {
    for i in 0..MAX_PROC {
        let ptr = process_ptr(i);
        if ptr.is_null() {
            continue;
        }
        let p = unsafe { &*ptr };
        if p.pid == pid && parent_pid.map_or(true, |want| p.parent_pid == want) {
            return Some(i);
        }
    }
    None
}

/// 当前存活的进程数。
pub fn live_count() -> usize {
    (0..MAX_PROC).filter(|&i| !process_ptr(i).is_null()).count()
}
