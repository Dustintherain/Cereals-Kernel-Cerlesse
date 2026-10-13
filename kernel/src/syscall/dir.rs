//! 目录类系统调用（v0.9 第五步：`getdents`）
//!
//! v0.7 的 VFS 早就有 `list()`（`fs::vfs`），但它只在**内核自测**里用过 ——
//! 用户态没有任何办法列举目录，因此 `ls` 这类程序写不出来。这一层把 VFS 的
//! 目录列举接到 syscall 上，形状与 `open`/`execve` 完全一致：路径用
//! **指针 + 长度**（不在不可信内存里找终止符），输出写到用户提供的缓冲。
//!
//! ## 记录格式（`getdents` 的 ABI）
//!
//! **记录的权威布局在 `abi::dirent`**（阶段三起）：内核与 libc 引用同一份定义，
//! 不再靠「`libc::DIRENT_SIZE` 必须与这里一致」的注释维持。下面复述一遍是为了
//! 在 syscall 层能直接读到每一段的含义。
//!
//! 一次调用把整目录**定长记录**顺序写进用户缓冲 —— 定长让用户态不必解析
//! 变长链表，内核对每条记录的写入也都是常数时间、不依赖目录大小：
//!
//! ```text
//! 偏移  长度  字段        说明
//! 0     1     kind        0 = 文件，1 = 目录（见 [`crate::fs::inode::NodeKind`]）
//! 1     7     reserved    必须为 0（内核写 0；用户态不应依赖其内容）
//! 8     8     size        LE u64，普通文件字节数（目录恒 0）
//! 16    2     name_len    LE u16，名字的**实际**字节数（≤ [`DIRENT_NAME_MAX`]）
//! 18    6     reserved    必须为 0
//! 24    64    name        UTF-8 名字，不足部分补 0（无 NUL 终止符语义）
//! ```
//!
//! 与 Linux `getdents64` 的差异是刻意的：Linux 的 `linux_dirent64` 是变长记录
//! 且带 `ino`/`off`/`d_type` 字段，本内核的 VFS 还没有 inode 号与硬链接
//! （见 `fs/inode.rs`），因此这里只写「VFS 真的有的东西」，不编造字段。
//! 记录数与 `name_len` 是**内核写的**，用户态不需要猜。
//!
//! ## 返回值
//!
//! 写满的记录数（`0` = 空目录），缓冲装不下更多记录时**截断**而不是报错 ——
//! 调用方拿到的永远是「前 N 条，且每条完整」；负数 = `-errno`。
//!
//! 缓冲比一条记录还小是用法错误（[`EINVAL`]），而不是静默返回 0：
//! 静默会让 `ls` 显示「空目录」，与「缓冲太小」混为一谈。

use abi::errno::EINVAL;
use crate::driver::serial;
use crate::fs::vfs;
use crate::syscall::address_space::UserSpace;
use crate::syscall::file::{user_bytes, user_bytes_mut};

/// 单条目录记录的字节数与名字字段容量（**再导出**，定义在 `abi::dirent`）。
pub use abi::dirent::{DIRENT_SIZE, NAME_MAX as DIRENT_NAME_MAX};
/// 路径长度上限（与 `open`/`execve` 一致；定义在 `abi::limits`）。
const PATH_MAX: u64 = abi::limits::PATH_MAX;

/// 系统调用 `getdents(path_ptr, path_len, buf_ptr, buf_len)`：列举目录。
///
/// # Safety
/// `space` 必须是当前生效的用户地址空间；`buf_ptr` 必须可写。
pub unsafe fn sys_getdents(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let (path_ptr, path_len, buf_ptr, buf_len) = (args[0], args[1], args[2], args[3]);
    if path_len == 0 || path_len > PATH_MAX {
        return EINVAL;
    }
    if (buf_len as usize) < DIRENT_SIZE {
        return EINVAL;
    }

    // 1. 路径（校验 → UTF-8 → 规范化）
    let Ok(raw) = (unsafe { user_bytes(space, path_ptr, path_len, "getdents") }) else {
        return crate::syscall::file::EFAULT;
    };
    let Ok(path) = core::str::from_utf8(raw) else {
        return EINVAL;
    };
    let Ok(path) = vfs::normalize(path) else {
        return EINVAL;
    };

    // 2. 输出缓冲（要求可写）
    let Ok(out) = (unsafe { user_bytes_mut(space, buf_ptr, buf_len, "getdents") }) else {
        return crate::syscall::file::EFAULT;
    };

    // 3. VFS 列举（挂载表路由 → 后端），错误原样返回给用户态
    let entries = match vfs::list(&path) {
        Ok(entries) => entries,
        Err(errno) => {
            log_errno(&path, errno);
            return errno;
        }
    };

    // 4. 逐条编码；装不下就截断在这里（每条都是完整的）
    let cap = out.len() / DIRENT_SIZE;
    let count = entries.len().min(cap);
    for (i, entry) in entries.iter().take(count).enumerate() {
        let rec = &mut out[i * DIRENT_SIZE..(i + 1) * DIRENT_SIZE];
        rec.fill(0);
        rec[0] = if entry.meta.kind.is_dir() { 1 } else { 0 };
        rec[8..16].copy_from_slice(&entry.meta.size.to_le_bytes());
        let name = entry.name.as_bytes();
        let n = name.len().min(DIRENT_NAME_MAX);
        rec[16..18].copy_from_slice(&(n as u16).to_le_bytes());
        rec[24..24 + n].copy_from_slice(&name[..n]);
    }

    serial::print("user: getdents path=");
    serial::print(&path);
    serial::print(" entries=");
    serial::print_dec(entries.len() as u64);
    serial::print(" returned=");
    serial::print_dec(count as u64);
    serial::println("");
    count as i64
}

/// `user: getdents path=<p> errno=<n>`：目录列举失败的留痕（负例可断言）。
fn log_errno(path: &str, errno: i64) {
    serial::print("user: getdents path=");
    serial::print(path);
    serial::print(" errno=");
    serial::print_dec(errno.unsigned_abs());
    serial::println("");
}
