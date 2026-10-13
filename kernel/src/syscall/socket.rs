//! Socket 系统调用（v1.0 实现：`socket` / `bind` / `sendto` / `recvfrom`）
//!
//! 面向用户态的 UDP 最小集（`AF_INET` + `SOCK_DGRAM`）。fd 表（[`crate::fs::file`]）
//! 把套接字与文件放在同一个编号空间：`socket` 返回一个普通 fd，`close` 对两者都有效。
//!
//! ## ABI 形状
//!
//! 号沿用 Linux x86_64（[`abi::syscall`]）：`socket=41`、`sendto=44`、
//! `recvfrom=45`、`bind=49`；`net_config=267` 是 Cerlesse 扩展号，用来导出
//! DHCP 协商到的本机地址/网关/MAC（记录布局见 [`abi::socket`]）。
//!
//! 地址用 16 字节 `sockaddr_in`（**网络字节序**的端口与地址），内核直接读那些
//! 字节，不做二次转换。用户缓冲沿用与文件 syscall 同一套**按页校验**：
//! 发出去的要求可读、收进来的要求可写，越界一律 `-EFAULT` 并留串口记录。
//!
//! ## 这一版的行为边界
//!
//! | 调用 | 成功 | 明确拒绝 |
//! | ---- | ---- | ---- |
//! | `socket` | `AF_INET`+`SOCK_DGRAM` → fd | 别的族/类型 → `-EAFNOSUPPORT`/`-EPROTONOSUPPORT` |
//! | `bind` | `0`，本地端口记在套接字上 | 端口被占 → `-EADDRINUSE`；外机地址 → `-EADDRNOTAVAIL` |
//! | `sendto` | 发送字节数（含 0 长度数据报） | 非 0 `flags` → `-EOPNOTSUPP`；超长 → `-EMSGSIZE`；无网卡/未 DHCP → `-ENETDOWN` |
//! | `recvfrom` | 收下并回填发送方地址 | 未 `bind` → `-EINVAL`；超时 → `-EAGAIN` |
//!
//! `read`/`write` 一个 UDP 套接字返回 `-ENOTCONN`（没有 `connect`，无从定位对端），
//! 这不是缺陷而是取舍：标准写法用 `sendto`/`recvfrom`。
//!
//! ## 等待模型
//!
//! `recvfrom` 在内核里**同步轮询**网卡：收到**目的端口等于本套接字端口**的数据报就
//! 交付，否则丢弃。等待期间用 `sti; hlt` 让 PIT 推进（与 `read_stdin`/`nanosleep`
//! 同一手法），超过 [`RECV_TIMEOUT_TICKS`] 返回 `-EAGAIN` —— 不无限等是 v0.9 以来
//! 的既有取舍（一次只跑一个程序、内核在 syscall 里等）。

use crate::driver::serial;
use crate::fs::file as fdtable;
use crate::network::{self, socket as socktab};
use crate::network::ipv4::Address;
use crate::syscall::address_space::UserSpace;
use crate::syscall::file::{user_bytes, user_bytes_mut};
use crate::time::clock;

use abi::errno::{
    EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EBADF, EINVAL, EMSGSIZE, ENETDOWN, ENODEV, ENOSPC,
    ENOTSOCK, EOPNOTSUPP, EPROTONOSUPPORT,
};
use abi::socket::{
    AF_INET, IPPROTO_IP, IPPROTO_UDP, NET_CONFIG_GATEWAY_OFFSET, NET_CONFIG_IP_OFFSET,
    NET_CONFIG_MAC_OFFSET, NET_CONFIG_SIZE, SOCKADDR_IN_SIZE, SOCK_DGRAM, SOCK_FLAGS,
    SOCK_TYPE_MASK, UDP_PAYLOAD_MAX,
};

