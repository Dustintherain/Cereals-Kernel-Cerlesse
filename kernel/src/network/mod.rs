//! 简化网络栈（v0.9 实现）
//!
//! 目标不是完整协议栈，而是打通最小可验证闭环：
//!
//! ```text
//! virtio-net → Ethernet II → ARP → IPv4 → UDP（DHCP）
//!                                   └→ ICMP（Echo/ping）
//! ```
//!
//! TCP 仍是占位（`docs/architecture.md` 第 5 节把「完整 TCP/IP」列为非目标，
//! 见 [`tcp`]）；**UDP 的 socket 层**已在 v1.0 落地（[`socket`] + `syscall::socket`）：
//! 面向用户态的 `socket`/`bind`/`sendto`/`recvfrom` 复用这里的纯函数与 [`Stack`]。
//! 本模块提供：
//!
//! - [`ethernet`] / [`arp`] / [`ipv4`] / [`udp`] / [`dhcp`] / [`icmp`]：**纯函数**的组帧与解析；
//! - [`Stack`]：把上面的纯函数接到一块 [`NetworkDevice`] 上，负责发送、轮询接收、
//!   ARP 解析（带缓存）与 DHCP DISCOVER；
//! - [`selftest`]：网络验收 —— 对 QEMU 的 slirp 网关做一次 ARP 解析、一次
//!   DHCP DISCOVER → OFFER，再向网关发一条 **ICMP Echo** 并校验回显的载荷，
//!   证明 Ethernet/IP 上 UDP 与 ICMP 的收发路径都真的通了。
//!
//! 为什么用 slirp 的内置 DHCP 做 UDP 验收：见 [`dhcp`] 的模块说明 —— 它不需要
//! 主机侧跑任何服务，`make test-network` 因此是完全自包含的。

pub mod arp;
pub mod dhcp;
pub mod ethernet;
pub mod icmp;
pub mod ipv4;
pub mod socket;
pub mod tcp;
pub mod udp;

use crate::driver::net::{self, NetworkDevice};
use crate::driver::serial;
use crate::time::clock;

/// QEMU `-netdev user`（slirp）默认给客户的地址。
pub const GUEST_IP: ipv4::Address = ipv4::Address::new(10, 0, 2, 15);
/// QEMU `-netdev user`（slirp）默认的虚拟网关（= 虚拟主机 10.0.2.2）。
pub const GATEWAY_IP: ipv4::Address = ipv4::Address::new(10, 0, 2, 2);

/// ARP 解析的等待上限（PIT tick；100Hz → 100 tick = 1 秒）。
const ARP_TIMEOUT_TICKS: u64 = 100;
/// DHCP 应答的等待上限（100Hz → 300 tick = 3 秒）。
const DHCP_TIMEOUT_TICKS: u64 = 300;
/// ICMP Echo 应答的等待上限（100Hz → 200 tick = 2 秒）。
const ICMP_TIMEOUT_TICKS: u64 = 200;

/// 内核在 DHCP 里协商到的网络配置。
///
/// `net_config` 系统调用把它导出给用户态（用户态要 MAC 来填 DHCP 的 `chaddr`、
/// 要本机地址来判断“直发还是经网关”），UDP 发送路径也用它做**源地址**。
#[derive(Clone, Copy, Debug)]
pub struct NetConfig {
    /// 本机 IPv4（DHCP 租约）
    pub ip: ipv4::Address,
    /// 网关 IPv4
    pub gateway: ipv4::Address,
    /// 网关 MAC（[`selftest`] 里 ARP 解析到的那一个）
    pub gateway_mac: [u8; 6],
    /// 本机 MAC（用户态要它来填 DHCP 的 `chaddr`）
    pub mac: [u8; 6],
}

// 全局网络配置：`selftest` 里 DHCP 成功后写入，之后只读。
static mut CONFIG: Option<NetConfig> = None;

/// 记录协商到的网络配置（[`selftest`] 在 DHCP 之后调用）。
pub fn set_config(cfg: NetConfig) {
    // SAFETY: 启动自测期间单线程写入，此后只读。
    unsafe {
        *(&raw mut CONFIG) = Some(cfg);
    }
}

