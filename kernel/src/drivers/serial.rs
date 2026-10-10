//! 串口后端（v0.6）
//!
//! 把 `put_bytes` 暴露给 `syscall::file::sys_write`，仅承担最小调试输出。
//! 当前实现直接转发到硬件寄存器；后续可接 ISO 15742 / logging 后端。

/// 将 `count` 字节写入串口数据寄存器。每字节 `try_write` 一次，超时/错误时
/// 返回已写字节数。
pub fn put_bytes(buf: &[u8], count: usize) -> usize {
    let mut written = 0;
    for b in buf.iter().take(count) {
        if crate::driver::serial::try_write(*b) {
            written += 1;
        }
    }
    written
}
