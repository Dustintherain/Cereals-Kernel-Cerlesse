//! 文件类系统调用（v0.7 实现）
//!
//! 当前版本尚未接入文件系统；`open/close/read/write` 按“未实现”返回 `-ENOSYS`。
//! 后续接`VFS` 后再补充，这里先保留接口形状。

/// 系统调用 `open(path, flags, mode)`。
pub unsafe extern "sysv64" fn sys_open(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}

/// 系统调用 `close(fd)`。
pub unsafe extern "sysv64" fn sys_close(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}

/// 系统调用 `read(fd, buf, count)`。
pub unsafe extern "sysv64" fn sys_read(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}

/// 系统调用 `write(fd, buf, count)`。
pub unsafe extern "sysv64" fn sys_write(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    -1
}