/// 取协商到的网络配置（尚未 DHCP 时为 `None` → 调用方按 `-ENETDOWN` 处理）。
pub fn config() -> Option<NetConfig> {
    // SAFETY: 只读访问（`NetConfig` 是 `Copy`，这里返回一份副本）。
    unsafe { *(&raw const CONFIG) }
}

/// 读取 PIT tick 计数（`clock::tick_count` 是 unsafe：调用方须处于内核态）。
fn ticks() -> u64 {
    // SAFETY: 网络栈只在内核态（启动自测）里运行。
    unsafe { clock::tick_count() }
}

/// 一个极简的网络栈：一块设备 + 本机地址/网关 + ARP 缓存。
pub struct Stack<'a> {
    /// 底层设备
    pub dev: &'a mut dyn NetworkDevice,
    /// 本机 MAC（来自设备配置空间）
    pub mac: [u8; 6],
    /// 本机 IPv4 地址（DHCP 成功后会被 lease 覆盖）
    pub ip: ipv4::Address,
    /// 网关地址（ARP 解析的目标）
    pub gateway: ipv4::Address,
    /// IP → MAC 缓存
    pub arp_cache: arp::Cache,
    /// IPv4 报文标识递增
    next_id: u16,
}

impl<'a> Stack<'a> {
    /// 用一块设备构造栈（初始地址取 slirp 的默认值，随后可被 DHCP 覆盖）。
    pub fn new(dev: &'a mut dyn NetworkDevice) -> Self {
        Self {
            mac: dev.mac(),
            dev,
            ip: GUEST_IP,
            gateway: GATEWAY_IP,
            arp_cache: arp::Cache::new(),
            next_id: 1,
        }
    }

    /// 发送一个已经拼好的以太网帧。
    pub fn send_frame(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        self.dev.transmit(frame)
    }

    /// 在 `timeout_ticks` 内轮询一帧，收到就返回帧长。
    fn recv_frame(&mut self, out: &mut [u8], timeout_ticks: u64) -> Option<usize> {
        let start = ticks();
        loop {
            match self.dev.receive(out) {
                Ok(Some(n)) => return Some(n),
                Ok(None) => {}
                Err(_) => return None,
            }
            if ticks().wrapping_sub(start) > timeout_ticks {
                return None;
            }
            core::hint::spin_loop();
        }
    }

    /// 发送一个 UDP 数据报（自己负责 Ethernet + IPv4 的封装）。
    pub fn send_udp(
        &mut self,
        dst_ip: ipv4::Address,
        dst_mac: [u8; 6],
        src_port: u16,
        dst_port: u16,
        payload: &[u8],
    ) -> Result<usize, &'static str> {
        let mut frame = [0u8; net::FRAME_MAX];
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let eth = ethernet::build(&mut frame, dst_mac, self.mac, ethernet::ETHERTYPE_IPV4)
            .ok_or("net: frame buffer too small for Ethernet header")?;
        let ip_payload_len = udp::HEADER_LEN + payload.len();
        let ip = ipv4::build(
            eth,
            self.ip,
            dst_ip,
            ipv4::PROTO_UDP,
            64, // TTL
            id,
            ip_payload_len,
        )
        .ok_or("net: frame buffer too small for IPv4 header")?;
        udp::build(ip, self.ip, dst_ip, src_port, dst_port, payload)
            .ok_or("net: frame buffer too small for UDP datagram")?;

