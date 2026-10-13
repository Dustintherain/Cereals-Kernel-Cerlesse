//! `-errno` 约定（编号沿用 Linux x86_64 的取值）。
//!
//! 系统调用的返回值**非负即正常值**，**负数即 `-errno`**
//! （`errno = -(返回值)`，不是「返回 errno 的负值」之外的任何约定：
//! `rax = -14` 表示 `EFAULT`）。规范类型是 `i64`：寄存器是 64 位，
//! 内核的 handler 返回 `i64`，libc 侧把它按 `isize` 使用（同宽，显式转换一次）。
//!
//! 只列**内核真的会返回**的 errno —— 不预先声明一批用不上的号，
//! 否则「哪个错误可能发生」就没法从定义里读出来。

/// `-EPERM`：**能力不足**（CKM 判定拒绝，例如写受保护前缀）。
///
/// 与 [`EACCES`] 刻意区分：`EPERM` = 「你的能力不够」，`EACCES` = 「文件的 mode 不允许」。
pub const EPERM: i64 = -1;
/// `-ENOENT`：路径不存在。
pub const ENOENT: i64 = -2;
/// `-EIO`：设备读写/扫描失败（存储类调用）。
pub const EIO: i64 = -5;
/// `-EBADF`：fd 无效 / 未打开 / 打开模式与操作不符。
pub const EBADF: i64 = -9;
/// `-ECHILD`：`wait4` 找不到该 PID 的直接子进程。
pub const ECHILD: i64 = -10;
/// `-EAGAIN`：本次没有数据可用（非阻塞语义，不是错误：`read(0)` 无输入时会返回它）。
pub const EAGAIN: i64 = -11;
/// `-ENOMEM`：内核内存不足。
pub const ENOMEM: i64 = -12;
/// `-EACCES`：mode 不允许（POSIX）。
pub const EACCES: i64 = -13;
/// `-EFAULT`：用户传了不属于自己的地址区间。
pub const EFAULT: i64 = -14;
/// `-EISDIR`：目标是目录，但调用方要当文件用。
pub const EISDIR: i64 = -21;
/// `-EINVAL`：参数非法（未知 `flags` 位、越界长度、缓冲太小等）。
pub const EINVAL: i64 = -22;
/// `-ENOSPC`：空间不足（fd 用尽 / 超过 RAMFS 上限）。
pub const ENOSPC: i64 = -28;
/// `-ENOSYS`：号合法但内核未实现（或号越界时的兜底返回）。
pub const ENOSYS: i64 = -38;

// ---- Socket / 网络类（Linux 取值）----
/// `-ENODEV`：系统里没有对应的设备（`net_config` 在**没有网卡**时返回它）。
pub const ENODEV: i64 = -19;
/// `-ENOTSOCK`：fd 有效但不是套接字（`bind`/`sendto`/`recvfrom` 传了文件 fd）。
pub const ENOTSOCK: i64 = -88;
/// `-EMSGSIZE`：报文太长（`sendto` 的载荷超过 [`crate::socket::UDP_PAYLOAD_MAX`]）。
pub const EMSGSIZE: i64 = -90;
/// `-EPROTONOSUPPORT`：协议不支持（例如 `socket(AF_INET, SOCK_STREAM, ..)`）。
pub const EPROTONOSUPPORT: i64 = -93;
/// `-EOPNOTSUPP`：操作不支持（例如 `sendto` 传了非 0 `flags`）。
pub const EOPNOTSUPP: i64 = -95;
/// `-EAFNOSUPPORT`：地址族不支持（本内核只认 `AF_INET`）。
pub const EAFNOSUPPORT: i64 = -97;
/// `-EADDRINUSE`：端口已被别的套接字占用（`bind`）。
pub const EADDRINUSE: i64 = -98;
/// `-EADDRNOTAVAIL`：`bind` 的地址不是本机地址（本内核只接受 `0.0.0.0` 或本机租约地址）。
pub const EADDRNOTAVAIL: i64 = -99;
/// `-ENETDOWN`：网络不可用（没有网卡，或 DHCP 尚未协商出地址）。
pub const ENETDOWN: i64 = -100;
/// `-ENOTCONN`：套接字未连接（`read`/`write` 一个 UDP 套接字时返回它；
/// 本内核没有 `connect`，因此 UDP 一律走 `sendto`/`recvfrom`）。
pub const ENOTCONN: i64 = -107;
/// `-EHOSTUNREACH`：目的地址的 MAC 解析不到（ARP 超时且没有可用网关）。
pub const EHOSTUNREACH: i64 = -113;
