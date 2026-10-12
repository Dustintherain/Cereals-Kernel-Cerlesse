//! `getprocs` 进程记录（**权威布局**，内核 `syscall::process` 与 libc 都引用这里）。
//!
//! ```text
//! 偏移  长度  字段        说明
//! 0     8     pid         LE u64
//! 8     8     ppid        LE u64（父进程；没有父进程时 = pid 或 0，见 docs/process.md）
//! 16    4     state       LE u32（进程状态枚举的编号，见 libc::procinfo_state）
//! 20    4     reserved    必须为 0
//! 24    16    name        UTF-8 进程名，不足部分补 0
//! ```
//!
//! 与 `getdents` 同一套做法：**定长记录、一次调用写满、确定性顺序**
//! （按进程表槽位序），因此两次查询相同输入得到逐字节相同的输出 ——
//! 这是可以脚本断言的前提。

/// 单条进程记录的字节数（[`NAME_OFFSET`] + [`NAME_MAX`]）。
pub const PROCINFO_SIZE: usize = 40;
/// 记录里进程名字段的容量。
pub const NAME_MAX: usize = 16;
/// 名字字段的偏移。
pub const NAME_OFFSET: usize = 24;
