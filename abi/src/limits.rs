//! 参数上限（内核校验与用户态构造缓冲都要用同一批数字）。
//!
//! 这些都是「**越界即 `EINVAL`**」的硬上限：内核逐个校验，用户态据此决定缓冲大小，
//! 两边不一样就会出现「用户态按 256 构造、内核按 128 拒绝」这类难查的失败。

/// 路径长度上限（`open` / `execve` / `getdents` / 存储类调用的路径参数）。
pub const PATH_MAX: u64 = 256;
/// 单次 `read`/`write` 允许的最大字节数。
pub const IO_MAX: u64 = 4096;
/// `execve` 的 `argv` 项数上限。
pub const ARG_MAX: u64 = 8;
/// 单个 `argv` 参数的长度上限。
pub const ARG_MAX_LEN: u64 = 256;
/// `argv` 字符串 + 指针数组占用的用户栈上限（用户栈 16 KiB，这里只占一小块）。
pub const ARG_MAX_BYTES: u64 = 1024;
