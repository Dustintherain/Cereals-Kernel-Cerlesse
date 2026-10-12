//! 打开的文件 / 文件描述符表（v0.7 实现）
//!
//! 这一层把「文件系统里的文件」变成「进程可以拿着用的整数 fd」：
//!
//! - **fd 0/1/2 固定保留**给标准输入/输出/错误。v0.7 里 0 尚未接通输入源，
//!   1/2 直接写串口（由 syscall 层完成，不在这里）；
//! - **动态 fd 从 [`FD_FIRST_DYNAMIC`] (=3) 开始**，`open` 取**最小空闲槽位**
//!   （因此「close 之后再 open 会复用同一个 fd」是可用断言观察到的行为）；
//! - 每个 fd 记录 `path` + `offset` + 读写模式。`offset` 由 `read`/`write`
//!   推进，这是 v0.7 唯一的位置状态（没有 `lseek`、没有 `O_APPEND`）。
//!
//! 一次只跑一个用户程序（v0.6/v0.7 的模型），所以 fd 表是**全局**的，
//! 每次进入用户态前用 [`reset`] 清空动态槽位。真正的「每进程 fd 表」等 v0.8
//! 的进程对象。

use alloc::string::String;

use crate::fs::vfs;

/// fd 槽位总数（0..=15；0/1/2 保留）。
pub const FD_MAX: usize = 16;

/// fd 编号与 `open` 标志（**再导出**，定义在 `abi::flags` —— 这是 ABI，
/// 用户态按同一批位值调用 `open`）。`O_ALLOWED` 是「允许出现在 `flags` 里的位」，
/// 其余位一律 [`vfs::EINVAL`]（不静默忽略）。
pub use abi::flags::{
    FD_FIRST_DYNAMIC, O_ACCMODE, O_ALLOWED, O_CREAT, O_RDONLY, O_RDWR, O_WRONLY,
};

/// 打开模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenMode {
    /// 只读
    Read,
    /// 只写
    Write,
    /// 读写
    ReadWrite,
}

impl OpenMode {
    /// 是否允许读。
    pub const fn readable(self) -> bool {
        matches!(self, OpenMode::Read | OpenMode::ReadWrite)
    }

    /// 是否允许写。
    pub const fn writable(self) -> bool {
        matches!(self, OpenMode::Write | OpenMode::ReadWrite)
    }

    /// 由 `flags` 的低 2 位解出模式（其余位已在 `open` 里被 `O_ALLOWED` 挡掉）。
    const fn from_flags(flags: i32) -> Self {
        match flags & O_ACCMODE {
            O_WRONLY => OpenMode::Write,
            O_RDWR => OpenMode::ReadWrite,
            O_RDONLY => OpenMode::Read,
            _ => OpenMode::Read, // unreachable：O_ALLOWED 已经限定了低 2 位
        }
    }
}

/// 一个打开的 fd。
#[derive(Clone, Debug)]
pub struct FileHandle {
    /// 文件路径（规范化后的绝对路径）
    pub path: String,
    /// 当前读写偏移
    pub offset: u64,
    /// 打开模式
    pub mode: OpenMode,
}

/// fd 表。
pub struct FdTable {
    slots: [Option<FileHandle>; FD_MAX],
}

impl FdTable {
    /// 空表（0/1/2 也留空：它们是**保留位**而不是已打开的文件）。
    fn new() -> Self {
        Self {
            slots: [const { None }; FD_MAX],
        }
    }

    /// 清空全部动态 fd（保留 0/1/2）。
    fn clear(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if i >= FD_FIRST_DYNAMIC as usize {
                *slot = None;
            }
        }
    }
}

// 全局 fd 表：一次只跑一个用户程序（见模块头注释）。
static mut FD_TABLE: Option<FdTable> = None;

/// 以闭包访问 fd 表（首次访问时初始化）。
fn with_table<T>(f: impl FnOnce(&mut FdTable) -> T) -> T {
    unsafe {
        let table = (&raw mut FD_TABLE).as_mut().expect("FD_TABLE 是静态量");
        let table = table.get_or_insert_with(FdTable::new);
        f(table)
    }
}

