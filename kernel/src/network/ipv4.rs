//! IPv4（v0.9 实现）
//!
//! 只做「无选项、无分片重组」的 IPv4：IHL 必须为 5（20 字节头），
//! `flags & MF`、`fragment offset != 0` 的报文明确拒绝（v0.9 不做重组）。
//! 这覆盖了 UDP/DHCP/ICMP 在局域网内的全部实际形态，也让「收到什么算合法」
//! 有唯一答案，而不是靠一个永远不触发的重组缓冲。
//!
//! ```text
//! 偏移  长度  字段
//! 0     1     version(4) | IHL(4)   （必须 0x45）
//! 1     1     DSCP/ECN
//! 2     2     total length（大端，含头）
//! 4     2     identification
//! 6     2     flags(3) | fragment offset(13)
//! 8     1     TTL
//! 9     1     protocol
//! 10    2     header checksum
//! 12    4     source address
//! 16    4     destination address
//! ```
//!
//! 校验和（RFC 1071 的 16 位反码和）在这里实现一次，IP 头与 UDP 伪头都复用它
//!（[`sum16`] / [`finish`] 拆开是为了让 UDP 能把伪头 + 头 + 载荷累积到同一个和里）。

/// IPv4 头长度（无选项）。
pub const HEADER_LEN: usize = 20;

/// 协议号：ICMP。
pub const PROTO_ICMP: u8 = 1;

/// 协议号：UDP。
pub const PROTO_UDP: u8 = 17;

/// 一个 IPv4 地址（网络字节序的 4 字节）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Address([u8; 4]);

impl Address {
    /// 由 4 个八位组构造。
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// 由八位组数组构造。
    pub const fn from_octets(octets: [u8; 4]) -> Self {
        Self(octets)
    }

    /// 取八位组数组。
    pub const fn octets(self) -> [u8; 4] {
        self.0
    }

    /// 未指定地址 `0.0.0.0`（DHCP 请求的源地址）。
    pub const UNSPECIFIED: Self = Self([0, 0, 0, 0]);
    /// 受限广播地址 `255.255.255.255`（DHCP DISCOVER 的目的地址）。
    pub const BROADCAST: Self = Self([255, 255, 255, 255]);
}

/// 解析出的 IPv4 头（只保留上层会用到的字段）。
///
/// 头里的 `total_len`/`ttl`/`identification` 不在这里暴露：调用方拿到的是已经按
/// `total_len` 截好的 `payload`（`payload.len() == total_len - 20`），需要再看头字段
/// 时再加，而不是先塞进来不用。
#[derive(Clone, Copy, Debug)]
pub struct Ipv4Header {
    /// 源地址
    pub src: Address,
    /// 目的地址
    pub dst: Address,
    /// 上层协议号
    pub protocol: u8,
}

/// 把 `data` 按 16 位大端字累加进 `acc`（奇数尾字节左移 8 位）。
pub fn sum16(mut acc: u32, data: &[u8]) -> u32 {
    let mut i = 0;
    while i + 1 < data.len() {
        acc += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        acc += (data[i] as u32) << 8;
    }
    acc
}

/// 折叠 `acc` 的高位进位并取反，得到 16 位反码校验和。
pub fn finish(mut acc: u32) -> u16 {
    while acc >> 16 != 0 {
        acc = (acc & 0xFFFF) + (acc >> 16);
    }
    !(acc as u16)
}

/// 一段内存的互联网校验和（用于 IP 头）。
pub fn checksum(data: &[u8]) -> u16 {
    finish(sum16(0, data))
}

/// 写一个 IPv4 头，返回 payload 的可写切片。
///
/// 校验和按规范先置 0 再计算（接收方对包含校验和的整个头求和应为 0xFFFF）。
pub fn build(
    buf: &mut [u8],
    src: Address,
    dst: Address,
    protocol: u8,
    ttl: u8,
    identification: u16,
    payload_len: usize,
) -> Option<&mut [u8]> {
    let total_len = HEADER_LEN.checked_add(payload_len)?;
    if total_len > u16::MAX as usize || buf.len() < HEADER_LEN {
        return None;
    }
    buf[0] = 0x45; // version 4 | IHL 5
    buf[1] = 0x00; // DSCP/ECN
    buf[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    buf[4..6].copy_from_slice(&identification.to_be_bytes());
    buf[6..8].copy_from_slice(&0u16.to_be_bytes()); // 不分片
    buf[8] = ttl;
    buf[9] = protocol;
    buf[10..12].copy_from_slice(&0u16.to_be_bytes()); // 校验和占位
    buf[12..16].copy_from_slice(&src.octets());
    buf[16..20].copy_from_slice(&dst.octets());
    let sum = checksum(&buf[..HEADER_LEN]);
    buf[10..12].copy_from_slice(&sum.to_be_bytes());
    Some(&mut buf[HEADER_LEN..])
}

/// 解析一个 IPv4 报文，返回头与 payload 切片。
///
/// 拒绝：版本/ IHL 不是 4/5、总长小于头、总长超过实际字节数、
/// 分片（`MF` 置位或 offset != 0）、校验和错误。
pub fn parse(buf: &[u8]) -> Option<(Ipv4Header, &[u8])> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    if buf[0] != 0x45 {
        return None; // 只接受 version 4 + 无选项
    }
    let total_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if total_len < HEADER_LEN || total_len > buf.len() {
        return None;
    }
    let flags_frag = u16::from_be_bytes([buf[6], buf[7]]);
    if flags_frag & 0x2000 != 0 || flags_frag & 0x1FFF != 0 {
        return None; // MF 或 fragment offset：v0.9 不做重组
    }
    if checksum(&buf[..HEADER_LEN]) != 0 {
        return None; // 校验和应当让整头求和为 0xFFFF（即 checksum()==0）
    }
    let header = Ipv4Header {
        src: Address::from_octets([buf[12], buf[13], buf[14], buf[15]]),
        dst: Address::from_octets([buf[16], buf[17], buf[18], buf[19]]),
        protocol: buf[9],
    };
    Some((header, &buf[HEADER_LEN..total_len]))
}
