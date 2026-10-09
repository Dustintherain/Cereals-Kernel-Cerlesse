//! IPC（进程间通信，v0.5）
//!
//! - [`pipe`]：单向管道骨架（v0.5 已实现：创建 / 写入 / 读取 + 串口可观测日志）；
//! - [`channel`]：消息通道（后续占位）；
//! - [`shared_memory`]：共享内存（后续占位）。
//!
//! v0.5 目标是「可验证骨架」：不引入文件描述符表完整语义（v0.7）、
//! 不做双向管道 / select / poll / 完整阻塞调度闭环（见 docs/process.md 第 2.6 节）。

pub mod channel;
pub mod pipe;
pub mod shared_memory;
