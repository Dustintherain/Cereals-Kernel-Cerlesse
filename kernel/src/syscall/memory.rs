//! 内存类系统调用（v0.9 运行时前置条件：用户堆）
//!
//! v0.7 的用户程序只有「映像窗口（64 KiB）+ 4 页栈」，**没有堆** —— `brk`/`mmap`
//! 落在公共 ENOSYS 桩上，动态内存只能放进栈或静态数组（见 `libc/README.md` 第 4 节）。
//! 这一节补上第一个运行时前置条件：**可分配的用户内存**。
//!
//! | syscall | 号 | 行为 |
//! | ---- | ---- | ---- |
//! | `mmap` | 9 | 匿名映射（`MAP_PRIVATE|MAP_ANONYMOUS`）或**文件映射**（`MAP_PRIVATE` / `MAP_SHARED`），first-fit 落在 mmap 窗口（256 KiB），返回页对齐地址 |
//! | `mprotect` | 10 | 改写**已映射**页的保护属性（v1.0 第一步）：页对齐、单个窗口、全段已映射；`PROT_NONE` 关页帧而不归还 |
//! | `munmap` | 11 | 整段归还（先写回共享文件页，v1.0 第五步）：要求页对齐、全段已映射，缺一即 `EINVAL` |
//! | `brk` | 12 | 增长/收缩堆顶（窗口 256 KiB）；`addr == 0` 只查询。**Linux 语义：不返回错误**，失败返回原值 |
//! | `msync` | 26 | 把 `MAP_SHARED` 文件映射的页写回文件（v1.0 第五步）：页对齐、全段已映射；非共享页跳过 |
//!
//! ## 三条刻意的取舍（都有理由，也都有对应的验收条目）
//!
//! 1. **`mmap` 收 6 个参数**（`addr, len, prot, flags, fd, offset`，见 `syscall/mod.rs`）。
//!    - **匿名映射**（`MAP_PRIVATE|MAP_ANONYMOUS`，`fd = -1`）：不使用偏移，要求
//!      `offset = 0`（非 0 → `EINVAL`）；
//!    - **文件映射**（v1.0 第三步只读、第五步可写）：`fd ≥ 3` 且可读、`flags` 恰为
//!      `MAP_PRIVATE` **或** `MAP_SHARED`、`prot` 不含 `PROT_EXEC`、`offset` 必须
//!      **页对齐**；`[offset, offset+len)` 与文件的交集拷进新映射，文件尾之后补零。
//!      `MAP_PRIVATE` 的改写只留在本进程（与匿名映射同构）；`MAP_SHARED` 的页在
//!      `msync`/`munmap` 时**写回文件**（要求 fd 打开可写），写回长度夹在映射那一刻的
//!      文件大小内 —— 映射不会把文件撑大。**匿名**共享映射
//!      （`MAP_SHARED|MAP_ANONYMOUS`）仍未实现（需要 `fork` 之间真正共享页帧），
//!      `MAP_FIXED` 同样是 `EINVAL`。
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
    UserSpace, MMAP_FILE_PATH_MAX, PROT_EXEC, PROT_MASK, PROT_WRITE, USER_HEAP_MAX,
    USER_HEAP_VIRT, USER_MMAP_MAX,
};
use crate::memory::page::PAGE_SIZE;

/// `-EFAULT`：用户传了不属于自己的地址区间（本模块只在「没有当前用户地址空间」时用）。
/// **定义在 `abi::errno`**。
use abi::errno::{EBADF, EFAULT};

