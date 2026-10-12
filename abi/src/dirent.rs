//! `getdents` 目录记录（**权威布局**，内核 `syscall::dir` 与 libc 都引用这里）。
//!
//! 一次调用把整目录**定长记录**顺序写进用户缓冲 —— 定长让用户态不必解析变长链表，
//! 内核对每条记录的写入也都是常数时间、不依赖目录大小：
//!
//! ```text
//! 偏移  长度  字段        说明
//! 0     1     kind        0 = 文件，1 = 目录（[`KIND_FILE`] / [`KIND_DIR`]；内核侧对应 fs::inode::NodeKind）
//! 1     7     reserved    必须为 0（内核写 0；用户态不应依赖其内容）
//! 8     8     size        LE u64，普通文件字节数（目录恒 0）
//! 16    2     name_len    LE u16，名字的**实际**字节数（≤ [`NAME_MAX`]）
//! 18    6     reserved    必须为 0
//! 24    64    name        UTF-8 名字，不足部分补 0（无 NUL 终止符语义）
//! ```
//!
//! 与 Linux `getdents64` 的差异是刻意的：Linux 的 `linux_dirent64` 是变长记录且带
//! `ino`/`off`/`d_type` 字段，本内核的 VFS 还没有 inode 号与硬链接，因此这里
//! 只写「VFS 真的有的东西」，不编造字段。返回值为**写满的记录数**（0 = 空目录），
//! 缓冲装不下更多记录时**整条截断**而不是报错。

/// 单条目录记录的字节数（[`NAME_OFFSET`] + [`NAME_MAX`]）。
pub const DIRENT_SIZE: usize = 88;
/// 记录里名字字段的容量（长于此的名字被截断，`name_len` 如实变小）。
pub const NAME_MAX: usize = 64;
/// 名字字段的偏移（用户态解析器不自己算偏移）。
pub const NAME_OFFSET: usize = 24;

/// 记录里 `kind` 的取值：普通文件。
pub const KIND_FILE: u8 = 0;
/// 记录里 `kind` 的取值：目录。
pub const KIND_DIR: u8 = 1;
