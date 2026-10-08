//! 进程 / 线程模型（v0.5）
//!
//! v0.5 第一版聚焦「内核线程 + Round Robin 调度」这条最小可验证路径：
//! - [`thread`]：线程结构与状态机（NEW / READY / RUNNING / BLOCKED / TERMINATED）；
//! - [`pid`]：PID 分配与回收；
//! - [`context`]：线程上下文（转发 `arch::x86_64::context` 的切换原语）；
//! - [`process`]：进程级结构（Address Space / File Table / Signal）—— 待 v0.6/v0.7 落地。
//!
//! 调度器位于 [`crate::scheduler`]；本模块只描述「被调度的对象」。

pub mod context;
pub mod pid;
pub mod process;
pub mod thread;
