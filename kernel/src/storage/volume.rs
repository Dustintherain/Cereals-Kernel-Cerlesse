//! 卷对象、文件系统探测与稳定卷标识（CSS-1 实现，见 `docs/storage.md` 第 3 节）
//!
//! 这一层回答三个问题，全部**只读**：
//!
//! 1. 某个分区（或整盘）里是什么文件系统？—— [`probe`]（BPB / superblock / 魔数）；
//! 2. 这个卷的稳定标识是什么？—— [`make_volume`]（`fs:` > `pt:` > `raw:`，不用随机数）；
//! 3. 用户态看到什么？—— [`Volume::write_record`]（定长 64 字节，布局是 ABI）。
//!
//! 只读原因与健康状态都是**枚举**而不是布尔（`docs/storage.md` 3.5 / 3.6）：
//! 「不能写」和「为什么不能写」、「读得到」和「可信吗」是两组不同的问题。
//!
//! 现状（CSS-1）只有 FAT 后端是真实后端（且只读），其余格式「探测得到、没有后端」——
//! 这类卷仍然进卷表（`health = unsupported`），因为「盘在、格式不认识」必须与
//! 「盘不在」可区分（4.5 的 `ENOTSUP` 与 `ENOENT`）。

use alloc::format;
use alloc::string::String;

use crate::driver::block::{BlockDevice, SECTOR_SIZE};
use crate::driver::serial;

/// 卷记录的字节数与字段宽度（**再导出**，定义在 `abi::volume` —— 阶段三起
/// 「记录长什么样」的权威定义在 ABI 单一事实源，内核是写记录的一方、不是定义方）。
pub use abi::volume::{
    FREE_UNKNOWN, ID_MAX as VOL_ID_MAX, LABEL_MAX, RECORD_SIZE as VOLUME_RECORD_SIZE,
};

/// 探测到的文件系统类型（`fs_kind` 的取值就是 `code()`）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    /// 没识别出来（或识别出来但不在下表枚举里）。
    Unsupported = 0,
    Fat12 = 1,
    Fat16 = 2,
    Fat32 = 3,
    ExFat = 4,
    Ext2 = 5,
    Ext4 = 6,
    Ntfs = 7,
    Iso9660 = 8,
}

impl FsKind {
    /// 记录里的数值。
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 串口证据里的短名。
    pub fn name(self) -> &'static str {
        match self {
            FsKind::Unsupported => "unsupported",
            FsKind::Fat12 => "fat12",
            FsKind::Fat16 => "fat16",
            FsKind::Fat32 => "fat32",
            FsKind::ExFat => "exfat",
            FsKind::Ext2 => "ext2",
            FsKind::Ext4 => "ext4",
            FsKind::Ntfs => "ntfs",
            FsKind::Iso9660 => "iso9660",
        }
    }

}

/// 只读原因（不是布尔：用户必须能问出「为什么只读」）。
///
/// `#[allow(dead_code)]`：取值域（0..=4）是记录 ABI 的一部分，CSS-1 只会产生
/// `ro-backend`，其余三个值在 CSS-3（`ro-mount`/`ro-device`）与 CSS-4（`ro-dirty`）
/// 落地 —— 枚举先完整定义，避免以后改 ABI 数值。
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// 可写。
    Rw = 0,
    /// 后端只实现了只读（现状 FAT；未来未实现写的格式同此）。
    RoBackend = 1,
    /// 挂载参数要求只读（CSS-3）。
    RoMount = 2,
    /// 设备只读。
    RoDevice = 3,
    /// 检测到未完成写入的痕迹（CSS-4）。
    RoDirty = 4,
}

impl ReadOnlyReason {
    /// 记录里的数值。
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 串口证据里的短名。
    pub fn name(self) -> &'static str {
        match self {
            ReadOnlyReason::Rw => "rw",
            ReadOnlyReason::RoBackend => "ro-backend",
            ReadOnlyReason::RoMount => "ro-mount",
            ReadOnlyReason::RoDevice => "ro-device",
            ReadOnlyReason::RoDirty => "ro-dirty",
        }
    }
}

/// 卷健康状态（枚举）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// 探测与校验全过。
    Ok = 0,
    /// 结构可疑但可读（GPT CRC 失败、`vol_id` 冲突、目录项非法……）。
    Degraded = 1,
    /// 设备消失 / 读扇区失败。
    Offline = 2,
    /// 探测到文件系统但内核没有对应后端。
    Unsupported = 3,
}