/// `recvfrom` 等待一个数据报的上限（PIT 100Hz → 200 tick = 2 秒）。
///
/// 与 `read_stdin` 的 `STDIN_IDLE_TICKS` 同一理由：本内核单 CPU、用户态无抢占，
/// 无限等会整机卡死；超时回 `-EAGAIN` 交给用户程序决定。
const RECV_TIMEOUT_TICKS: u64 = 200;

/// 解析用户态传来的 `sockaddr_in`：只认 `AF_INET`，返回 `(地址, 端口)`。
///
/// 地址缓冲要求**可读**且至少 [`SOCKADDR_IN_SIZE`] 字节；`sin_zero` 不校验
/// （POSIX 不要求它有意义）。
unsafe fn read_sockaddr(
    space: *const UserSpace,
    ptr: u64,
    len: u64,
    op: &str,
) -> Result<(Address, u16), i64> {
    if ptr == 0 || len < SOCKADDR_IN_SIZE as u64 {
        return Err(EINVAL);
    }
    let bytes = unsafe { user_bytes(space, ptr, SOCKADDR_IN_SIZE as u64, op)? };
    let family = u16::from_le_bytes([bytes[0], bytes[1]]);
    if family != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let port = u16::from_be_bytes([bytes[2], bytes[3]]);
    let addr = Address::from_octets([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Ok((addr, port))
}

/// 把发送方地址写回用户的 `sockaddr_in`，并更新 `*addrlen`（`socklen_t` = u32）。
///
/// `addr_ptr == 0` 表示调用方不要地址（此时 `addrlen_ptr` 也忽略）。
unsafe fn write_source_addr(
    space: *const UserSpace,
    addr_ptr: u64,
    addrlen_ptr: u64,
    src: Address,
    src_port: u16,
) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    let out = unsafe { user_bytes_mut(space, addr_ptr, SOCKADDR_IN_SIZE as u64, "recvfrom")? };
    out[0..2].copy_from_slice(&AF_INET.to_le_bytes());
    out[2..4].copy_from_slice(&src_port.to_be_bytes());
    out[4..8].copy_from_slice(&src.octets());
    out[8..16].fill(0);
    if addrlen_ptr != 0 {
        let lenp = unsafe { user_bytes_mut(space, addrlen_ptr, 4, "recvfrom")? };
        // 输入是调用方的缓冲长度：回填的值不能超过它（POSIX 语义）。
        let capacity = u32::from_le_bytes([lenp[0], lenp[1], lenp[2], lenp[3]]) as usize;
        let actual = capacity.min(SOCKADDR_IN_SIZE) as u32;
        lenp[0..4].copy_from_slice(&actual.to_le_bytes());
    }
    Ok(())
}

/// 取 fd 对应的套接字 id；fd 有效但不是套接字返回 `-ENOTSOCK`。
fn socket_id(fd: i32) -> Result<u16, i64> {
    match fdtable::socket_of(fd) {
        Some(id) => Ok(id),
        None => Err(if fd < 0 { EBADF } else { ENOTSOCK }),
    }
}

/// 系统调用 `socket(domain, type, protocol)`：创建 UDP 套接字并分配 fd。
pub unsafe fn sys_socket(args: &[u64; 6], _space: *const UserSpace) -> i64 {
    let domain = args[0];
    let sotype = args[1] as u32;
    let protocol = args[2] as u32;

    if domain != AF_INET as u64 {
        serial::print("user: socket rejected domain=");
        serial::print_dec(domain);
        serial::println(" (only AF_INET)");
        return EAFNOSUPPORT;
    }
    if sotype & !(SOCK_TYPE_MASK | SOCK_FLAGS) != 0 {
        return EINVAL;
    }
    if sotype & SOCK_TYPE_MASK != SOCK_DGRAM {
        serial::println("user: socket rejected (only SOCK_DGRAM)");
        return EPROTONOSUPPORT;
    }
    if protocol != IPPROTO_IP && protocol != IPPROTO_UDP {
        return EPROTONOSUPPORT;
    }

    let Some(id) = socktab::create() else {
        return ENOSPC; // 套接字表满
    };
    match fdtable::open_socket(id) {
        Ok(fd) => {
            serial::print("user: socket domain=AF_INET type=SOCK_DGRAM fd=");
            serial::print_dec(fd as u64);
            serial::print(" id=");
            serial::print_dec(id as u64);
            serial::println("");
            fd as i64
        }
        Err(errno) => {
            socktab::release(id);
            errno
        }
    }
}

/// 系统调用 `bind(fd, addr_ptr, addr_len)`：绑定本地地址/端口。
pub unsafe fn sys_bind(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let fd = args[0] as i32;
    let id = match socket_id(fd) {
        Ok(id) => id,
        Err(errno) => return errno,
    };
    let (addr, port) = match unsafe { read_sockaddr(space, args[1], args[2], "bind") } {
        Ok(v) => v,
        Err(errno) => return errno,
    };
    // 只接受「任意地址」或本机租约地址（没有子网/多宿主概念，别的地址明确拒绝）。
    if addr != Address::UNSPECIFIED {
        match network::config().map(|c| c.ip) {
            Some(ours) if ours == addr => {}
            _ => return EADDRNOTAVAIL,
        }
    }
    match socktab::bind(id, port) {
        Ok(local) => {
            serial::print("user: bind fd=");
            serial::print_dec(fd as u64);
            serial::print(" port=");
            serial::print_dec(local as u64);
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: bind fd=");
            serial::print_dec(fd as u64);
            serial::print(" port=");
            serial::print_dec(port as u64);
            serial::print(" errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::println("");
            errno
        }
    }
}

/// 系统调用 `sendto(fd, buf, len, flags, addr_ptr, addr_len)`：发一个 UDP 数据报。
pub unsafe fn sys_sendto(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let fd = args[0] as i32;
    let buf = args[1];
    let len = args[2];
    let flags = args[3];
    let addr_ptr = args[4];
    let addr_len = args[5];

    let id = match socket_id(fd) {
        Ok(id) => id,
        Err(errno) => return errno,
    };
    if flags != 0 {
        return EOPNOTSUPP; // MSG_* 一律未实现：不静默忽略
    }
    if len > UDP_PAYLOAD_MAX as u64 {
        return EMSGSIZE;
    }
    let payload = match unsafe { user_bytes(space, buf, len, "sendto") } {
        Ok(p) => p,
        Err(errno) => return errno,
    };
    let (dst, dst_port) = match unsafe { read_sockaddr(space, addr_ptr, addr_len, "sendto") } {
        Ok(v) => v,
        Err(errno) => return errno,
    };
    // 没绑过端口就自动分配一个临时端口（Linux 的隐式 bind 语义）。
    let local = match socktab::port_of(id) {
        Some(p) => p,
        None => match socktab::bind(id, 0) {
            Ok(p) => p,
            Err(errno) => return errno,
        },
    };

    match network::udp_send(local, dst, dst_port, payload) {
        // POSIX：`sendto` 返回**载荷字节数**（不是整帧长度）。
        Ok(frame_bytes) => {
            serial::print("user: sendto fd=");
            serial::print_dec(fd as u64);
            serial::print(" sport=");
            serial::print_dec(local as u64);
            serial::print(" dst=");
            network::print_addr(dst);
            serial::print(":");
            serial::print_dec(dst_port as u64);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" frame=");
            serial::print_dec(frame_bytes as u64);
            serial::println("");
            len as i64
        }
        Err(errno) => {
            serial::print("user: sendto fd=");
            serial::print_dec(fd as u64);
            serial::print(" errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::println(if errno == ENETDOWN {
                " (network down)"
            } else {
                ""
            });
            errno
        }
    }
}