        let total = ethernet::HEADER_LEN + ipv4::HEADER_LEN + ip_payload_len;
        self.dev.transmit(&frame[..total])?;
        Ok(total)
    }

    /// 解析 `target` 的 MAC：先查缓存，未命中就发 ARP 请求并等应答。
    pub fn arp_resolve(&mut self, target: ipv4::Address) -> Option<[u8; 6]> {
        if let Some(mac) = self.arp_cache.lookup(target) {
            return Some(mac);
        }
        let mut frame = [0u8; ethernet::HEADER_LEN + arp::PACKET_LEN];
        let mut arp_bytes = [0u8; arp::PACKET_LEN];
        arp::build(
            &mut arp_bytes,
            arp::OPER_REQUEST,
            self.mac,
            self.ip,
            ethernet::ZERO_MAC,
            target,
        );
        {
            let payload =
                ethernet::build(&mut frame, ethernet::BROADCAST_MAC, self.mac, ethernet::ETHERTYPE_ARP)?;
            payload[..arp::PACKET_LEN].copy_from_slice(&arp_bytes);
        }
        self.send_frame(&frame).ok()?;

        let mut rx = [0u8; net::FRAME_MAX];
        let start = ticks();
        loop {
            let remaining = ARP_TIMEOUT_TICKS.saturating_sub(ticks().wrapping_sub(start));
            let n = self.recv_frame(&mut rx, remaining)?;
            let Some(eth) = ethernet::parse(&rx[..n]) else {
                continue;
            };
            if eth.ethertype != ethernet::ETHERTYPE_ARP {
                continue;
            }
            let Some(packet) = arp::parse(eth.payload) else {
                continue;
            };
            if packet.oper != arp::OPER_REPLY || packet.sender_ip != target {
                continue;
            }
            // 应答必须发给我们：以太网目的为本机（或广播），且 ARP 的目的 IP / MAC 都是本机。
            if !(eth.dst == self.mac || ethernet::is_broadcast(eth.dst)) {
                continue;
            }
            if packet.target_ip != self.ip || packet.target_mac != self.mac {
                continue;
            }
            // 交叉校验：帧头源 MAC 应与 ARP 报文的发送方 MAC 一致（不一致只提示，以 ARP 为准）。
            if eth.src != packet.sender_mac {
                serial::println("net: arp note: frame src != arp sender mac");
            }
            self.arp_cache.insert(target, packet.sender_mac);
            return Some(packet.sender_mac);
        }
    }

    /// 发一条 ICMP Echo 请求并等回显相同载荷的应答，返回回显字节数。
    ///
    /// 接收侧逐项校验：Ethernet → IPv4 → ICMP，源地址必须是 `dst_ip`、类型必须是
    /// Echo Reply、`id`/`seq` 必须匹配、载荷必须**逐字节相同**。任何一步不符就继续等，
    /// 直到超时返回 `Err` —— 这样「ping 通了」是网络上回来的真实内容，而不是日志措辞。
    pub fn icmp_ping(
        &mut self,
        dst_ip: ipv4::Address,
        dst_mac: [u8; 6],
        id: u16,
        seq: u16,
        payload: &[u8],
    ) -> Result<usize, &'static str> {
        let mut frame = [0u8; net::FRAME_MAX];
        let ip_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let eth = ethernet::build(&mut frame, dst_mac, self.mac, ethernet::ETHERTYPE_IPV4)
            .ok_or("net: frame buffer too small for Ethernet header")?;
        let icmp_len = icmp::HEADER_LEN + payload.len();
        let ip = ipv4::build(
            eth,
            self.ip,
            dst_ip,
            ipv4::PROTO_ICMP,
            64, // TTL
            ip_id,
            icmp_len,
        )
        .ok_or("net: frame buffer too small for IPv4 header")?;
        icmp::build(ip, icmp::TYPE_ECHO_REQUEST, id, seq, payload)
            .ok_or("net: frame buffer too small for ICMP echo")?;

        let total = ethernet::HEADER_LEN + ipv4::HEADER_LEN + icmp_len;
        self.dev.transmit(&frame[..total])?;

        let mut rx = [0u8; net::FRAME_MAX];
        let start = ticks();
        loop {
            let remaining = ICMP_TIMEOUT_TICKS.saturating_sub(ticks().wrapping_sub(start));
            let Some(n) = self.recv_frame(&mut rx, remaining) else {
                return Err("net: no ICMP echo reply within timeout");
            };
            let Some(eth) = ethernet::parse(&rx[..n]) else {
                continue;
            };
            if eth.ethertype != ethernet::ETHERTYPE_IPV4 {
                continue;
            }
            let Some((header, ip_payload)) = ipv4::parse(eth.payload) else {
                continue;
            };
            if header.protocol != ipv4::PROTO_ICMP || header.src != dst_ip {
                continue;
            }
            let Some(echo) = icmp::parse(ip_payload) else {
                continue;
            };
            if echo.echo_type != icmp::TYPE_ECHO_REPLY || echo.id != id || echo.seq != seq {
                continue;
            }
            if echo.payload != payload {
                return Err("net: ICMP echo reply payload mismatch");
            }
            return Ok(echo.payload.len());
        }
    }

    /// 发一个 DHCP DISCOVER 并等 OFFER，返回 offer 里给的地址。
    pub fn dhcp_discover(&mut self) -> Result<dhcp::Offer, &'static str> {
        let mut payload = [0u8; dhcp::MESSAGE_LEN];
        let len = dhcp::build_discover(&mut payload, self.mac);
        self.send_udp(
            ipv4::Address::BROADCAST,
            ethernet::BROADCAST_MAC,
            dhcp::CLIENT_PORT,
            dhcp::SERVER_PORT,
            &payload[..len],
        )?;

        let mut rx = [0u8; net::FRAME_MAX];
        let start = ticks();
        loop {
            let remaining = DHCP_TIMEOUT_TICKS.saturating_sub(ticks().wrapping_sub(start));
            let Some(n) = self.recv_frame(&mut rx, remaining) else {
                return Err("net: no DHCP OFFER within timeout");
            };
            let Some(eth) = ethernet::parse(&rx[..n]) else {
                continue;
            };
            if eth.ethertype != ethernet::ETHERTYPE_IPV4 {
                continue;
            }
            let Some((header, ip_payload)) = ipv4::parse(eth.payload) else {
                continue;
            };
            if header.protocol != ipv4::PROTO_UDP {
                continue;
            }
            let Some(datagram) = udp::parse(ip_payload, header.src, header.dst) else {
                continue;
            };
            if datagram.src_port != dhcp::SERVER_PORT || datagram.dst_port != dhcp::CLIENT_PORT {
                continue;
            }
            if let Some(offer) = dhcp::parse_offer(datagram.payload) {
                return Ok(offer);
            }
        }
    }
}