impl Health {
    /// 记录里的数值。
    pub fn code(self) -> u8 {
        self as u8
    }

    /// 串口证据里的短名。
    pub fn name(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Degraded => "degraded",
            Health::Offline => "offline",
            Health::Unsupported => "unsupported",
        }
    }
}

/// 文件系统自带的 UUID/序列号（前缀）。`len` 是**有意义**的字节数：
/// FAT/exFAT/NTFS 的序列号是 4 字节，ext 的 UUID 取前 6 字节（够放进 15 字符）。
#[derive(Clone, Copy)]
pub struct FsUuid {
    pub bytes: [u8; 6],
    pub len: u8,
}

impl FsUuid {
    fn from_bytes(src: &[u8], len: usize) -> Self {
        let mut bytes = [0u8; 6];
        let take = len.min(src.len()).min(6);
        bytes[..take].copy_from_slice(&src[..take]);
        Self { bytes, len: take as u8 }
    }
}

/// 稳定卷标识（`vol_id`）：`fs:…` / `pt:…` / `raw:…`。
///
/// 规则见 `docs/storage.md` 3.4：优先用文件系统自带 UUID/序列号，其次用
/// 「磁盘号-分区号-起始 LBA」，最后退化到整盘。生成规则是**确定性**的：
/// 不用随机数、不读时钟，同一镜像两次启动逐字节相同。
#[derive(Clone, Copy)]
pub struct VolId {
    bytes: [u8; 16],
    len: u8,
}

impl VolId {
    /// 校验并构造：`[A-Za-z0-9._:-]{1,15}`（记录里 NUL 填充）。
    pub fn new(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > VOL_ID_MAX {
            return None;
        }
        let ok = bytes.iter().all(|&b| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || b == b':'
        });
        if !ok {
            return None;
        }
        let mut out = [0u8; 16];
        out[..bytes.len()].copy_from_slice(bytes);
        Some(Self { bytes: out, len: bytes.len() as u8 })
    }

    /// 可读形式（`fs:1a2b3c4d`）。
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }

    /// 原始字节（记录里 NUL 填充的 16 字节字段）。
    fn field(&self) -> &[u8; 16] {
        &self.bytes
    }
}

/// 卷（卷管理器内部引用的对象；`id` 才是身份，其余字段可随插拔改变）。
pub struct Volume {
    /// 稳定标识。
    pub id: VolId,
    /// 所属块设备（注册表序号）。
    pub disk_id: u16,
    /// 分区序号（无分区表 = 0，表示整盘一个卷）。
    pub part_index: u8,
    /// 探测到的文件系统。
    pub fs: FsKind,
    /// 文件系统自带 UUID/序列号（若存在）。
    pub uuid: Option<FsUuid>,
    /// 用户可见卷标（已按 `[A-Za-z0-9._-]` 规范化）。
    pub label: [u8; LABEL_MAX],
    /// 卷标有效长度。
    pub label_len: u8,
    /// 卷标是否被截断（超过 11 字节时必须置位，而不是静默截断）。
    pub label_truncated: bool,
    /// 只读原因。
    pub read_only: ReadOnlyReason,
    /// 健康状态。
    pub health: Health,
    /// 容量（字节）。
    pub total_bytes: u64,
    /// 可用容量（`None` = unknown，不许用 0 冒充）。
    pub free_bytes: Option<u64>,
    /// 当前挂载点数（CSS-2 起由挂载表维护：挂载/卸载后 `registry::set_mount_count`
    /// 写入，重扫后由 `MountTable::sync_counts` 重放；新建的卷从 0 开始）。
    pub mount_count: u32,
    /// 盘符（**CSS-3**：`0` = 未分配，否则 `'A'..='Z'` 的 ASCII）。
    ///
    /// 盘符是别名而不是身份（3.1）：映射住在 `letters`（字母 → `vol_id`），
    /// 这个字段是**给消费者的视图**（记录偏移 24），扫描之后由
    /// `registry::apply_letters` 从字母表灌进来。
    pub letter: u8,
    /// 是否可移动介质（CSS-1 未知，恒 false）。
    pub removable: bool,
}

