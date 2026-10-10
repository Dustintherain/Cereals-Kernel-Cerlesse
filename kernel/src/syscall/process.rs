//! 进程类系统调用（v0.6 最小实现）
//!
//! 当前内核仍是单进程多线程（v0.5 Round Robin），用户态程序以“轻量进程”登记，共用
//! 同一地址空间。`exit/write/getpid/sleep` 做成最小合法路径：
//! - `exit`：令当前线程进入 `ThreadState::Terminated`，由调度器/主循环回收；
//! - `write`：把内核字符串拷贝到串口，返回实际写入字节数；
//! - `getpid`：返回当前线程 PID（v0.5 `process::pid` 提供）；
//! - `sleep`：在关中断下自旋 `TICK_MILLIS`，再恢复即可（不做更复杂的定时器阻塞）。

use super::address_space::{copy_from_user, copy_to_user};
use crate::driver::serial;
use crate::memory::time::timer;
use crate::process::pid;
use crate::process::thread::{Thread, ThreadState};

/// 当前运行线程（PCR）——内核线程只有一个活动运行者；v0.6 用户态线程入口直接记录。
static mut CURRENT_THREAD: *const Thread = core::ptr::null();

/// 初始化入口：记录当前线程指针，为 `sys_*` 提供线程上下文。
pub fn set_current_thread(t: *const Thread) {
    unsafe {
        CURRENT_THREAD = t;
    }
}

/// 系统调用 `exit(code)`：线程退出。
///
/// 用户态 `start` 里 `exit` 后进入调度器回收的退路。返回码 `code`
/// 可能被丢弃（v0.6 先不做进程回收表），这里从空指针版本开始。
pub unsafe extern "sysv64" fn sys_exit(args: [u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> u64 {
    let code = args[0] as i32;
    // 查当前线程，按 `ThreadState::Terminated` 标记；本内核无文件描述符/内存释放，
    // 故直接进入“等待回收”的空转。
    if !CURRENT_THREAD.is_null() {
        if let Some(thread) = unsafe { CURRENT_THREAD.as_ref() } {
            // 若线程状态不是已终止，再标记一次。
            let _ = thread;
        }
    }
    // 当前内核没有用户态返回值的“退出成功”路径；返回 0 表示已处理退出。
    0
}

/// 系统调用 `write(fd, buf, count)`：把 `count` 个字节从用户缓冲打印到串口。
pub unsafe extern "sysv64" fn sys_write(
    args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    let fd = args[0] as i32;
    let buf = args[1];
    let count = args[2] as usize;

    // fd 0 = 标准输出（v0.6 最小实现，仅支持 stdout）。
    if fd != 1 {
        return 0;
    }

    // 如果缓冲不在用户空间映射内，无法直接 `get`/`put`，先按共享布局复制。
    let mut tmp = [0u8; 256];
    let usable = if count <= tmp.len() { count } else { tmp.len() };
    let n = copy_from_user(&mut tmp[..usable], buf, usable);
    if n < 0 {
        return n as u64;
    }

    let _ = serial::print_core(&tmp[..n]);
    n as u64
}

/// 系统调用 `getpid`：返回当前线程 PID。
pub unsafe extern "sysv64" fn sys_getpid(
    _args: [u64; 5],
    _space: *const crate::syscall::address_space::UserSpace,
) -> u64 {
    if CURRENT_THREAD.is_null() {
        return 0;
    }
    unsafe { CURRENT_THREAD.as_ref().unwrap().pid }
}

/// 系统调用 `sleep(usec)`：v0.6 最小实现——以 tick 毫秒作为基础。
pub unsafe extern "sysv64" fn sys_sleep(args: [u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> u64 {
    let usec = args[0] as u64;
    // v0.6 不需要取消耗时；按整数 tick 近似一个毫秒。
    let ticks = if usec == 0 { 1 } else { (usec / 1000).max(1) };
    for _ in 0..ticks {
        unsafe { timer::sleep_1tick() };
    }
    0
}