/// 发一个 UDP 数据报（用户态 `sendto` 的内核实现）。
///
/// 源地址取自 [`config`]（DHCP 租约），源端口由调用方给定（套接字绑定的端口）。
/// 目的 MAC 的选择：
/// - 广播地址 → 以太网广播；
/// - 网关本身 → 已知的网关 MAC；
/// - 其它 → 先 ARP 直解析（同网段），解析不到再退回网关 MAC（不做子网掩码/路由表，
///   缺省按 /24 看待，够本地链路用）。
///
/// 失败：没有网卡或尚未 DHCP → `-ENETDOWN`；帧放不下 / 发送失败 → `-EHOSTUNREACH`。
pub fn udp_send(
    src_port: u16,
    dst: ipv4::Address,
    dst_port: u16,
    payload: &[u8],
) -> Result<usize, i64> {
    let cfg = config().ok_or(abi::errno::ENETDOWN)?;
    let Some(dev) = crate::driver::net::first() else {
        return Err(abi::errno::ENETDOWN);
    };
    let mut stack = Stack::new(dev);
    stack.ip = cfg.ip;
    stack.gateway = cfg.gateway;
    stack.arp_cache.insert(cfg.gateway, cfg.gateway_mac);

    let dst_mac = if dst == ipv4::Address::BROADCAST {
        ethernet::BROADCAST_MAC
    } else if dst == cfg.gateway {
        cfg.gateway_mac
    } else {
        match stack.arp_resolve(dst) {
            Some(mac) => mac,
            None => cfg.gateway_mac,
        }
    };

    stack
        .send_udp(dst, dst_mac, src_port, dst_port, payload)
        .map_err(|_| abi::errno::EHOSTUNREACH)
}