impl Volume {
    /// 写一条 64 字节卷记录（布局见 `docs/storage.md` 6.4，**改它就是改 ABI**）。
    pub fn write_record(&self, out: &mut [u8]) -> Result<(), &'static str> {
        if out.len() < VOLUME_RECORD_SIZE {
            return Err("storage: volume record buffer too small");
        }
        let rec = &mut out[..VOLUME_RECORD_SIZE];
        for byte in rec.iter_mut() {
            *byte = 0;
        }
        rec[0..16].copy_from_slice(self.id.field());
        rec[16..18].copy_from_slice(&self.disk_id.to_le_bytes());
        rec[18] = self.part_index;
        rec[19] = self.fs.code();
        rec[20] = self.read_only.code();
        rec[21] = self.health.code();
        let mut flags: u16 = 0;
        if self.removable {
            flags |= 1 << 0;
        }
        if self.uuid.is_some() {
            flags |= 1 << 1;
        }
        if self.free_bytes.is_none() {
            flags |= 1 << 2;
        }
        if self.label_truncated {
            flags |= 1 << 3;
        }
        rec[22..24].copy_from_slice(&flags.to_le_bytes());
        // 盘符（CSS-3）：`0` = 未分配。字母是别名，`vol_id` 才是身份（3.1）
        rec[24] = self.letter;
        rec[25..25 + LABEL_MAX].copy_from_slice(&self.label);
        if let Some(uuid) = self.uuid {
            // 记录里只放前 4 字节（完整前缀在 id 里已经可读）
            rec[36..40].copy_from_slice(&uuid.bytes[..4]);
        }
        rec[40..48].copy_from_slice(&self.total_bytes.to_le_bytes());
        rec[48..56].copy_from_slice(&self.free_bytes.unwrap_or(FREE_UNKNOWN).to_le_bytes());
        rec[56..60].copy_from_slice(&self.mount_count.to_le_bytes());
        Ok(())
    }

    /// 卷标（可读形式；空卷标返回 `None`）。
    pub fn label_str(&self) -> Option<&str> {
        if self.label_len == 0 {
            return None;
        }
        core::str::from_utf8(&self.label[..self.label_len as usize]).ok()
    }

    /// 串口证据行（`storage: vol …`，见 `docs/storage.md` 6.7）。
    pub fn log(&self) {
        serial::print("storage: vol id=");
        serial::print(self.id.as_str());
        serial::print(" disk=");
        serial::print_dec(self.disk_id as u64);
        serial::print(" part=");
        serial::print_dec(self.part_index as u64);
        serial::print(" fs=");
        serial::print(self.fs.name());
        serial::print(" label=");
        match self.label_str() {
            Some(label) => serial::print(label),
            None => serial::print("-"),
        }
        serial::print(" letter=");
        if self.letter == 0 {
            serial::print("-");
        } else {
            serial::print(crate::storage::letters::ascii(&self.letter));
        }
        serial::print(" mounts=");
        serial::print_dec(self.mount_count as u64);
        serial::print(" ro=");
        serial::print(self.read_only.name());
        serial::print(" health=");
        serial::println(self.health.name());
    }
}

/// 一次探测的结果（文件系统类型 + 自带标识 + 可疑标记）。
pub struct Probe {
    /// 探测到的文件系统。
    pub fs: FsKind,
    /// UUID/序列号（若存在）。
    pub uuid: Option<FsUuid>,
    /// 卷标。
    pub label: [u8; LABEL_MAX],
    /// 卷标有效长度。
    pub label_len: u8,
    /// 卷标是否被截断。
    pub label_truncated: bool,
    /// 容量（BPB/分区大小）。
    pub total_bytes: u64,
    /// 探测到结构可疑（如 FAT 几何不自洽），仍可继续。
    pub degraded: bool,
    /// 读扇区失败（设备消失 / I/O 错误）。
    pub read_failed: bool,
}

impl Probe {
    fn empty(total_bytes: u64) -> Self {
        Self {
            fs: FsKind::Unsupported,
            uuid: None,
            label: [0; LABEL_MAX],
            label_len: 0,
            label_truncated: false,
            total_bytes,
            degraded: false,
            read_failed: false,
        }
    }
}

