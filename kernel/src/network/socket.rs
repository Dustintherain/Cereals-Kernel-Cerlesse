//! UDP 套接字表（v1.0 实现；面向用户态的 `socket`/`bind`/`sendto`/`recvfrom`）
//!
//! v0.9 只在**内核内部**验证网络路径（ARP + DHCP 的 UDP 往返，见
//! [`crate::network::selftest`]）。v1.0 把这条路径接到用户态：本模块提供一张
//! **固定容量**的套接字表，fd 表（[`crate::fs::file::Socket`]）里存的就是这里的
//! 槽位 id —— 编号空间、生命周期与文件 fd 合一（Linux 如此）。
//!
//! ## 这一版做什么、不做什么
//!
//! - **只做 UDP**（`SOCK_DGRAM`）：套接字 = 一个本地端口（别无状态）；
//! - `sendto`/`recvfrom` 用显式地址，没有 `connect`/`listen`/`accept`；
//! - 收信是**同步轮询**：`recvfrom`（[`crate::syscall::socket`]）在内核里轮询网卡，
//!   收到**目的端口等于本套接字端口**的数据报就交付。**没有等待队列** —— 目的端口
//!   不匹配的数据报、以及本套接字来不及取走时到达的数据报都被丢弃，
//!   这是与 [`crate::driver::net`] 的「非阻塞轮询、无中断」同一取舍。
//!
//! 表用固定数组 + `static mut`：与 [`crate::fs::file`] 的 fd 表同一种收口
//!（一次只跑一个用户程序，内核在 syscall 里等待，不需要每进程/并发保护）。


/// 同时存在的套接字上限（发送/接收方各留几个，够一个最小程序用）。
pub const SOCK_MAX: usize = 8;
/// 单槽收信邮箱的容量（一个经典以太网 UDP 载荷）。
pub const RX_MAX: usize = 1472;
/// 临时端口范围起点（IANA 动态端口）。
const EPHEMERAL_FIRST: u16 = 49152;
/// 临时端口范围终点（含）。
const EPHEMERAL_LAST: u16 = 65535;

/// 一个套接字槽位。
///
/// `Copy` + `const EMPTY` 让整张表能在静态量里 `const` 初始化（与 fd 表的
/// `[const { None }; N]` 同一目的）。
#[derive(Clone, Copy)]
struct State {
    /// 槽位是否在用
    in_use: bool,
    /// 本地端口（0 = 尚未 `bind`）
    port: u16,
}

impl State {
    /// 空槽位（`const`，供静态数组初始化）。
    const EMPTY: State = State {
        in_use: false,
        port: 0,
    };
}

// 全局套接字表：一次只跑一个用户程序（见模块头注释）。
static mut SOCKETS: [State; SOCK_MAX] = [State::EMPTY; SOCK_MAX];

/// 以闭包访问套接字表。
fn with_table<T>(f: impl FnOnce(&mut [State; SOCK_MAX]) -> T) -> T {
    unsafe {
        let table = (&raw mut SOCKETS).as_mut().expect("SOCKETS 是静态量");
        f(table)
    }
}

/// 按 id 取槽位（`id` 越界或未在用返回 `None`）。
fn get(id: u16) -> Option<&'static mut State> {
    if id as usize >= SOCK_MAX {
        return None;
    }
    unsafe {
        let table = (&raw mut SOCKETS).as_mut()?;
        let slot = table.get_mut(id as usize)?;
        if slot.in_use {
            Some(slot)
        } else {
            None
        }
    }
}

/// 创建一个套接字，返回槽位 id（表满返回 `None`）。
pub fn create() -> Option<u16> {
    with_table(|table| {
        for (i, slot) in table.iter_mut().enumerate() {
            if !slot.in_use {
                *slot = State::EMPTY;
                slot.in_use = true;
                return Some(i as u16);
            }
        }
        None
    })
}

/// 释放一个套接字（`close` 时调用；未知 id 忽略）。
pub fn release(id: u16) {
    if let Some(slot) = get(id) {
        *slot = State::EMPTY;
    }
}

/// 清空整张表（进入用户态前调用，保证上一个程序的套接字不泄漏给下一个）。
pub fn reset() {
    with_table(|table| {
        for slot in table.iter_mut() {
            *slot = State::EMPTY;
        }
    });
}

/// 绑定本地端口：`port = 0` 分配一个空闲临时端口，否则要求该端口未被占用。
///
/// 成功返回实际端口；端口被占 `-EADDRINUSE`，临时端口耗尽 `-EADDRINUSE`（同一个）。
pub fn bind(id: u16, port: u16) -> Result<u16, i64> {
    let taken = |p: u16| -> bool {
        with_table(|table| table.iter().any(|s| s.in_use && s.port == p))
    };
    let chosen = if port == 0 {
        let mut free = None;
        with_table(|table| {
            for candidate in EPHEMERAL_FIRST..=EPHEMERAL_LAST {
                if !table.iter().any(|s| s.in_use && s.port == candidate) {
                    free = Some(candidate);
                    break;
                }
            }
        });
        free.ok_or(abi::errno::EADDRINUSE)?
    } else {
        if taken(port) {
            return Err(abi::errno::EADDRINUSE);
        }
        port
    };
    match get(id) {
        Some(slot) => {
            slot.port = chosen;
            Ok(chosen)
        }
        None => Err(abi::errno::EBADF),
    }
}

/// 套接字的本地端口（未绑定返回 `None`）。
pub fn port_of(id: u16) -> Option<u16> {
    get(id).map(|slot| slot.port).filter(|p| *p != 0)
}
