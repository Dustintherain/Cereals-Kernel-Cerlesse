//! 用户态入口 → 内核分发 → 返回（v0.6）
//!
//! 这是用户态 `syscall` 指令陷入内核后的第一层 Rust 入口。
//! 当前阶段的责任很小且明确：
//! - 从用户态传入的寄存器快照（rax = syscall number，rdi..r10 为参数）里
//!   取出 syscall number；
//! - 查 `SYS_TABLE`，如果号合法且有 handler，就调用它；
//! - 把 handler 返回值写回 rax，准备 `sysretq` 返回。
//!
//! v0.6 还没有添加真正的用户页表，所以这里不做页表切换/指针校验的完整版本，
//! 仅实现最小可链接的分发桩。完整的 `sys_enter` 语义（用户内存访问校验、返回值
//! 包装、错误码语义）在用户态地址空间启用后再补。

/// 用户态 syscall 入口。
///
/// # Safety
///
/// 调用方必须处于合法的中断/陷入上下文，且寄存器约定（Linux x86_64 syscall ABI）
/// 已由汇编桩填好。`args` 中的指针/长度在本版本不校验，仅作为传递字。
///
/// 参数约定（v0.6 桩）：`args[0..5]` = rdi, rsi, rdx, r10, r8。
#[inline]
pub unsafe extern "C" fn sys_enter(
    nid: u64,
    args: [u64; 5],
    space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    if let Some(handler) = crate::syscall::syscall_table::parse(nid) {
        let args = args;
        let space = space;
        handler(&args, space).max(0) as u64
    } else {
        crate::syscall::syscall_table::syscall_enosys(&args, space).max(0) as u64
    }
}
