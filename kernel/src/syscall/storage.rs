//! 存储类系统调用（CSS-1：`storage_volumes` = 261、`storage_rescan` = 265）
//!
//! 这与 `getprocs`/`getdents` 是同一套做法：**定长记录 + 一次调用写满**，
//! 记录布局的权威定义在内核（这里是 `storage::volume`），libc 提供解析器。
//!
//! - [`sys_storage_volumes`] 是**只读**查询，不需要任何能力位（CKM 的只读面
//!   同样如此，见 `docs/storage.md` 6.2）；写入的条数受 `buf_len` 限制，装不下
//!   就截断（每条记录仍然完整）——`out_count_ptr` 用来问「总共有多少条」；
//! - [`sys_storage_rescan`] / [`sys_storage_mount`] / [`sys_storage_unmount`] /
//!   [`sys_storage_assign_letter`] 都是**受控动作**：CKM-1 起要求
//!   `CAP_STORAGE_ADMIN`（`docs/ckm.md` 3.4 判定点 8），拒绝时返回 `-EPERM`
//!   并留下审计 + 串口证据。
//!
//! CSS-3 把 262/263/264 从 ENOSYS 桩上拿下来（`docs/storage.md` 6.3）：
//!
//! | 号 | 调用 | 行为 | 失败语义 |
//! | ---- | ---- | ---- | ---- |
//! | 262 | `storage_mount` | 把卷挂到路径（已挂载的卷 → 只加别名，**不重开设备**） | `ENOENT` 卷不存在 / `ENOTSUP` 没后端 / `EEXIST` 挂载点重复 |
//! | 263 | `storage_unmount` | 卸掉别名，卷仍在卷表里 | `ENOENT` 不是挂载点 / `EINVAL` 卸根 |
//! | 264 | `storage_assign_letter` | 分配/取消盘符（`0` = 回到缺省规则） | `EEXIST` 字母被占 / `EINVAL` 非法字母 |
//!
//! 参数校验（长度、保留位、指针）**先于**能力判定：拒绝必须发生在副作用之前，
//! 但「参数不合法」与「没有权限」是两回事，顺序反了会让 `EINVAL` 变成 `EPERM`。

use alloc::string::String;

use crate::ckm::api as ckm;
use crate::ckm::policy::Op;
use crate::driver::serial;
use crate::fs::vfs::MountOptions;
use crate::storage::registry;
use crate::storage::volume::VOLUME_RECORD_SIZE;
use crate::syscall::address_space::UserSpace;

/// `-EINVAL`：缓冲区/参数非法；`-EIO`：扫描/读取设备失败。**定义在 `abi::errno`**。
use abi::errno::{EINVAL, EIO};

/// 系统调用 `storage_volumes(buf_ptr, buf_len, out_count_ptr, 0, 0)`。
///
/// 按确定性顺序（磁盘升序 → 分区升序 → `vol_id` 升序）把卷记录写进用户缓冲，
/// 返回**实际写入的记录数**；`out_count_ptr != 0` 时同一时刻把**总卷数**写成
/// 一个 LE `u64`（便于调用方先问大小、再决定缓冲）。
///
/// 返回负数 = `-errno`；`buf_len < 64` 返回 `-EINVAL`（至少要装下一条记录）。
pub unsafe fn sys_storage_volumes(args: &[u64; 5], space: *const UserSpace) -> i64 {
    let (buf_ptr, buf_len, count_ptr) = (args[0], args[1], args[2]);

    if (buf_len as usize) < VOLUME_RECORD_SIZE {
        return EINVAL;
    }
    let out = match unsafe {
        crate::syscall::file::user_bytes_mut(space, buf_ptr, buf_len, "storage_volumes")
    } {
        Ok(out) => out,
        Err(errno) => return errno,
    };

    // 快照到内核侧定长缓冲（卷表容量固定，且这个调用不是热路径）
    let mut scratch = [0u8; crate::storage::MAX_VOLUMES * VOLUME_RECORD_SIZE];
    let total = match registry::snapshot(&mut scratch) {
        Ok(n) => n,
        Err(_) => return EIO,
    };

    let cap = out.len() / VOLUME_RECORD_SIZE;
    let written = total.min(cap);
    for i in 0..written {
        let src = &scratch[i * VOLUME_RECORD_SIZE..(i + 1) * VOLUME_RECORD_SIZE];
        out[i * VOLUME_RECORD_SIZE..(i + 1) * VOLUME_RECORD_SIZE].copy_from_slice(src);
    }

    if count_ptr != 0 {
        let out_count = match unsafe {
            crate::syscall::file::user_bytes_mut(space, count_ptr, 8, "storage_volumes")
        } {
            Ok(out) => out,
            Err(errno) => return errno,
        };
        out_count.copy_from_slice(&(total as u64).to_le_bytes());
    }

    serial::print("user: storage_volumes count=");
    serial::print_dec(written as u64);
    serial::print(" total=");
    serial::print_dec(total as u64);
    serial::println("");
    written as i64
}

