//! PID 分配与回收（v0.5/v0.6）

#![allow(dead_code)]

/// PID 上限（超过后回绕）。
pub const PID_MAX: u64 = 4096;

/// 下一个待分配的 PID。0 号保留给启动上下文（`kernel_main`）。
static mut NEXT_PID: u64 = 0;

/// 分配一个 PID。首次调用返回 0（启动上下文），随后为 1、2、3……
pub fn alloc() -> u64 {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID = if NEXT_PID + 1 >= PID_MAX { 1 } else { NEXT_PID + 1 };
        pid
    }
}

/// 重置分配器（仅供自测/重启路径调用）。
pub fn reset() {
    unsafe { NEXT_PID = 0 }
}

/// 当前运行线程的 PID。v0.6 中内核线程与轻量进程一一对应，调度器在
/// 调度切换时把 `cur_pid` 置回；若无当前任务，返回 0。
pub fn current_task() -> u64 {
    unsafe {
        if !crate::process::thread::CURRENT_THREAD.is_null() {
            let t = &*crate::process::thread::CURRENT_THREAD;
            t.pid
        } else {
            0
        }
    }
}

/// 返回当前运行任务的 PID。若无当前任务，返回 `None`。
pub fn current() -> Option<u64> {
    Some(current_task())
}