/// 按 `vol_id` 规则构造一个卷（确定性；见 `docs/storage.md` 3.4）。
///
/// `table_degraded` 来自分区表解析（GPT CRC 失败等）：表可疑则卷也可疑。
pub fn make_volume(
    disk_id: u16,
    part_index: u8,
    start_lba: u64,
    probe: &Probe,
    table_degraded: bool,
) -> Volume {
    let id_text = match probe.uuid {
        Some(uuid) => format!("fs:{}", hex(&uuid.bytes[..uuid.len as usize])),
        None if part_index > 0 => {
            let full = format!("pt:{disk_id}-{part_index}-{start_lba}");
            if full.len() <= VOL_ID_MAX {
                full
            } else {
                // 15 字符预算放不下「磁盘-分区-起 LBA」时的确定性退化：
                // 保留磁盘/分区（它们已保证同一时刻唯一），LBA 换成其十进制字符串的 CRC16。
                // 规则本身仍是确定性的（同一输入 → 同一 id），只是不再直接可读。
                let crc = crate::storage::crc32(full.as_bytes()) & 0xFFFF;
                format!("pt:{disk_id}-{part_index}-{crc:04x}")
            }
        }
        None => format!("raw:{disk_id}"),
    };
    let id = VolId::new(&id_text).expect("vol_id 由构造规则保证合法");
    // `health` 的顺序与 `docs/storage.md` 3.6 一致：
    //   `offline`（设备没了）> `unsupported`（**这个卷用不了**：要么没识别出格式，
    //   要么识别出了但内核没有后端）> `degraded`（结构可疑，但能用）> `ok`。
    // `unsupported` 优先于 `degraded` 的理由：「用不了」是更终局的答案，
    // 表可疑（degraded）只影响「能不能信」，不影响「有没有后端」——两者都不许丢，
    // 所以磁盘证据行照样打 `health=degraded`（见 registry）。
    let backend_supported = matches!(probe.fs, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32);
    let health = if probe.read_failed {
        Health::Offline
    } else if !backend_supported {
        Health::Unsupported
    } else if probe.degraded || table_degraded {
        Health::Degraded
    } else {
        Health::Ok
    };
    Volume {
        id,
        disk_id,
        part_index,
        fs: probe.fs,
        uuid: probe.uuid,
        label: probe.label,
        label_len: probe.label_len,
        label_truncated: probe.label_truncated,
        // CSS-1 的所有后端都只读（FAT）或不存在（其余格式，`health=unsupported`）。
        read_only: ReadOnlyReason::RoBackend,
        health,
        total_bytes: probe.total_bytes,
        free_bytes: None,
        // 探测阶段还不知道挂载表的状态；已挂载的卷由 `registry::scan_all` 末尾的
        // `MountTable::sync_counts()` 重放回真实计数（CSS-2）。
        mount_count: 0,
        // 盘符同理：扫描完成后由 `registry::apply_letters` 从字母表灌进来（CSS-3）。
        letter: 0,
        removable: false,
    }
}

