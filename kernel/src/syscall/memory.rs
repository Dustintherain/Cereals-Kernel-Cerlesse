//! 内存类系统调用（v0.6 实现）
//!
//! 当前版本只保证“用户态地址空间隔离”的最小接口；`brk/mmap` 占位。

/// 系统调用 `brk(addr)`：v0.6 占位，返回 `-ENOSYS`。
pub unsafe extern "sysv64" fn sys_brk(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}

/// 系统调用 `mmap(addr, len, prot, flags, fd, offset)`：v0.6 占位，返回 `-ENOSYS`。
pub unsafe extern "sysv64" fn sys_mmap(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}