/// 清空动态 fd（进入用户态前调用，保证上一个程序的 fd 不泄漏给下一个）。
pub fn reset() {
    with_table(|t| t.clear());
}

/// `open(path, flags)`：打开（必要时创建）文件，返回动态 fd。
///
/// 语义（v0.7 最小集）：
/// - `flags` 只允许 [`O_ALLOWED`] 里的位，其余 [`vfs::EINVAL`]；
/// - 带 [`O_CREAT`] 时：文件不存在就创建（父目录必须存在）；已存在则沿用，
///   此时**不截断**（v0.7 没有 `O_TRUNC`）；
/// - 返回最小的空闲 fd（所以 `close(3)` 之后 `open` 会再次拿到 3）。
pub fn open(path: &str, flags: i32) -> Result<i32, i64> {
    if flags & !O_ALLOWED != 0 {
        return Err(vfs::EINVAL);
    }
    let path = vfs::normalize(path)?;

    if flags & O_CREAT != 0 {
        match vfs::create(&path) {
            Ok(()) => {}
            Err(vfs::EEXIST) => {} // 已存在：O_CREAT 不带 O_EXCL 视为成功
            Err(err) => return Err(err),
        }
    }
    let meta = vfs::stat(&path)?;
    if meta.kind.is_dir() {
        return Err(vfs::EISDIR);
    }

    let mode = OpenMode::from_flags(flags);
    with_table(|t| {
        for (i, slot) in t.slots.iter_mut().enumerate() {
            if i < FD_FIRST_DYNAMIC as usize || slot.is_some() {
                continue;
            }
            *slot = Some(FileHandle {
                path: path.clone(),
                offset: 0,
                mode,
            });
            return Ok(i as i32);
        }
        Err(vfs::ENOSPC) // fd 用尽（16 - 3 = 13 个动态槽位）
    })
}

/// 取某个已打开 fd 的路径（**克隆**一份，不把 fd 表借给调用方）。
///
/// 用途：`syscall/file.rs` 在 `write` 时要按**这个 fd 的路径**做受保护前缀判定
/// （判定点 1 里「write 到已打开文件」的那一半），而路径状态在 fd 表里。
/// 克隆是刻意的：判定会打串口/写审计，不应在持有 fd 表的情况下做。
pub fn path_of(fd: i32) -> Option<String> {
    with_table(|t| {
        t.slots
            .get(fd as usize)
            .and_then(|slot| slot.as_ref().map(|h| h.path.clone()))
    })
}

/// `read(fd, buf)`：从当前偏移读入 `buf`，推进偏移。
pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize, i64> {
    with_table(|t| {
        let slot = t
            .slots
            .get_mut(fd as usize)
            .ok_or(vfs::EBADF)?
            .as_mut()
            .ok_or(vfs::EBADF)?; // 未打开的 fd（含 0/1/2）
        if !slot.mode.readable() {
            return Err(vfs::EBADF); // 只写打开
        }
        let n = vfs::read(&slot.path, slot.offset, buf)?;
        slot.offset += n as u64;
        Ok(n)
    })
}

/// `write(fd, data)`：从当前偏移写入 `data`，推进偏移。
pub fn write(fd: i32, data: &[u8]) -> Result<usize, i64> {
    with_table(|t| {
        let slot = t
            .slots
            .get_mut(fd as usize)
            .ok_or(vfs::EBADF)?
            .as_mut()
            .ok_or(vfs::EBADF)?;
        if !slot.mode.writable() {
            return Err(vfs::EBADF); // 只读打开
        }
        let n = vfs::write(&slot.path, slot.offset, data)?;
        slot.offset += n as u64;
        Ok(n)
    })
}

/// `close(fd)`：释放槽位。
pub fn close(fd: i32) -> Result<(), i64> {
    if fd < FD_FIRST_DYNAMIC {
        return Err(vfs::EBADF); // 0/1/2 是保留位，不能关闭
    }
    with_table(|t| {
        let slot = t.slots.get_mut(fd as usize).ok_or(vfs::EBADF)?;
        if slot.take().is_none() {
            return Err(vfs::EBADF);
        }
        Ok(())
    })
}
