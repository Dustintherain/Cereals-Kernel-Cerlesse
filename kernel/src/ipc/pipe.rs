//! 单向管道 IPC（v0.5 实现骨架）
//!
//! 第一版只做「可验证骨架」（docs/process.md 第 2.6 节）：
//! - 固定容量的环形缓冲，一端写、一端读（单向）；
//! - 非阻塞语义：写满时只写入剩余空间（返回实际写入字节数），
//!   缓冲为空时读返回 0（EAGAIN 风格雏形，完整阻塞调度闭环留待后续）；
//! - 每次创建 / 写入 / 读取都在串口打印一行摘要，供集成测试断言。
//!
//! 第一版在 pid 0（启动上下文）中同步调用，不涉及跨任务并发；
//! 后续接入进程/线程并发时再补同步原语。
//!
//! 关联：docs/process.md 第 2.6 节、DEVELOPMENT.md v0.5 IPC 验收项。

use core::ptr::addr_of_mut;

use crate::driver::serial;

/// 第一版管道表容量。
pub const PIPE_COUNT: usize = 4;
/// 单条管道环形缓冲容量（字节）。
pub const PIPE_CAP: usize = 64;

/// 单条管道：`used` 标记槽位占用，`head/len` 描述环形缓冲中的有效区间。
struct Pipe {
    used: bool,
    id: u64,
    buf: [u8; PIPE_CAP],
    head: usize,
    len: usize,
}

impl Pipe {
    const EMPTY: Pipe = Pipe {
        used: false,
        id: 0,
        buf: [0; PIPE_CAP],
        head: 0,
        len: 0,
    };
}

/// 管道表（.bss）。
static mut PIPES: [Pipe; PIPE_COUNT] = [Pipe::EMPTY; PIPE_COUNT];

#[inline]
fn pipe_ptr(index: usize) -> *mut Pipe {
    unsafe { addr_of_mut!(PIPES).add(index) as *mut Pipe }
}

/// 按 id（1-based 槽位号）取管道裸指针；越界或未占用返回空指针。
fn find(id: u64) -> *mut Pipe {
    if id == 0 || id > PIPE_COUNT as u64 {
        return core::ptr::null_mut();
    }
    let p = pipe_ptr(id as usize - 1);
    if unsafe { (*p).used } {
        p
    } else {
        core::ptr::null_mut()
    }
}

/// 创建一条单向管道，成功返回管道 id，并打印 `pipe: create id=<n> cap=<c>`。
pub fn create() -> Option<u64> {
    unsafe {
        for index in 0..PIPE_COUNT {
            let p = pipe_ptr(index);
            if !(*p).used {
                *p = Pipe::EMPTY;
                (*p).used = true;
                (*p).id = index as u64 + 1;
                let id = (*p).id;
                log(|| {
                    serial::print("pipe: create id=");
                    serial::print_dec(id);
                    serial::print(" cap=");
                    serial::print_dec(PIPE_CAP as u64);
                    serial::println("");
                });
                return Some(id);
            }
        }
    }
    None
}

/// 从写端写入，返回实际写入的字节数（缓冲剩余空间不足时短写）。
/// 打印 `pipe: write id=<n> len=<w>`。
pub fn write(id: u64, data: &[u8]) -> usize {
    let p = find(id);
    if p.is_null() || data.is_empty() {
        return 0;
    }
    unsafe {
        let pipe = &mut *p;
        let free = PIPE_CAP - pipe.len;
        let n = if data.len() < free { data.len() } else { free };
        let start = (pipe.head + pipe.len) % PIPE_CAP;
        for (i, &b) in data[..n].iter().enumerate() {
            pipe.buf[(start + i) % PIPE_CAP] = b;
        }
        pipe.len += n;

        let written = n;
        let pid = pipe.id;
        log(|| {
            serial::print("pipe: write id=");
            serial::print_dec(pid);
            serial::print(" len=");
            serial::print_dec(written as u64);
            serial::println("");
        });
        n
    }
}

/// 从读端读取最多 `out.len()` 字节，返回实际读取数（缓冲为空时返回 0）。
/// 打印 `pipe: read id=<n> len=<r>`。
pub fn read(id: u64, out: &mut [u8]) -> usize {
    let p = find(id);
    if p.is_null() || out.is_empty() {
        return 0;
    }
    unsafe {
        let pipe = &mut *p;
        let n = if out.len() < pipe.len { out.len() } else { pipe.len };
        for i in 0..n {
            out[i] = pipe.buf[(pipe.head + i) % PIPE_CAP];
        }
        pipe.head = (pipe.head + n) % PIPE_CAP;
        pipe.len -= n;

        let got = n;
        let pid = pipe.id;
        log(|| {
            serial::print("pipe: read id=");
            serial::print_dec(pid);
            serial::print(" len=");
            serial::print_dec(got as u64);
            serial::println("");
        });
        n
    }
}

/// v0.5 IPC 验收自测：创建管道 → 写入 → 读回比对 → 空读返回 0。
///
/// 由 `kernel_main` 在 Round Robin 巡回完成之后调用，
/// 顺带验证管道骨架不给调度器引入新崩溃面。
pub fn selftest() -> Result<(), &'static str> {
    const MSG: &[u8] = b"hello v0.5";

    let id = create().ok_or("pipe table full")?;

    let written = write(id, MSG);
    if written != MSG.len() {
        return Err("short write");
    }

    let mut out = [0u8; 16];
    let got = read(id, &mut out);
    if got != MSG.len() || &out[..got] != MSG {
        return Err("readback mismatch");
    }

    let empty = read(id, &mut out);
    if empty != 0 {
        return Err("empty read != 0");
    }

    Ok(())
}

/// 在串口重入保护下输出一行日志；保护被占用时跳过本次输出
/// （与 scheduler 的日志约定一致，避免与 IRQ0 心跳输出穿插）。
fn log(f: impl FnOnce()) {
    unsafe {
        if crate::interrupt::irq::serial_debug_enter() {
            f();
            crate::interrupt::irq::serial_debug_exit();
        }
    }
}
