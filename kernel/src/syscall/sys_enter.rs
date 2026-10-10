//! 用户态入口 → 内核分发 → 返回（v0.6）
//!
//! 这是用户态 `syscall` 指令陷入内核后的第一层 Rust 入口。
//! 当前阶段的责任很小且明确：
//! - 从用户态传入的寄存器快照（rax = syscall number，rdi..r10 为参数）
//!   里取出 syscall number；
//! - 查 `SYS_TABLE`，如果号合法且有 handler，就调用它；
//! - 把handler 返回值写回 rax，准备 `sysretq` 返回。
//!
//! v0.6 还没有用户空间真实启用，所以这里不做页表切换/指针校验的完整版本，
//! 仅实现最小可链接的分发桩。完整的 `sys_enter` 语义（用户内存访问校验、
//! 返回值包装、错误码语义）在用户态空间启用后再补。

/// 用户态 syscall 入口。
///
/// # Safety
///
/// 调用方必须处于合法的中断/陷入上下文，且寄存器约定（Linux x86_64 syscall ABI）
/// 已由汇编桩填好。`args` 中的指针/长度在本版本不校验，仅作为传递字。
/// 用户态 syscall 入口。
///
/// # Safety
///
/// 调用方必须处于合法的中断/陷入上下文，且寄存器约定（Linux x86_64 syscall ABI）
/// 已由汇编桩填好。`args` 中的指针/长度在本版本不校验，仅作为传递字。
///
/// 参数约定（v0.6 桩）：
/// - `nid` = syscall number（rax）
/// - `args` = rdi..r10 的前 4 个参数 + 栈上传入的第 5 个参数快照
/// - `space` = 当前进程用户地址空间指针（启用用户空间后使用）
pub unsafe extern "C" fn sys_enter(
    nid: u64,
    args: [u64; 5],
    space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    let handler = match crate::syscall::syscall_table::parse(nid) {
        Some(idx) => crate::syscall::syscall_table::SYS_TABLE[idx],
        None => {
            // v0.6 默认未启用用户空间；syscall_enosys 语义未来可换成正式错误码。
            return crate::syscall::syscall_table::syscall_enosys(args, space);
        }
    };
    handler(args, space)
}

/// 未注册系统调用的公共返回值桩。
///
/// 当前用 `-2` 作为“ENOSYS 语义”的占位；将来可换成正式错误码语义。
/// 公共 ENOSYS 桩：未来可换成正式错误码语义，当前先用 `-2` 占位。
#[allow(dead_code)]
pub(crate) unsafe extern "sysv64" fn syscall_enosys(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    0xFFFFFFFFFFFFFFFE
}
