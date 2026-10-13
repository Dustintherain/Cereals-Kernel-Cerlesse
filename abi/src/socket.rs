//! Socket ABI：`AF_INET` + `SOCK_DGRAM`（UDP）最小集（v1.0 实现）。
//!
//! 这一版只做**无连接 UDP**：`socket` / `bind` / `sendto` / `recvfrom` 四个调用
//!（外加 `net_config` 让用户态拿到本机地址与 MAC）。流式（TCP）与 `connect` /
//! `listen` / `accept` 仍是后续项，因此这里**不预声明**它们的常量 ——
//! 「哪个地址族/类型可用」应当能从定义里一眼读出，而不是靠运行时试。
//!
//! ## `sockaddr_in` 布局（16 字节，与 Linux 相同）
//!
//! ```text
//! 偏移  长度  字段
//! 0     2    sin_family（主机序；AF_INET = 2）
//! 2     2    sin_port（**网络字节序**/大端）
//! 4     4    sin_addr（**网络字节序**/大端）
//! 8     8    sin_zero（必须为 0）
//! ```
//!
//! 端口与地址用网络字节序是刻意的：它与 IPv4/UDP 报文里的字段是同一份字节，
//! 内核不需要再做一次转换（`open`/`write` 那套「指针 + 长度」仍然不变）。
//!
//! ## `net_config` 记录（16 字节）
//!
//! `net_config(buf, len)` 把内核在 DHCP 里协商到的网络配置导出给用户态：
//!
//! ```text
//! 偏移  长度  字段
//! 0     4    本机 IPv4（网络字节序）
//! 4     4    网关 IPv4（网络字节序）
//! 8     6    本机 MAC
//! 14    2    保留（= 0）
//! ```
//!
//! 用户态要 MAC 是因为 DHCP 之类的报文里要放 `chaddr`；本机地址让程序能判断
//! 「同网段直发还是经网关」（本内核不实现子网掩码，缺省按 /24 处理，见网络自测）。

/// 未指定地址族（`sockaddr` 的 `sa_family` 占位；本内核不接受它做 socket）。
pub const AF_UNSPEC: u16 = 0;
/// IPv4 地址族（本内核**唯一**支持的族）。
pub const AF_INET: u16 = 2;

/// 流式套接字（TCP）—— **尚未实现**，`socket` 会返回 `-EPROTONOSUPPORT`。
pub const SOCK_STREAM: u32 = 1;
/// 数据报套接字（UDP）—— 本内核支持的类型。
pub const SOCK_DGRAM: u32 = 2;
/// 套接字类型掩码（低 4 位；高位是 `SOCK_NONBLOCK`/`SOCK_CLOEXEC`）。
pub const SOCK_TYPE_MASK: u32 = 0xF;
/// `SOCK_NONBLOCK`：非阻塞（**接受但当前无效果** —— `recvfrom` 本来就有超时上限）。
pub const SOCK_NONBLOCK: u32 = 0x800;
/// `SOCK_CLOEXEC`：随 `execve` 关闭（**接受但当前无效果**）。
pub const SOCK_CLOEXEC: u32 = 0x80000;
/// `socket` 的 `type` 参数里允许出现的高位标志。
pub const SOCK_FLAGS: u32 = SOCK_NONBLOCK | SOCK_CLOEXEC;

/// `IPPROTO_IP`：由内核按类型选择协议（`SOCK_DGRAM` → UDP）。
pub const IPPROTO_IP: u32 = 0;
/// `IPPROTO_ICMP`：ICMP（内核有 Echo 自测，但没有 ICMP socket）。
pub const IPPROTO_ICMP: u32 = 1;
/// `IPPROTO_TCP`：TCP（尚未实现）。
pub const IPPROTO_TCP: u32 = 6;
/// `IPPROTO_UDP`：UDP（本内核支持的协议）。
pub const IPPROTO_UDP: u32 = 17;

/// `sockaddr_in` 的字节数（用户态缓冲必须至少这么大）。
pub const SOCKADDR_IN_SIZE: usize = 16;
/// `sockaddr_in` 里 `sin_port` 的偏移。
pub const SOCKADDR_IN_PORT_OFFSET: usize = 2;
/// `sockaddr_in` 里 `sin_addr` 的偏移。
pub const SOCKADDR_IN_ADDR_OFFSET: usize = 4;

/// `bind` / `sendto` 里的「任意端口」：内核分配一个临时端口。
pub const PORT_ANY: u16 = 0;

/// 单个 UDP 数据报载荷上限（经典以太网 MTU 1500 - 20 IP - 8 UDP = 1472）。
/// 超过它的 `sendto` 返回 `-EMSGSIZE`。
pub const UDP_PAYLOAD_MAX: usize = 1472;

/// `net_config` 记录的字节数。
pub const NET_CONFIG_SIZE: usize = 16;
/// `net_config` 记录里本机 IPv4 的偏移（网络字节序）。
pub const NET_CONFIG_IP_OFFSET: usize = 0;
/// `net_config` 记录里网关 IPv4 的偏移（网络字节序）。
pub const NET_CONFIG_GATEWAY_OFFSET: usize = 4;
/// `net_config` 记录里本机 MAC 的偏移（6 字节）。
pub const NET_CONFIG_MAC_OFFSET: usize = 8;

// 编译期自洽：记录里各字段不越界（与 `record.rs` 的 R 系列断言同一目的）。
const _: () = assert!(NET_CONFIG_MAC_OFFSET + 6 <= NET_CONFIG_SIZE);
const _: () = assert!(SOCKADDR_IN_ADDR_OFFSET + 4 <= SOCKADDR_IN_SIZE);
const _: () = assert!(SOCKADDR_IN_PORT_OFFSET + 2 <= SOCKADDR_IN_SIZE);
const _: () = assert!(SOCK_FLAGS & SOCK_TYPE_MASK == 0);