/// `MAP_*`：匿名只支持 `MAP_PRIVATE|MAP_ANONYMOUS`；文件映射支持 `MAP_PRIVATE` 或
/// `MAP_SHARED`；`MAP_FIXED` 与 `MAP_SHARED|MAP_ANONYMOUS` 一律 `EINVAL`。
/// `MS_*` 是 `msync` 的标志位。位值**定义在 `abi::flags`**（阶段三的单一事实源）。
use abi::flags::{
    FD_FIRST_DYNAMIC, MAP_ANONYMOUS, MAP_ANON_SUPPORTED, MAP_PRIVATE, MAP_SHARED, MS_ASYNC,
    MS_SUPPORTED, MS_SYNC,
};

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
        if flags != MAP_ANON_SUPPORTED {
            return reject("flags must be exactly MAP_PRIVATE|MAP_ANONYMOUS");
        }
        if fd != u64::MAX {
            return reject("MAP_ANONYMOUS expects fd=-1");
        }
        if offset != 0 {
            return reject("MAP_ANONYMOUS requires offset=0");
        }
        let result = space.mmap_anon(len, prot);
        return finish_mmap(result, len, prot, flags, offset, None, false);
    }

    // ---- 文件映射（v1.0 第三步只读 / 第五步可写 + `MAP_SHARED` 写回）----
    //
    // `MAP_PRIVATE`：页是私有的（改了什么都不落盘，与匿名映射同构）；
    // `MAP_SHARED`：写回文件（`msync` 与 `munmap` 各触发一次写回，见 syscall/memory.rs 的
    // `sys_msync` / `sys_munmap`），因此要求 fd **打开可写** —— 只读 fd 说明调用方无意落盘。
    let shared = flags & MAP_SHARED != 0;
    if flags != MAP_PRIVATE && flags != MAP_SHARED {
        return reject("file-backed mmap flags must be exactly MAP_PRIVATE or MAP_SHARED");
    }
    if prot & PROT_EXEC != 0 {
        return reject("file-backed mmap is not executable (no PROT_EXEC)");
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
    if shared && prot & PROT_WRITE != 0 && !file::is_writable(fd as i32) {
        serial::print("user: mmap rejected (MAP_SHARED|PROT_WRITE needs an fd open for writing) path=");
        serial::println(&path);
        return EBADF;
    }
    if path.len() > MMAP_FILE_PATH_MAX {
        return reject("file-backed mmap path is longer than the mapping table allows");
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
    // `data.len()` 是**写回上限**：映射比文件长是合法的（尾部补零），但写回不撑大文件
    let file_len = data.len() as u64;
    let addr = match space.mmap_file(len, prot, &path, offset, file_len, shared) {
        Ok(addr) => addr,
        Err(errno) => {
            return finish_mmap(Err(errno), len, prot, flags, offset, Some(&path), shared)
        }
    };
    // 把 [offset, offset+len) 与文件的交集拷进新映射；文件尾之后保持零（`mmap_alloc` 已清零）。
    let start = offset as usize;
    let end = core::cmp::min(start + len as usize, data.len());
    if end > start {
        space.mmap_write(addr, &data[start..end]);
    }
    finish_mmap(Ok(addr), len, prot, flags, offset, Some(&path), shared)
}

/// 统一打印 `mmap` 的结果并返回：成功一行 / 失败一行（文件映射多一个 `file=` 字段，
/// 以及该映射是否 `MAP_SHARED` 的 `shared=` 字段）。
fn finish_mmap(
    result: Result<u64, i64>,
    len: u64,
    prot: u64,
    flags: u64,
    offset: u64,
    path: Option<&str>,
    shared: bool,
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
            serial::print(" shared=");
            serial::print_dec(shared as u64);
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

/// 系统调用 `munmap(addr, len)`：整段归还映射（页对齐 + 全段已映射）。
///
/// **先校验、再写回、最后归还**（v1.0 第五步）：`MAP_SHARED` 文件映射的页在这里被写回
/// 文件，因为归还之后页帧就没了。
///
/// 顺序是刻意的：写回之前先 [`UserSpace::munmap_check`] —— 一次注定失败的 `munmap`
/// （未对齐 / 越窗口 / 有页未映射 / 部分归还共享映射）**不会**产生任何 I/O
/// （否则就是「做了一半又被拒」）。校验通过后写回失败**也不阻止归还**：`munmap` 的
/// 成功与否不该被文件系统状态左右（Linux 的 `munmap` 也不会因为回写失败而失败），
/// 失败在串口上留一行 `user: munmap writeback errno=<n>`，如实上报。
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
    if space.munmap_check(addr, len).is_err() {
        // 归还本身会被拒（`munmap` 下面会打自己的 errno 行）：不写回、不打重复的日志
        return match space.munmap(addr, len) {
            Ok(()) => 0,
            Err(errno) => munmap_errno(errno, addr, len),
        };
    }
    match space.mmap_writeback(addr, len) {
        Ok((pages, bytes)) if pages > 0 => {
            serial::print("user: munmap writeback pages=");
            serial::print_dec(pages as u64);
            serial::print(" bytes=");
            serial::print_dec(bytes as u64);
            serial::println("");
        }
        Ok(_) => {}
        Err(EINVAL) => {}
        Err(errno) => {
            serial::print("user: munmap writeback errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::print(" addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::println(" (the mapping is still released)");
        }
    }
    match space.munmap(addr, len) {
        Ok(()) => {
            serial::print("user: munmap addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::println("");
            0
        }
        Err(errno) => munmap_errno(errno, addr, len),
    }
}

/// `munmap` 失败时的统一日志行与返回值（唯一出口，避免两条路径的文字漂移）。
fn munmap_errno(errno: i64, addr: u64, len: u64) -> i64 {
    serial::print("user: munmap errno=");
    serial::print_dec(errno.unsigned_abs());
    serial::print(" addr=");
    serial::print_hex(addr);
    serial::print(" len=");
    serial::print_dec(len);
    serial::print(
        " (must be page-aligned, inside the mmap window, fully mapped; a shared file mapping must be unmapped as a whole; max=",
    );
    serial::print_dec(USER_MMAP_MAX);
    serial::println(")");
    errno
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

/// 系统调用 `msync(addr, len, flags)`（v1.0 第五步）：把 `MAP_SHARED` 文件映射的页写回文件。
///
/// 语义取舍（都写在这里，串口上可断言）：
///
/// - **只处理共享文件页**：匿名页与 `MAP_PRIVATE` 文件页跳过（`pages=0 bytes=0`）——
///   它们本来就不该落盘。没有任何共享页的一段内存做 `msync` 是**合法**的（Linux 允许）；
/// - **`MS_ASYNC` 与 `MS_SYNC` 现在是同一件事**（同步写回，内核里没有回写线程），
///   `MS_INVALIDATE` 被接受并忽略（没有页缓存可以让别人失效）；
/// - **`flags` 至少要有一位 `MS_SYNC`/`MS_ASYNC`**（与 Linux 一致）；未知位一律 `EINVAL`，
///   不静默忽略；
/// - **范围必须页对齐、非空、落在 mmap 窗口内且每一页已映射**，缺一即 `EINVAL`
///   （与 `munmap`/`mprotect` 同一约定，而不是 Linux 的 `ENOMEM`）；`addr` 指向一段
///   **没有**共享文件页的映射（例如匿名映射）是合法的，返回 `pages=0`；
/// - 写回失败（文件系统只读、fd 已关等）返回 `vfs::write` 的 `-errno`：此时**可能**
///   已经写回了前面的页 —— 不回滚，串口上如实记录。
pub unsafe fn sys_msync(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let addr = args[0];
    let len = args[1];
    let flags = args[2];

    let reject = |reason: &str| -> i64 {
        serial::print("user: msync rejected (");
        serial::print(reason);
        serial::print(") addr=");
        serial::print_hex(addr);
        serial::print(" len=");
        serial::print_dec(len);
        serial::print(" flags=");
        serial::print_hex(flags);
        serial::println("");
        EINVAL
    };

    if flags & !MS_SUPPORTED != 0 {
        return reject("unknown MS_* bits");
    }
    if flags & (MS_SYNC | MS_ASYNC) == 0 {
        return reject("msync needs MS_SYNC or MS_ASYNC");
    }
    if addr % PAGE_SIZE != 0 {
        return reject("addr is not page-aligned");
    }
    if len == 0 {
        return reject("len=0");
    }

    let space = match unsafe { space_mut(space) } {
        Some(space) => space,
        None => {
            serial::println("user: msync rejected (no current user address space)");
            return EFAULT;
        }
    };

    match space.mmap_writeback(addr, len) {
        Ok((pages, bytes)) => {
            serial::print("user: msync addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" flags=");
            serial::print_hex(flags);
            serial::print(" -> pages=");
            serial::print_dec(pages as u64);
            serial::print(" bytes=");
            serial::print_dec(bytes as u64);
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: msync errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::print(" addr=");
            serial::print_hex(addr);
            serial::print(" len=");
            serial::print_dec(len);
            serial::print(" flags=");
            serial::print_hex(flags);
            serial::println(" (page-aligned, one window, fully mapped; only MAP_SHARED file pages are written back)");
            errno
        }
    }
}