/// 探测一个偏移上的文件系统（只读；不挂载、不修改任何字节）。
///
/// 顺序固定（`docs/storage.md` 5.3）：FAT → EXFAT → EXT2/EXT4 → NTFS → ISO9660。
/// 命中多个当前不会发生（各格式的识别特征互斥）；命中不了就是 `unsupported`。
///
/// **FAT 这一支由后端自己回答**（`fs::fat::probe`，CSS-2）：BPB 的规则只能有一处，
/// 否则「内核接受但挂不上」与「挂得上但内核认不出」迟早同时出现。
pub fn probe(dev: &mut dyn BlockDevice, start_lba: u64, sectors: u64) -> Probe {
    let capacity = sectors.saturating_mul(SECTOR_SIZE as u64);

    match crate::fs::fat::probe(dev, start_lba, sectors) {
        crate::fs::fat::ProbeOutcome::ReadFailed => {
            let mut probe = Probe::empty(capacity);
            probe.read_failed = true;
            return probe;
        }
        crate::fs::fat::ProbeOutcome::Fat(hit) => {
            let mut probe = Probe::empty(hit.total_bytes);
            probe.fs = fat_kind(hit.geometry.fat_type);
            // 序列号按 LE 字节原样进记录（与 CSS-1 逐字节相同：raw → u32 → raw）
            probe.uuid = Some(FsUuid::from_bytes(&hit.serial.to_le_bytes(), 4));
            copy_label(&hit.label, &mut probe);
            return probe;
        }
        crate::fs::fat::ProbeOutcome::NotFat => {}
    }

    let mut boot = [0u8; SECTOR_SIZE as usize];
    if read_at(dev, start_lba, &mut boot).is_err() {
        let mut probe = Probe::empty(capacity);
        probe.read_failed = true;
        return probe;
    }

    if &boot[3..11] == b"EXFAT   " {
        let mut probe = Probe::empty(capacity);
        probe.fs = FsKind::ExFat;
        probe.uuid = Some(FsUuid::from_bytes(&boot[0x64..0x68], 4));
        return probe;
    }
    if &boot[3..11] == b"NTFS    " {
        let mut probe = Probe::empty(capacity);
        probe.fs = FsKind::Ntfs;
        probe.uuid = Some(FsUuid::from_bytes(&boot[0x48..0x4C], 4));
        return probe;
    }

    // ext2/4：superblock 在卷内偏移 1024，魔数 0xEF53 在 +0x38。
    // 一次读 3 个扇区（1536 字节）就能覆盖 magic / UUID / label。
    let mut sb = [0u8; 3 * SECTOR_SIZE as usize];
    if read_at(dev, start_lba, &mut sb).is_ok() {
        let magic = u16::from_le_bytes([sb[1024 + 0x38], sb[1024 + 0x39]]);
        if magic == 0xEF53 {
            let incompat = u32::from_le_bytes([
                sb[1024 + 0x60],
                sb[1024 + 0x61],
                sb[1024 + 0x62],
                sb[1024 + 0x63],
            ]);
            // ext4 的特性位（EXTENTS / 64BIT / FLEX_BG）——只影响名字，不影响「先只读」。
            let ext4 = incompat & (0x40 | 0x80 | 0x200) != 0;
            let mut probe = Probe::empty(capacity);
            probe.fs = if ext4 { FsKind::Ext4 } else { FsKind::Ext2 };
            probe.uuid = Some(FsUuid::from_bytes(&sb[1024 + 0x68..1024 + 0x78], 6));
            copy_label(&sb[1024 + 0x78..1024 + 0x88], &mut probe);
            return probe;
        }
    }

    // ISO9660：主卷描述符在卷内扇区 16，偏移 1 起是 "CD001"。
    let mut pvd = [0u8; SECTOR_SIZE as usize];
    if read_at(dev, start_lba + 16, &mut pvd).is_ok() && &pvd[1..6] == b"CD001" {
        let mut probe = Probe::empty(capacity);
        probe.fs = FsKind::Iso9660;
        copy_label(&pvd[40..72], &mut probe);
        return probe;
    }

    Probe::empty(capacity)
}

/// FAT 类型 → 记录的 `fs_kind`（两个枚举的映射放在这里，避免各自 `as u8`）。
fn fat_kind(fat_type: crate::fs::fat::FatType) -> FsKind {
    match fat_type {
        crate::fs::fat::FatType::Fat12 => FsKind::Fat12,
        crate::fs::fat::FatType::Fat16 => FsKind::Fat16,
        crate::fs::fat::FatType::Fat32 => FsKind::Fat32,
    }
}

/// 把一段原始卷标规范化：去尾部空格/NUL，非法字节替换成 `.`/`_`，超长置截断位。
/// 返回（规范化字节、有效长度、是否被截断）。
///
/// **公开的原因**：CSS-2 的挂载自测要把「后端报的 `volume_info()`」与「记录里的卷标」
/// 对起来（`fs::storage_mount_selftest`）—— 两边必须用同一条规范化规则，
/// 否则比的是两种东西（也就无法证明记录布局被消费者读对了）。
pub fn normalize_label(src: &[u8]) -> ([u8; LABEL_MAX], u8, bool) {
    let mut end = src.len();
    while end > 0 && (src[end - 1] == b' ' || src[end - 1] == 0) {
        end -= 1;
    }
    let mut label = [0u8; LABEL_MAX];
    let mut count = 0usize;
    let mut truncated = false;
    for &byte in &src[..end] {
        let ch = match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => byte,
            b' ' => b'_',
            _ => b'.',
        };
        if count == LABEL_MAX {
            truncated = true;
            break;
        }
        label[count] = ch;
        count += 1;
    }
    (label, count as u8, truncated)
}

/// 把一段原始卷标规范化进 [`Probe`]。
fn copy_label(src: &[u8], probe: &mut Probe) {
    let (label, len, truncated) = normalize_label(src);
    probe.label = label;
    probe.label_len = len;
    probe.label_truncated = truncated;
}

/// `[u8]` → 小写十六进制（无前缀，无分隔）。
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0F) as usize] as char);
    }
    out
}

/// 从 `lba` 读满一个扇区。
fn read_at(dev: &mut dyn BlockDevice, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    match dev.read(lba, buf) {
        Ok(got) if got == buf.len() => Ok(()),
        _ => Err("storage: short read while probing"),
    }
}