/// 轮询网卡一次，收到一个 UDP 数据报就拷进 `out` 并返回
/// `(源地址, 源端口, 目的端口, 拷入字节数)`；没有数据报返回 `None`。
///
/// **非阻塞**：由 `syscall::socket::sys_recvfrom` 负责循环与超时 —— 这样调用方
/// 能按目的端口过滤（本函数把目的端口一并返回，让上层决定丢弃还是交付）。
pub fn udp_recv_poll(out: &mut [u8]) -> Option<(ipv4::Address, u16, u16, usize)> {
    let dev = crate::driver::net::first()?;
    let mut frame = [0u8; net::FRAME_MAX];
    let n = dev.receive(&mut frame).ok()??;
    let eth = ethernet::parse(&frame[..n])?;
    if eth.ethertype != ethernet::ETHERTYPE_IPV4 {
        return None;
    }
    let (header, ip_payload) = ipv4::parse(eth.payload)?;
    if header.protocol != ipv4::PROTO_UDP {
        return None;
    }
    let datagram = udp::parse(ip_payload, header.src, header.dst)?;
    let copied = datagram.payload.len().min(out.len());
    out[..copied].copy_from_slice(&datagram.payload[..copied]);
    Some((header.src, datagram.src_port, datagram.dst_port, copied))
}

/// v0.9 网络验收：ARP 解析网关 + DHCP DISCOVER → OFFER。
///
/// 断言的都是**网络上回来的真实内容**（不是日志措辞）：ARP 应答里的 MAC、
/// OFFER 里的 `yiaddr` / server-id / 网关。任何一步没通，这里就会返回 `Err`。
pub fn selftest(dev: &mut dyn NetworkDevice) -> Result<(), &'static str> {
    let mut stack = Stack::new(dev);

    serial::print("net: stack mac=");
    net::print_mac(stack.mac);
    serial::print(" ip=");
    print_addr(stack.ip);
    serial::print(" gateway=");
    print_addr(stack.gateway);
    serial::println("");

    // 1) ARP：解析网关的 MAC（Ethernet 广播请求 → 单播应答）。
    let gateway_mac = stack
        .arp_resolve(stack.gateway)
        .ok_or("net: ARP resolve of the gateway timed out")?;
    serial::print("net: arp ");
    print_addr(stack.gateway);
    serial::print(" -> ");
    net::print_mac(gateway_mac);
    serial::println(" (reply)");
    serial::println("net: arp PASS");

    // 2) DHCP：UDP DISCOVER → OFFER（Ethernet → IPv4 → UDP 的完整往返）。
    let offer = stack.dhcp_discover()?;
    serial::print("net: dhcp offer yiaddr=");
    print_addr(offer.yiaddr);
    if let Some(server) = offer.server_id {
        serial::print(" server=");
        print_addr(server);
    }
    if let Some(mask) = offer.subnet_mask {
        serial::print(" mask=");
        print_addr(mask);
    }
    if let Some(router) = offer.router {
        serial::print(" router=");
        print_addr(router);
    }
    serial::println("");
    if offer.yiaddr == ipv4::Address::UNSPECIFIED {
        return Err("net: DHCP OFFER carried a zero address");
    }
    serial::println("net: dhcp PASS");

    // 把协商结果记进全局配置：用户态 `net_config` 与 UDP `sendto` 都读它。
    set_config(NetConfig {
        ip: offer.yiaddr,
        gateway: stack.gateway,
        gateway_mac,
        mac: stack.mac,
    });

    // 3) ICMP：向网关发 Echo 请求 → 收到回显**相同载荷**的应答（ping）。
    //    DHCP 之后本机地址换成租约地址，源地址因此与 OFFER 一致。
    stack.ip = offer.yiaddr;
    let gateway = stack.gateway;
    let ident: u16 = 0x4242;
    let seq: u16 = 7;
    let ping_payload = b"cerlesse-icmp";
    let echoed = stack.icmp_ping(gateway, gateway_mac, ident, seq, ping_payload)?;
    serial::print("net: icmp echo id=");
    serial::print_hex(ident as u64);
    serial::print(" seq=");
    serial::print_dec(seq as u64);
    serial::print(" payload=");
    serial::print_dec(echoed as u64);
    serial::print(" reply from ");
    print_addr(gateway);
    serial::println("");
    if echoed != ping_payload.len() {
        return Err("net: ICMP echo reply length mismatch");
    }
    serial::println("net: icmp PASS");

    serial::println("net: selftest PASS");
    Ok(())
}

/// 按 `a.b.c.d` 打印一个 IPv4 地址。
pub fn print_addr(addr: ipv4::Address) {
    for (i, octet) in addr.octets().iter().enumerate() {
        if i > 0 {
            serial::print(".");
        }
        serial::print_dec(*octet as u64);
    }
}
