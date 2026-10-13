//! 套接字地址与网络配置的用户态形状（v1.0）
//!
//! 内核只认 16 字节的 `sockaddr_in` 与 16 字节的 `net_config` 记录（布局是 ABI，
//! 见 `abi::socket`）。用户程序不该自己按偏移拼字节，所以这里给
//! [`SockAddrIn`]（IPv4 + 端口的构造/读取）与 [`NetConfig`]（DHCP 协商结果）。
//!
//! 端口与地址一律用**网络字节序**（大端）保存 —— 与报文里的字段一致，
//! 内核因此不需要再转换一次。

use abi::socket::{
    NET_CONFIG_GATEWAY_OFFSET, NET_CONFIG_IP_OFFSET, NET_CONFIG_MAC_OFFSET,
    NET_CONFIG_SIZE, SOCKADDR_IN_SIZE,
};

pub use abi::socket::{
    AF_INET, AF_UNSPEC, IPPROTO_ICMP, IPPROTO_IP, IPPROTO_TCP, IPPROTO_UDP, PORT_ANY, SOCK_DGRAM,
    SOCK_FLAGS, SOCK_NONBLOCK, SOCK_STREAM, SOCK_TYPE_MASK, UDP_PAYLOAD_MAX,
};

/// IPv4 通配地址 `0.0.0.0`（`bind` 时表示「任意本机地址」）。
pub const INADDR_ANY: [u8; 4] = [0, 0, 0, 0];
/// 受限广播地址 `255.255.255.255`（`sendto` 的目标）。
pub const INADDR_BROADCAST: [u8; 4] = [255, 255, 255, 255];

/// 一个 `sockaddr_in`（16 字节，与内核 ABI 逐字节一致）。
#[derive(Clone, Copy)]
pub struct SockAddrIn {
    /// 原始字节（内核直接读这一段）
    pub bytes: [u8; SOCKADDR_IN_SIZE],
}

impl SockAddrIn {
    /// 由 `addr`（4 字节，网络字节序）与 `port`（主机序，内部转大端）构造。
    pub const fn new(addr: [u8; 4], port: u16) -> Self {
        let p = port.to_be_bytes();
        Self {
            bytes: [
                AF_INET as u8,
                (AF_INET >> 8) as u8,
                p[0],
                p[1],
                addr[0],
                addr[1],
                addr[2],
                addr[3],
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        }
    }

    /// 「任意本机地址 + 指定端口」的地址（`bind` 常用；`port = 0` 让内核分配临时端口）。
    pub const fn any(port: u16) -> Self {
        Self::new(INADDR_ANY, port)
    }

    /// 端口（主机序）。
    pub fn port(&self) -> u16 {
        u16::from_be_bytes([self.bytes[2], self.bytes[3]])
    }

    /// 地址（4 字节，网络字节序）。
    pub fn addr(&self) -> [u8; 4] {
        [self.bytes[4], self.bytes[5], self.bytes[6], self.bytes[7]]
    }

    /// 地址族（应当总是 [`AF_INET`]）。
    pub fn family(&self) -> u16 {
        u16::from_le_bytes([self.bytes[0], self.bytes[1]])
    }

    /// 作为 `&[u8]` 交给 syscall（长度恒为 [`SOCKADDR_IN_SIZE`]）。
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 空地址（`recvfrom` 的输出缓冲初值）。
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; SOCKADDR_IN_SIZE],
        }
    }
}

impl Default for SockAddrIn {
    fn default() -> Self {
        Self::empty()
    }
}

/// 内核在 DHCP 里协商到的网络配置（`net_config` 记录，16 字节）。
#[derive(Clone, Copy, Default)]
pub struct NetConfig {
    /// 记录原始字节（即 `net_config` 写出的那 16 字节）
    pub bytes: [u8; NET_CONFIG_SIZE],
}

impl NetConfig {
    /// 空记录。
    pub const fn new() -> Self {
        Self {
            bytes: [0u8; NET_CONFIG_SIZE],
        }
    }

    /// 本机 IPv4（4 字节，网络字节序）。
    pub fn ip(&self) -> [u8; 4] {
        let o = NET_CONFIG_IP_OFFSET;
        [self.bytes[o], self.bytes[o + 1], self.bytes[o + 2], self.bytes[o + 3]]
    }

    /// 网关 IPv4（4 字节，网络字节序）。
    pub fn gateway(&self) -> [u8; 4] {
        let o = NET_CONFIG_GATEWAY_OFFSET;
        [self.bytes[o], self.bytes[o + 1], self.bytes[o + 2], self.bytes[o + 3]]
    }

    /// 本机 MAC（6 字节）。
    pub fn mac(&self) -> [u8; 6] {
        let o = NET_CONFIG_MAC_OFFSET;
        [
            self.bytes[o],
            self.bytes[o + 1],
            self.bytes[o + 2],
            self.bytes[o + 3],
            self.bytes[o + 4],
            self.bytes[o + 5],
        ]
    }

    /// 作为 `&mut [u8]` 交给 syscall（长度恒为 [`NET_CONFIG_SIZE`]）。
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}
