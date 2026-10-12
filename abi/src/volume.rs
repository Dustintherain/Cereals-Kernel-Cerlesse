//! 存储卷记录与 `storage_*` 的参数取值（**权威布局**，内核 `storage::volume`
//! 与 libc 都引用这里；见 `docs/storage.md` 3.4）。
//!
//! ```text
//!  0  id(16, NUL 填充)      16  disk_id(u16 LE)   18  part_index(1)
//! 19  fs_kind(1)            20  read_only(1)       21  health(1)
//! 22  flags(u16 LE)         24  letter(1)          25  label(11)
//! 36  uuid(4)               40  total_bytes(u64 LE)
//! 48  free_bytes(u64 LE)    56  mounts(u32 LE)     60  reserved(4, = 0)
//! ```
//!
//! `flags` 位：0 可移动介质 / 1 有卷自带 UUID / 2 `free_bytes` 未知 / 3 卷标被截断。

/// 单条卷记录的字节数。
pub const RECORD_SIZE: usize = 64;
/// `id` 字段里可读字符的上限（记录占 16 字节：15 字符 + NUL）。
pub const ID_MAX: usize = 15;
/// 卷标字段容量（11 字节，FAT 风格）。
pub const LABEL_MAX: usize = 11;
/// `free_bytes` 的 unknown 哨兵（0 会让「盘满」与「没实现」无法区分）。
pub const FREE_UNKNOWN: u64 = u64::MAX;
/// `storage_mount` 的 `flags` bit0：只读挂载（→ 只读原因 `ro-mount`）。
pub const MOUNT_RDONLY: u64 = 1;
/// `storage_assign_letter` 的「取消」取值（回到缺省规则，而不是「没有盘符」）。
pub const LETTER_NONE: u8 = 0;
/// `storage_rescan` 的「全部设备」下标。
pub const RESCAN_ALL: u32 = u32::MAX;
