//! 内存类系统调用（v0.9 运行时前置条件：用户堆）
//!
//! v0.7 的用户程序只有「映像窗口（64 KiB）+ 4 页栈」，**没有堆** —— `brk`/`mmap`
//! 落在公共 ENOSYS 桩上，动态内存只能放进栈或静态数组（见 `libc/README.md` 第 4 节）。
//! 这一节补上第一个运行时前置条件：**可分配的用户内存**。
//!
//! | syscall | 号 | 行为 |
//! | ---- | ---- | ---- |
//! | `mmap` | 9 | 匿名映射：`MAP_PRIVATE|MAP_ANONYMOUS`，first-fit 落在 mmap 窗口（256 KiB），返回页对齐地址 |
//! | `mprotect` | 10 | 改写**已映射**页的保护属性（v1.0 第一步）：页对齐、单个窗口、全段已映射；`PROT_NONE` 关页帧而不归还 |
//! | `munmap` | 11 | 整段归还：要求页对齐、全段已映射，缺一即 `EINVAL` |
//! | `brk` | 12 | 增长/收缩堆顶（窗口 256 KiB）；`addr == 0` 只查询。**Linux 语义：不返回错误**，失败返回原值 |
//!
//! ## 三条刻意的取舍（都有理由，也都有对应的验收条目）
//!
//! 1. **`mmap` 收 6 个参数**（`addr, len, prot, flags, fd, offset`，见 `syscall/mod.rs`）。
//!    - **匿名映射**（`MAP_PRIVATE|MAP_ANONYMOUS`，`fd = -1`）：不使用偏移，要求
//!      `offset = 0`（非 0 → `EINVAL`）；
//!    - **文件映射**（v1.0 第三步）：`fd ≥ 3`、`flags = MAP_PRIVATE`、只读
//!      （拒绝 `PROT_WRITE`/`PROT_EXEC`，不碰 W^X）、`offset` 必须**页对齐**；
//!      `[offset, offset+len)` 的字节拷进新映射，文件尾之后补零。`MAP_SHARED`
//!      （写回文件）仍未实现 —— 明确拒绝。
//! 2. **W^X 是硬约束**：`PROT_WRITE|PROT_EXEC` 在 `mmap` 与 `mprotect` 上都是
//!    `EINVAL`。自分配内存不存在「既可写又可执行」这一档；需要 JIT 的运行时走
//!    「先 `mmap(RW)`、写完 `mprotect(RX)`」这条路（v1.0 之前 `mprotect` 落在
//!    ENOSYS 桩上，那条路走不通）。
//! 3. **`PROT_NONE` 现在合法**（v1.0 第一步起）：`mmap(PROT_NONE)` 得到一个
//!    已分配但 ring3 不可访问的映射，`mprotect` 再启用它 —— 在此之前没有
//!    `mprotect`，`PROT_NONE` 的段再也回不来，所以当时直接拒。`MAP_FIXED` 仍然
//!    是 `EINVAL`（内核按调用方给的固定地址放置映射可能覆盖映像/栈）。
//!
//! `mprotect` 的失败面比 `mmap` 大（对齐、窗口、已映射、跨窗口），因此错误路径
//! 把**每一类原因**写在串口上（`user: mprotect rejected (<reason>)` 与
//! `user: mprotect errno=<n> ...` 两条），验收脚本断言它们，而不是只看返回值。
//!
//! 每个失败的路径都在串口留一行 `user: <op> rejected (...)` 或 `user: <op> errno=<n>`：
//! 与文件类 syscall 一样，「内核真的校验过」这件事必须是可断言的串口事实。

use crate::driver::serial;
use crate::fs::file;
use crate::fs::vfs::{self, EINVAL};
use crate::syscall::address_space::{
    UserSpace, PROT_EXEC, PROT_MASK, PROT_WRITE, USER_HEAP_MAX, USER_HEAP_VIRT, USER_MMAP_MAX,
};
use crate::memory::page::PAGE_SIZE;

/// `-EFAULT`：用户传了不属于自己的地址区间（本模块只在「没有当前用户地址空间」时用）。
/// **定义在 `abi::errno`**。
use abi::errno::EFAULT;

/// `MAP_*`：只支持私有匿名映射；`MAP_FIXED`/`MAP_SHARED` 等一律 `EINVAL`。
/// 位值**定义在 `abi::flags`**（阶段三的单一事实源）。
use abi::flags::{FD_FIRST_DYNAMIC, MAP_ANONYMOUS, MAP_PRIVATE, MAP_SUPPORTED};

/// 取当前用户地址空间的可变引用。
///
/// 入口汇编把 `CER_USER_SPACE`（`enter_ring3` 时写入 `&space`）作为 `space` 传给分发层，
/// 因此这里的指针在一次用户态运行期间是有效的（`UserSpace` 活在内核调用栈上）。
///
/// # Safety
/// 只在 ring3 发起的 syscall 路径上调用；`space` 为 0 时返回 `None`（内核自身发起）。
unsafe fn space_mut<'a>(space: *const UserSpace) -> Option<&'a mut UserSpace> {
    if space.is_null() {
        None
    } else {
        Some(unsafe { &mut *(space as *mut UserSpace) })
    }
}

