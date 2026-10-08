//! PID 分配与回收（v0.5 第一版）
//!
//! 第一版采用单调递增分配（不回收），因为内核任务表是定长的
//! （见 `scheduler::scheduler::MAX_TASKS`）。
//! 进程创建/退出导致的 PID 回收随 v0.6 的用户态进程模型一起补。

#![allow(dead_code)]

/// PID 上限（超过后回绕）。
pub const PID_MAX: u64 = 4096;

/// 下一个待分配的 PID。0 号保留给启动上下文（`kernel_main`）。
static mut NEXT_PID: u64 = 0;

/// 分配一个 PID。
///
/// 首次调用返回 0（启动上下文），随后为 1、2、3……超过 `PID_MAX` 后回绕到 1。
pub fn alloc() -> u64 {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID = if NEXT_PID + 1 >= PID_MAX { 1 } else { NEXT_PID + 1 };
        pid
    }
}

/// 重置分配器（仅供自测/重启路径调用）。
pub fn reset() {
    unsafe { NEXT_PID = 0 };
}

/// 已分配到的 PID 上界（调试用）。
pub fn next_hint() -> u64 {
    unsafe { NEXT_PID }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_max_is_reasonable() {
        assert!(PID_MAX > 1024);
    }
}
