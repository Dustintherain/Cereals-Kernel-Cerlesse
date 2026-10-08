//! 线程结构与状态机（v0.5）
//!
//! 一个 [`Thread`] 是调度器的最小单位：它持有上下文（寄存器 + 内核栈指针）、
//! 状态与调度记账信息。进程级资源（地址空间 / 文件表 / 信号）见 [`super::process`]。
//!
//! 状态机（第一版实际使用前四种；`Blocked`/`Terminated` 语义随 IPC 与进程退出补）：
//!
//! ```text
//! New ──spawn──▶ Ready ──scheduled──▶ Running
//!                  ▲                     │
//!                  └──── preempted ──────┤
//!                                        └── exit ──▶ Terminated
//!                  ▲                     │
//!                  └──── unblock ── Blocked
//! ```

#![allow(dead_code)]

use super::context::ThreadContext;

/// 线程状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    /// 已创建但尚未加入就绪队列。
    New,
    /// 就绪，等待被调度。
    Ready,
    /// 正在运行。
    Running,
    /// 阻塞等待（v0.5 第二版：等 IPC / sleep）。
    Blocked,
    /// 已结束，等待回收。
    Terminated,
}

impl ThreadState {
    /// 状态名字（串口日志/自检用）。
    pub const fn name(self) -> &'static str {
        match self {
            ThreadState::New => "NEW",
            ThreadState::Ready => "READY",
            ThreadState::Running => "RUNNING",
            ThreadState::Blocked => "BLOCKED",
            ThreadState::Terminated => "TERMINATED",
        }
    }
}

/// 线程控制块（TCB）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Thread {
    /// 线程号（内核线程第一版与进程一一对应）。
    pub pid: u64,
    /// 调试名。
    pub name: &'static str,
    /// 上下文：callee-saved 寄存器区 + 内核栈指针。
    pub ctx: ThreadContext,
    /// 当前状态。
    pub state: ThreadState,
    /// 剩余时间片（tick 数）；由 IRQ0（100Hz）驱动递减。
    pub quantum_ticks: u32,
    /// 内核栈顶（仅记录，便于自检与后续回收）。
    pub stack_top: u64,
    /// 被调度执行的次数。
    pub runs: u64,
    /// 被切出（让出 CPU）的次数。
    pub switches: u64,
}

impl Thread {
    /// 空槽位（静态任务表初始化用）。
    pub const EMPTY: Self = Self {
        pid: 0,
        name: "",
        ctx: ThreadContext::new(),
        state: ThreadState::New,
        quantum_ticks: 0,
        stack_top: 0,
        runs: 0,
        switches: 0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_are_stable() {
        assert_eq!(ThreadState::Ready.name(), "READY");
        assert_eq!(ThreadState::Running.name(), "RUNNING");
    }
}