/// 系统调用 `storage_rescan(disk_index, 0, 0, 0, 0)`。
///
/// `disk_index == u32::MAX`（`0xFFFF_FFFF`）表示重扫全部设备；否则是块设备注册表
/// 序号。重扫只做**只读扫描**：先摘掉该磁盘的旧卷记录，再按当前内容重建并打印
/// 证据行。返回重扫后的卷数；设备不存在返回 `-ENODEV`。
pub unsafe fn sys_storage_rescan(args: &[u64; 5], _space: *const UserSpace) -> i64 {
    let index = args[0];
    if index > u32::MAX as u64 {
        return EINVAL;
    }
    let index = index as u32;

    // 判定点 8（`docs/ckm.md` 3.4）：重扫设备是受控动作，需要 `CAP_STORAGE_ADMIN`。
    // 参数校验之后、动任何设备状态之前判定 —— 拒绝必须发生在副作用之前。
    let errno = ckm::enforce(Op::StorageRescan, index as u64);
    if errno != 0 {
        serial::print("user: storage_rescan disk=");
        serial::print_dec(index as u64);
        serial::println(" -> EPERM (need CAP_STORAGE_ADMIN)");
        return errno;
    }

    if index == abi::volume::RESCAN_ALL {
        return match registry::scan_all(true) {
            Ok(count) => {
                serial::print("user: storage_rescan disk=all volumes=");
                serial::print_dec(count as u64);
                serial::println("");
                count as i64
            }
            Err(_) => EIO,
        };
    }

    if index as usize >= crate::driver::block::count() {
        serial::print("user: storage_rescan disk=");
        serial::print_dec(index as u64);
        serial::println(" -> ENODEV");
        return crate::fs::vfs::ENODEV;
    }
    match registry::rescan_disk(index as usize) {
        Ok(count) => {
            serial::print("user: storage_rescan disk=");
            serial::print_dec(index as u64);
            serial::print(" volumes=");
            serial::print_dec(count as u64);
            serial::println("");
            count as i64
        }
        Err(_) => EIO,
    }
}

// ---- CSS-3：挂载/卸载/盘符（262/263/264，`docs/storage.md` 6.3）----

/// 用户态字符串的长度上限（路径/卷标识都在 15–128 字节量级；超长一律 `EINVAL`）。
const STR_MAX: usize = 128;

/// 从用户态拷一个字符串（校验指针与 UTF-8；零长度也算 `EINVAL`）。
unsafe fn user_str(
    space: *const UserSpace,
    ptr: u64,
    len: u64,
    tag: &'static str,
) -> Result<String, i64> {
    if len == 0 || len as usize > STR_MAX {
        return Err(EINVAL);
    }
    let bytes = match unsafe { crate::syscall::file::user_bytes(space, ptr, len, tag) } {
        Ok(bytes) => bytes,
        Err(errno) => return Err(errno),
    };
    match core::str::from_utf8(bytes) {
        Ok(text) if !text.is_empty() => Ok(String::from(text)),
        _ => Err(EINVAL),
    }
}

