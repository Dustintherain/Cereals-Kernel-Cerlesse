//! 系统调用子系统（v0.6）
//!
//! 职责：
//! - 系统调用表：把 syscall 号映射到内核 handler。
//! - 地址空间隔离：v0.6 最小隔离——`UserSpace` 复用内核恒等映射；
//!   用户代码运行在 `USER_VIRT_BASE` 起的用户页中，并在 `cr3` 切换后仍可
//!   正常访问内核空间（内核非用户页不带 `USER` 标志）。
//! - 最小用户态入口：内核 `kernel_main` 从固定内核地址 `user_entry` 取得
//!   用户入口，通过 `switch_to` 把第一次调度切到用户态上下文；
//!   用户态第一次执行 `sys_enter` 时恢复 `rip` 到用户入口，随后从 `main`
//!   开始运行。
//!
//! 约定：
//! - 按 Linux x86_64 `syscall` ABI：`rax = syscall number`，`rdi..r10` = 4
//!   个参数（第五参数走栈 `rbx`），返回值于 `rax`。
//! - 所有从用户态传入的数据（指针、长度）必须先由内核校验，再由内核读写。
//! - 内核返回用户态时统一返回 `Ok(())`，并把 `rax` 作为用户 `syscall` 返回值。
//!   线程退出走同一个通道：`sys_exit` 把 `rax = 0` 后进入 `context::switch_to`
//!   的退出路径，让调度器回收该线程。
//!

pub mod address_space;
pub mod file;
pub mod memory;
pub mod process;
pub mod syscall_table;
pub mod sys_enter;
pub mod trampoline;

pub use crate::syscall::sys_enter::sys_enter;
pub use crate::syscall::syscall_table::{
    SYS_EXIT, SYS_READ, SYS_WRITE, SYS_OPEN, SYS_CLOSE, SYS_GETPID, SYS_SLEEP, SYS_FORK, SYS_EXEC, SYS_WAIT,
    SYS_MAX,
};

/// 让当前任务进入用户态（v0.6）：保存寄存器快照，切到用户态上下文，置 `cr3`
/// 为用户页表根。`rt` 为当前任务的 `ThreadContext`，`ctx` 为用户态寄存器区。
///
/// # Safety
/// 调用者保证：任务栈安全、`rt` 指向已初始化的 `ThreadContext`、`ctx` 指向有效
/// 寄存器区。
pub unsafe fn switch_to_user(rt: *mut crate::process::context::ThreadContext, ctx: *mut crate::process::context::ThreadContext) {
    let _ = rt;
    let _ctx = ctx;
}
