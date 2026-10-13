//! ICMP（v1.0 第三步之后的网络增项）：只做 **Echo（ping）**
//!
//! 与 [`crate::network::udp`] 一样是**纯函数**的组帧/解析：不持有状态，方便
//! 直接验证。它接在现有 IPv4 收发路径上，不改动 Ethernet / IPv4。
//!
//! ```text
//! 偏移  长度  字段
//! 0     1    类型（8 = Echo Request，0 = Echo Reply）
//! 1     1    代码（Echo 恒 0）
//! 2     2    校验和（覆盖整条 ICMP 报文）
//! 4     2    标识符（identifier）
//! 6     2    序号（sequence）
//! 8     ...  Echo 数据（请求里发什么，应答里回显什么）
//! ```
//!
//! 校验和沿用 [`crate::network::ipv4::checksum`]（与 IP 头同一套 16 位反码和），
//! 因此 ICMP 不引入第二份校验实现。解析时要求「整条报文求和为 0」，与 UDP 的
//! 「非零就校验」不同：ICMP 的校验和是**必填**的（RFC 792 没有「不计算」的取值）。
//!
//! 未做：ICMP 差错报文（Destination Unreachable / Time Exceeded 等）与分片。
//! Echo 足以覆盖「网络路径通不通」这条验收线，差错报文等有实际消费者时再加。

/// ICMP 头长度（Echo 固定 8 字节）。
pub const HEADER_LEN: usize = 8;
/// Echo Reply。
pub const TYPE_ECHO_REPLY: u8 = 0;
/// Echo Request。
pub const TYPE_ECHO_REQUEST: u8 = 8;

/// 解析出的 Echo 报文。
#[derive(Clone, Copy, Debug)]
pub struct Echo<'a> {
    /// 类型（`0` 应答 / `8` 请求）
    pub echo_type: u8,
    /// 标识符
    pub id: u16,
    /// 序号
    pub seq: u16,
    /// Echo 数据（不含 8 字节头）
    pub payload: &'a [u8],
}

/// 写一条 ICMP Echo 报文（头 + 数据），返回总字节数。
///
/// `echo_type` 是 [`TYPE_ECHO_REQUEST`] 或 [`TYPE_ECHO_REPLY`]（两者格式相同）。
pub fn build(buf: &mut [u8], echo_type: u8, id: u16, seq: u16, payload: &[u8]) -> Option<usize> {
    let total = HEADER_LEN.checked_add(payload.len())?;
    if buf.len() < total {
        return None;
    }
    buf[0] = echo_type;
    buf[1] = 0; // code：Echo 恒 0
    buf[2..4].copy_from_slice(&0u16.to_be_bytes()); // 校验和占位
    buf[4..6].copy_from_slice(&id.to_be_bytes());
    buf[6..8].copy_from_slice(&seq.to_be_bytes());
    buf[HEADER_LEN..total].copy_from_slice(payload);
    let sum = crate::network::ipv4::checksum(&buf[..total]);
    buf[2..4].copy_from_slice(&sum.to_be_bytes());
    Some(total)
}

/// 解析一条 ICMP 报文；校验和不为 0（即求和未达到 0xFFFF）返回 `None`。
pub fn parse(buf: &[u8]) -> Option<Echo<'_>> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    if crate::network::ipv4::checksum(buf) != 0 {
        return None;
    }
    Some(Echo {
        echo_type: buf[0],
        id: u16::from_be_bytes([buf[4], buf[5]]),
        seq: u16::from_be_bytes([buf[6], buf[7]]),
        payload: &buf[HEADER_LEN..],
    })
}