/// 以页为单位的上取整（溢出返回 `None`）。
fn page_round_up(len: u64) -> Option<u64> {
    len.checked_add(PAGE_SIZE - 1).map(|v| v & !(PAGE_SIZE - 1))
}

/// 系统调用 `brk(addr)`。
///
/// 返回值恒为「当前 break」：成功时是页对齐后的新值（`>= addr`），失败时是原值。
/// 用户态据此判断（`ret < addr` 即失败），与 Linux 的 `brk` 一致 ——
/// 这也是将来 glibc 的 `sbrk`/`malloc` 能直接接上的前提。
pub unsafe fn sys_brk(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let addr = args[0];
    let space = match unsafe { space_mut(space) } {
        Some(space) => space,
        None => {
            serial::println("user: brk rejected (no current user address space)");
            return 0;
        }
    };
    let before = space.brk_value();
    let after = space.brk(addr);
    if addr == 0 {
        serial::print("user: brk query -> ");
        serial::print_hex(after);
        serial::println("");
        return after as i64;
    }
    if after < addr {
        serial::print("user: brk rejected addr=");
        serial::print_hex(addr);
        serial::print(" (outside heap window [");
        serial::print_hex(USER_HEAP_VIRT);
        serial::print(", ");
        serial::print_hex(USER_HEAP_VIRT + USER_HEAP_MAX);
        serial::println("))");
        return before as i64;
    }
    serial::print("user: brk addr=");
    serial::print_hex(addr);
    serial::print(" -> ");
    serial::print_hex(after);
    serial::print(" pages=");
    serial::print_dec((after - USER_HEAP_VIRT) / PAGE_SIZE);
    serial::println("");
    after as i64
}

/// 系统调用 `mmap(addr, len, prot, flags, fd, offset)`（`offset` = `args[5]`，见模块文档）。
///
/// `addr` 只是提示、内核忽略（不支持 `MAP_FIXED`），返回内核选的页对齐地址。
/// `offset` 只对文件映射有意义；匿名映射要求它为 0（非 0 → `EINVAL`）。
pub unsafe fn sys_mmap(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let hint = args[0];
    let len = args[1];
    let prot = args[2];
    let flags = args[3];
    let fd = args[4];
    let offset = args[5];

    let reject = |reason: &str| -> i64 {
        serial::print("user: mmap rejected (");
        serial::print(reason);
        serial::print(") hint=");
        serial::print_hex(hint);
        serial::print(" len=");
        serial::print_dec(len);
        serial::print(" prot=");
        serial::print_hex(prot);
        serial::print(" flags=");
        serial::print_hex(flags);
        serial::print(" offset=");
        serial::print_hex(offset);
        serial::println("");
        EINVAL
    };

    if len == 0 {
        return reject("len=0");
    }
    // 未知保护位（例如 PROT_GROWSDOWN）→ 拒绝，而不是忽略
    if prot & !PROT_MASK != 0 {
        return reject("unknown PROT bits");
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return reject("PROT_WRITE|PROT_EXEC is not allowed (W^X)");
    }

    let space = match unsafe { space_mut(space) } {
        Some(space) => space,
        None => {
            serial::println("user: mmap rejected (no current user address space)");
            return EFAULT;
        }
    };

    if flags & MAP_ANONYMOUS != 0 {
        // ---- 匿名映射：不用偏移 ----
        if flags & MAP_SUPPORTED != flags {
            return reject("flags must be exactly MAP_PRIVATE|MAP_ANONYMOUS");
        }
        if fd != u64::MAX {
            return reject("MAP_ANONYMOUS expects fd=-1");
        }
        if offset != 0 {
            return reject("MAP_ANONYMOUS requires offset=0");
        }
        let result = space.mmap_anon(len, prot);
        return finish_mmap(result, len, prot, flags, offset, None);
    }

    // ---- 文件映射（v1.0 第三步）：只读、私有、offset 页对齐 ----
    if flags != MAP_PRIVATE {
        return reject("file-backed mmap supports MAP_PRIVATE only (no MAP_SHARED)");
    }
    if prot & (PROT_WRITE | PROT_EXEC) != 0 {
        return reject("file-backed mmap is read-only (no PROT_WRITE/PROT_EXEC)");
    }
    if offset % PAGE_SIZE != 0 {
        return reject("file-backed mmap offset must be page-aligned");
    }
    if fd == u64::MAX || fd < FD_FIRST_DYNAMIC as u64 {
        return reject("file-backed mmap needs an open fd (>= 3)");
    }
    let Some(path) = file::path_of(fd as i32) else {
        return reject("file-backed mmap fd is not open");
    };
    if !file::is_readable(fd as i32) {
        return reject("file-backed mmap fd is not open for reading");
    }
    let data = match vfs::read_all(&path) {
        Ok(data) => data,
        Err(_) => return reject("file-backed mmap cannot read file"),
    };
    if offset >= data.len() as u64 {
        return reject("file-backed mmap offset is past end of file");
    }
    let bytes = page_round_up(len).unwrap_or(0);
    if bytes > USER_MMAP_MAX {
        return reject("file-backed mmap length exceeds the mmap window");
    }
    let addr = match space.mmap_anon(len, prot) {
        Ok(addr) => addr,
        Err(errno) => return finish_mmap(Err(errno), len, prot, flags, offset, Some(&path)),
    };
    // 把 [offset, offset+len) 的字节拷进新映射；文件尾之后保持零（`mmap_anon` 已清零）。
    let start = offset as usize;
    let end = core::cmp::min(start + len as usize, data.len());
    if end > start {
        space.mmap_write(addr, &data[start..end]);
    }
    finish_mmap(Ok(addr), len, prot, flags, offset, Some(&path))
}