/// 系统调用 `storage_mount(vol_id_ptr, vol_id_len, point_ptr, point_len, flags)`（262）。
///
/// `flags` bit0 = 只读挂载（→ 只读原因 `ro-mount`，`docs/storage.md` 3.5），
/// 其余位一律 `EINVAL`（未知标志不静默忽略：那是「明明要只读却写了」的来源）。
/// 已挂载的卷再加挂载点只加**别名**（同一份数据，CSS-2 的 `alias_volume`），
/// 因此「两个入口不是两份数据」在这条路径上也成立。
pub unsafe fn sys_storage_mount(args: &[u64; 5], space: *const UserSpace) -> i64 {
    let (vol_ptr, vol_len, point_ptr, point_len, flags) =
        (args[0], args[1], args[2], args[3], args[4]);
    if flags & !1 != 0 {
        return EINVAL;
    }
    let vol_id = match unsafe { user_str(space, vol_ptr, vol_len, "storage_mount") } {
        Ok(text) => text,
        Err(errno) => return errno,
    };
    let point = match unsafe { user_str(space, point_ptr, point_len, "storage_mount") } {
        Ok(text) => text,
        Err(errno) => return errno,
    };

    // 判定点 8：挂载是受控动作（`CAP_STORAGE_ADMIN`）。参数合法之后、动任何状态之前。
    let errno = ckm::enforce(Op::StorageMount, flags);
    if errno != 0 {
        serial::print("user: storage_mount vol=");
        serial::print(&vol_id);
        serial::print(" point=");
        serial::print(&point);
        serial::println(" -> EPERM (need CAP_STORAGE_ADMIN)");
        return errno;
    }

    let options = if flags & abi::volume::MOUNT_RDONLY != 0 {
        MountOptions::RO
    } else {
        MountOptions::RW
    };
    match crate::fs::mount_volume_by_id(&vol_id, &point, options) {
        Ok(()) => {
            serial::print("user: storage_mount vol=");
            serial::print(&vol_id);
            serial::print(" point=");
            serial::print(&point);
            serial::print(" ro=");
            serial::println(if options.read_only { "ro-mount" } else { "rw" });
            0
        }
        Err(errno) => {
            serial::print("user: storage_mount vol=");
            serial::print(&vol_id);
            serial::print(" point=");
            serial::print(&point);
            serial::print(" -> errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::println("");
            errno
        }
    }
}

/// 系统调用 `storage_unmount(point_ptr, point_len, 0, 0, 0)`（263）。
///
/// 卸载的是**别名**：卷仍在卷表里（`mount_count` 减一），再次挂载不需要重新探测
/// 设备。卸下加载点 `/` 返回 `EINVAL`（那是系统唯一的根文件系统）。
pub unsafe fn sys_storage_unmount(args: &[u64; 5], space: *const UserSpace) -> i64 {
    let (point_ptr, point_len, a, b, c) = (args[0], args[1], args[2], args[3], args[4]);
    if a != 0 || b != 0 || c != 0 {
        return EINVAL;
    }
    let point = match unsafe { user_str(space, point_ptr, point_len, "storage_unmount") } {
        Ok(text) => text,
        Err(errno) => return errno,
    };

    // 判定点 8：卸载同为受控动作
    let errno = ckm::enforce(Op::StorageUnmount, 0);
    if errno != 0 {
        serial::print("user: storage_unmount point=");
        serial::print(&point);
        serial::println(" -> EPERM (need CAP_STORAGE_ADMIN)");
        return errno;
    }

    match crate::fs::unmount_point(&point) {
        Ok(()) => {
            serial::print("user: storage_unmount point=");
            serial::print(&point);
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: storage_unmount point=");
            serial::print(&point);
            serial::print(" -> errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::println("");
            errno
        }
    }
}

/// 系统调用 `storage_assign_letter(vol_id_ptr, vol_id_len, letter, 0, 0)`（264）。
///
/// `letter` 是 ASCII（`'A'..='Z'`）；`0` 表示**取消自定义映射、回到缺省规则**
/// （不是「没有盘符」—— 缺省规则会立刻补一个）。字母被别的卷占着 -> `EEXIST`。
/// 映射写进 CKM 状态区（`storage/letters`），因此重启后字母不变。
pub unsafe fn sys_storage_assign_letter(args: &[u64; 5], space: *const UserSpace) -> i64 {
    let (vol_ptr, vol_len, letter, a, b) = (args[0], args[1], args[2], args[3], args[4]);
    if a != 0 || b != 0 || letter > u8::MAX as u64 {
        return EINVAL;
    }
    let vol_id = match unsafe { user_str(space, vol_ptr, vol_len, "storage_assign_letter") } {
        Ok(text) => text,
        Err(errno) => return errno,
    };

    // 判定点 8：改字母表是受控动作（与挂载同一能力位，但它是**持久化**动作）
    let errno = ckm::enforce(Op::StorageLetter, letter);
    if errno != 0 {
        serial::print("user: storage_assign_letter vol=");
        serial::print(&vol_id);
        serial::println(" -> EPERM (need CAP_STORAGE_ADMIN)");
        return errno;
    }

    match registry::assign_letter(&vol_id, letter as u8) {
        Ok(()) => {
            serial::print("user: storage_assign_letter vol=");
            serial::print(&vol_id);
            serial::print(" letter=");
            serial::print(crate::storage::letters::ascii(&(letter as u8)));
            serial::print(" -> letter=");
            serial::print(crate::storage::letters::ascii(&registry::letter_of(&vol_id)));
            serial::println("");
            0
        }
        Err(errno) => {
            serial::print("user: storage_assign_letter vol=");
            serial::print(&vol_id);
            serial::print(" -> errno=");
            serial::print_dec(errno.unsigned_abs());
            serial::println("");
            errno
        }
    }
}
