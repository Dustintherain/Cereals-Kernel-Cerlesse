//! 进程级结构（v0.5 占位，随 v0.6/v0.7 落地）
//!
//! v0.5 第一版只实现「内核线程 + Round Robin 调度」，进程级资源尚未接入：
//!
//! | 结构 | 说明 | 计划版本 |
//! | ---- | ---- | ---- |
//! | Address Space | 页表根 / 用户态隔离 | v0.6（见 docs/syscall.md，ADR-009） |
//! | File Table | 打开的 fd 集合 | v0.7（见 docs/filesystem.md） |
//! | Signal | 信号语义 | v0.6+（保守推迟） |
//! | Threads | 进程下的线程组 | v0.5 第二版 |
//!
//! 这里先给出最小的 `Process` 记录，让线程与进程的关系在代码里可见；
//! 真正的地址空间与 fd 表在对应阶段接入。

#![allow(dead_code)]

/// 进程记录（第一版仅作登记，不含地址空间/文件表）。
#[derive(Clone, Copy)]
pub struct Process {
    /// 进程号（与主线程 pid 相同）。
    pub pid: u64,
    /// 调试名。
    pub name: &'static str,
    /// 该进程下的线程数（第一版恒为 1）。
    pub threads: u32,
}

impl Process {
    /// 创建一个只含登记信息的进程记录。
    pub const fn new(pid: u64, name: &'static str) -> Self {
        Self {
            pid,
            name,
            threads: 1,
        }
    }
}