/// 统一打印 `mmap` 的结果并返回：成功一行 / 失败一行（文件映射多一个 `file=` 字段）。
fn finish_mmap(
    result: Result<u64, i64>,
    len: u64,
    prot: u64,
    flags: u64,
    offset: u64,
    path: Option<&str>,
) -> i64 {
    match result {
        Ok(addr) => {
            serial::print("user: mmap len=");
            serial::print_dec(len);
            serial::print(" prot=");
            serial::print_hex(prot);
            serial::print(" flags=");
            serial::print_hex(flags);
            serial::print(" offset=");
            serial::print_hex(offset);
            if let Some(path) = path {
                serial::print(" file=");
                serial::print(path);
            }
            serial::print(" -> ");
            serial::print_hex(addr);
            serial::print(" pages=");
            serial::print_dec(page_round_up(len).unwrap_or(0) / PAGE_SIZE);
            serial::println("");
            addr as i64
        }
        Err(errno) => {
            serial::print("user: mmap errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" (window max=");
            serial::print_dec(USER_MMAP_MAX);
            serial::println(")");
            errno
        }
    }
}

/// 系统调用 `munmap(addr, len)`：整段归还匿名映射（页对齐 + 全段已映射）。
pub unsafe fn sys_munmap(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let addr = args[0];
    let len = args[1];
    let space = match unsafe { space_mut(space) } {
        Some(space) => space,
        None => {
            serial::println("user: munmap rejected (no current user address space)");
            return EFAULT;
        }
    };
    match space.munmap_anon(addr, len) {
        Ok(()) => {
            serial::print("user: munmap addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: munmap errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::print(" addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" (must be page-aligned, inside the mmap window, fully mapped; max=");
            serial::print_dec(USER_MMAP_MAX);
            serial::println(")");
            errno
        }
    }
}

/// 系统调用 `mprotect(addr, len, prot)`（v1.0 第一步：运行时改保护属性）。
///
/// ABI 层校验（都在这里，地址空间层只管翻译与落页表）：
///
/// - `addr` 必须页对齐、`len > 0`（与 `munmap` 一致，Linux 的 `len=0` 也按 `EINVAL` 处理）；
/// - `prot` 里的未知位（如 `PROT_GROWSDOWN`）一律 `EINVAL`，不静默忽略；
/// - `PROT_WRITE|PROT_EXEC` 仍然 `EINVAL`：`mprotect` 不能绕过 W^X。它提供的正是
///   「先 `mmap(RW)` 写、再 `mprotect(RX)` 执行」这条路，而不是可写可执行的段。
///
/// 返回 `0`；地址空间层拒绝时返回 `-EINVAL` 并把原因写在串口上。
pub unsafe fn sys_mprotect(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let addr = args[0];
    let len = args[1];
    let prot = args[2];

    let reject = |reason: &str| -> i64 {
        serial::print("user: mprotect rejected (");
        serial::print(reason);
        serial::print(") addr=");
        serial::print_hex(addr);
        serial::print(" len=");
        serial::print_dec(len);
        serial::print(" prot=");
        serial::print_hex(prot);
        serial::println("");
        EINVAL
    };

    if addr % PAGE_SIZE != 0 {
        return reject("addr is not page-aligned");
    }
    if len == 0 {
        return reject("len=0");
    }
    if prot & !PROT_MASK != 0 {
        return reject("unknown PROT bits");
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return reject("PROT_WRITE|PROT_EXEC is not allowed (W^X)");
    }

    let space = match unsafe { space_mut(space) } {
        Some(space) => space,
        None => {
            serial::println("user: mprotect rejected (no current user address space)");
            return EFAULT;
        }
    };

    match space.mprotect(addr, len, prot) {
        Ok(pages) => {
            serial::print("user: mprotect addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" prot=");
            serial::print_hex(prot);
            serial::print(" -> pages=");
            serial::print_dec(pages as u64);
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: mprotect errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::print(" addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" prot=");
            serial::print_hex(prot);
            serial::println(" (page-aligned, one window, fully mapped; the user stack is fixed RWNX)");
            errno
        }
    }
}
