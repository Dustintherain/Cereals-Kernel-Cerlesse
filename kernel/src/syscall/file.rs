//! 文件类系统调用（v0.6 最小实现）
//!
//! 本阶段仅接入 `write` 的串口后端：`fd=1` 时将用户缓冲拷贝到串口直接打印。
//! `read / open / close` 保留骨架，返回 `-ENOSYS`。
//!
//! 错误返回：成功返回非负值；失败返回 `-errno`（如 `-ENOSYS` = -38）。

use core::slice;

/// 用户地址空间的最小校验：指针越界／超出用户区域时返回 false。
#[inline]
pub(crate) fn is_valid_user_ptr(_space: *const crate::syscall::address_space::UserSpace, _addr: u64, _len: usize) -> bool {
    // v0.6 中用户页表尚未启用，所有用户态地址均认为可访问（后续 v0.7 补全）。
    true
}

/// 系统调用 `write(fd, buf, count)`：v0.6 最小实现。
/// fd=1（stdout）时把 `count` 字节从用户缓冲拷贝到串口。
pub unsafe fn sys_write(args: &[u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> i64 {
    let fd = args[0] as i32;
    let buf = args[1];
    let count = args[2] as usize;

    if fd != 1 {
        return -3; // -EBADF（v0.6 未实现其他 fd）
    }
    if count == 0 {
        return 0;
    }
    if !is_valid_user_ptr(_space, buf, count) {
        return -14; // -EFAULT
    }

    // 直接写到串口；若串口初始化未完成则返回 0。
    let _bytes = slice::from_raw_parts(buf as *const u8, count);
    crate::driver::serial::put_bytes(_bytes, count);
    count as i64
}