/// 系统调用 `recvfrom(fd, buf, len, flags, addr_ptr, addr_len_ptr)`：收一个 UDP 数据报。
pub unsafe fn sys_recvfrom(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let fd = args[0] as i32;
    let buf = args[1];
    let len = args[2];
    let flags = args[3];
    let addr_ptr = args[4];
    let addrlen_ptr = args[5];

    let id = match socket_id(fd) {
        Ok(id) => id,
        Err(errno) => return errno,
    };
    if flags != 0 {
        return EOPNOTSUPP;
    }
    let Some(port) = socktab::port_of(id) else {
        return EINVAL; // 未绑定：不知道收哪个端口的数据报
    };
    let dst = match unsafe { user_bytes_mut(space, buf, len, "recvfrom") } {
        Ok(d) => d,
        Err(errno) => return errno,
    };

    let deadline = unsafe { clock::tick_count() } + RECV_TIMEOUT_TICKS;
    loop {
        let mut tmp = [0u8; socktab::RX_MAX];
        if let Some((src, src_port, dst_port, n)) = network::udp_recv_poll(&mut tmp) {
            if dst_port == port {
                let copy = n.min(dst.len());
                dst[..copy].copy_from_slice(&tmp[..copy]);
                if let Err(errno) =
                    unsafe { write_source_addr(space, addr_ptr, addrlen_ptr, src, src_port) }
                {
                    return errno;
                }
                unsafe {
                    core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
                }
                log_recv(fd, src, src_port, copy);
                return copy as i64;
            }
            // 目的端口不是本套接字的：丢弃（没有等待队列，见模块头注释）。
        }
        if unsafe { clock::tick_count() } >= deadline {
            serial::print("user: recvfrom fd=");
            serial::print_dec(fd as u64);
            serial::print(" port=");
            serial::print_dec(port as u64);
            serial::println(" idle (no datagram within timeout)");
            return EAGAIN;
        }
        // 等待期间打开中断：PIT 推进 tick，网卡由轮询收（见模块头注释）。
        unsafe {
            core::arch::asm!("sti", "hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// `user: recvfrom fd=<n> src=<a.b.c.d>:<p> len=<n>`。
fn log_recv(fd: i32, src: Address, src_port: u16, n: usize) {
    serial::print("user: recvfrom fd=");
    serial::print_dec(fd as u64);
    serial::print(" src=");
    network::print_addr(src);
    serial::print(":");
    serial::print_dec(src_port as u64);
    serial::print(" len=");
    serial::print_dec(n as u64);
    serial::println("");
}

/// 系统调用 `net_config(buf_ptr, buf_len)`（Cerlesse 扩展号 267）：导出网络配置。
///
/// 没有网卡（或 DHCP 尚未成功）返回 `-ENODEV` —— 用户态据此决定「跳过网络自测」。
/// 记录布局见 [`abi::socket`]：`ip(4) + gateway(4) + mac(6) + reserved(2)`。
pub unsafe fn sys_net_config(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let buf = args[0];
    let len = args[1];
    if len < NET_CONFIG_SIZE as u64 {
        return EINVAL;
    }
    let Some(cfg) = network::config() else {
        serial::println("user: net_config -> ENODEV (no network device / no DHCP lease)");
        return ENODEV;
    };
    let out = match unsafe { user_bytes_mut(space, buf, NET_CONFIG_SIZE as u64, "net_config") } {
        Ok(o) => o,
        Err(errno) => return errno,
    };
    out[NET_CONFIG_IP_OFFSET..NET_CONFIG_IP_OFFSET + 4].copy_from_slice(&cfg.ip.octets());
    out[NET_CONFIG_GATEWAY_OFFSET..NET_CONFIG_GATEWAY_OFFSET + 4]
        .copy_from_slice(&cfg.gateway.octets());
    out[NET_CONFIG_MAC_OFFSET..NET_CONFIG_MAC_OFFSET + 6].copy_from_slice(&cfg.mac);
    out[14..16].fill(0);
    serial::print("user: net_config ip=");
    network::print_addr(cfg.ip);
    serial::print(" gateway=");
    network::print_addr(cfg.gateway);
    serial::print(" mac=");
    crate::driver::net::print_mac(cfg.mac);
    serial::println("");
    NET_CONFIG_SIZE as i64
}
