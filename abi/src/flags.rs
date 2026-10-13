//! 标志位与 fd 编号（`open` / `mmap` / `mprotect` 的参数取值）。
//!
//! 类型约定：`O_*` 与 fd 编号两侧都是 `i32`（内核 `fs::file`、libc 都是），
//! 因此可以**直接 `pub use`**；`PROT_*` / `MAP_*` 是寄存器参数，
//! 内核侧按 `u64` 读、libc 侧按 `i32` 传，abi 给寄存器语义的 `u64`。

/// 标准输入（v0.7 未接通：`read(0)` 只能取键盘行，无输入时返回 [`crate::errno::EAGAIN`]）。
pub const STDIN: i32 = 0;
/// 标准输出（内核把它接到串口）。
pub const STDOUT: i32 = 1;
/// 标准错误（同样接到串口）。
pub const STDERR: i32 = 2;
/// 第一个动态 fd：0/1/2 是保留位，`open` 从 3 起取最小空闲槽位。
pub const FD_FIRST_DYNAMIC: i32 = 3;

/// `open`：只读。
pub const O_RDONLY: i32 = 0;
/// `open`：只写。
pub const O_WRONLY: i32 = 1;
/// `open`：读写。
pub const O_RDWR: i32 = 2;
/// `open`：访问模式掩码（低 2 位）。
pub const O_ACCMODE: i32 = 0o3;
/// `open`：不存在则创建（父目录必须已存在）。
pub const O_CREAT: i32 = 0o100;
/// `open`：本内核接受的**全部**标志位（其余位一律 `EINVAL`，不静默忽略）。
pub const O_ALLOWED: i32 = O_ACCMODE | O_CREAT;

/// `mmap`/`mprotect`：不可访问（页帧仍在，可以再用 `mprotect` 启用）。
pub const PROT_NONE: u64 = 0x0;
/// `mmap`/`mprotect`：可读。
pub const PROT_READ: u64 = 0x1;
/// `mmap`/`mprotect`：可写。
pub const PROT_WRITE: u64 = 0x2;
/// `mmap`/`mprotect`：可执行（**不能**与 [`PROT_WRITE`] 同时用：内核强制 W^X）。
pub const PROT_EXEC: u64 = 0x4;
/// 已知保护位掩码（`prot & !PROT_MASK != 0` → `EINVAL`）。
pub const PROT_MASK: u64 = PROT_READ | PROT_WRITE | PROT_EXEC;

/// `mmap`：**共享**映射（写回文件，v1.0 第五步）。
///
/// 文件映射下它与 [`MAP_PRIVATE`] 互斥：`MAP_SHARED` 的页在 `msync` / `munmap`
/// 时把内容写回文件，`MAP_PRIVATE` 的改写只留在本进程。**匿名**共享映射
/// （`MAP_SHARED|MAP_ANONYMOUS`）仍未实现（那需要 `fork` 之间真正共享页帧）。
pub const MAP_SHARED: u64 = 0x1;
/// `mmap`：私有映射（写时复制语义：改写不落盘）。
pub const MAP_PRIVATE: u64 = 0x2;
/// `mmap` 的**映射类型**位（`MAP_SHARED` 与 `MAP_PRIVATE` 互斥，二选一）。
pub const MAP_TYPE: u64 = MAP_SHARED | MAP_PRIVATE;
/// `mmap`：匿名映射（要求 `fd = -1`；只与 [`MAP_PRIVATE`] 组合）。
pub const MAP_ANONYMOUS: u64 = 0x20;
/// `mmap`：接受的**全部**标志位（`MAP_FIXED` 等一律 `EINVAL`）。
pub const MAP_SUPPORTED: u64 = MAP_TYPE | MAP_ANONYMOUS;
/// `mmap`：**匿名**映射接受的全部标志位（`MAP_SHARED|MAP_ANONYMOUS` 仍未实现）。
pub const MAP_ANON_SUPPORTED: u64 = MAP_PRIVATE | MAP_ANONYMOUS;

/// `msync`：只把数据交给回写、不等它落盘（本内核同步写回，等价于 [`MS_SYNC`]）。
pub const MS_ASYNC: u64 = 0x1;
/// `msync`：让同一文件的其它映射副本失效（本内核没有页缓存，接受并忽略）。
pub const MS_INVALIDATE: u64 = 0x2;
/// `msync`：写回并等待完成。
pub const MS_SYNC: u64 = 0x4;
/// `msync`：接受的**全部**标志位（未知位一律 `EINVAL`）。
pub const MS_SUPPORTED: u64 = MS_ASYNC | MS_INVALIDATE | MS_SYNC;
