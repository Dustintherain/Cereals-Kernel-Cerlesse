# Cerlesse Storage System（CSS）设计提案

- 阶段：跨阶段 / 存储管理（磁盘 → 分区 → 卷 → 盘符/挂载点；执行上排在 v1.0 之后，与 CKM 同期）
- 最后更新：2026-10-12
- 状态：🟡 **CSS-1 / CSS-2 已落地，CSS-3 大部分落地，其余规划中** —— 三项规格已定稿：**卷模型（第 3 节）**、
  **路径/盘符/挂载（第 4 节）**、**VFS 收口与 CKM 接口（第 5–6 节）**；实现按第 9 节的
  CSS-1 … CSS-6 分期推进。**CSS-1（底层识别）已落地**：`kernel/src/storage/`
  （`mod`/`registry`/`partition`/`gpt`/`mbr`/`volume`/`selftest`）+ `BlockDevice` 注册表 2 → 8 +
  `tools/mkfs/mkfs.py` 的 GPT/MBR 镜像生成 + `storage_volumes`(261)/`storage_rescan`(265) +
  卷记录（64 字节）+ 内核解析自测；
  **CSS-2（VFS 收口）已落地**：`FileSystem::caps`/`volume_info`（RAMFS 可写 / FAT 只读 + 大小写不敏感 + 卷标）、
  FAT 探测唯一入口 `fs::fat::probe`（`storage::volume` 不再抄一份 BPB 规则）、挂载表拆分「后端 / 挂载点」
  （同一卷可多挂载点）、挂载点带 `vol_id`/`options`、挂载与卸载改 **errno**（`EEXIST`/`ENOSPC`/`ENOENT`/`EINVAL`/
  `ENOTSUP`/`EUCLEAN`）、`fs: stat` 行尾带 `vol=`、新增 `fs::storage_mount_selftest`；
  **CSS-3（盘符）大部分落地（2026-10-12）**：盘符表 `storage/letters.rs`（确定规则 `C`/`D`/`E`… +
  自定义映射持久化到 CKM 状态区键 `storage/letters`，跨重启不漂移）、`storage_mount`(262)/
  `storage_unmount`(263)/`storage_assign_letter`(264) 三个号 + `CAP_STORAGE_ADMIN`/判定点 8、
  `libc::path` 的 `C:\` → POSIX 重写、`/bin/volumes` 的 `list`/`mount`/`unmount`/`letter`/`demo` 子命令；
  **仍缺**：`/storage/{volumes,partitions,letters}` 只读状态树、卷标（label 只读展示已有，改名/持久化未做）、
  可写文件系统（CSS-4）、管理界面（CSS-5）
- 关联：[../DEVELOPMENT.md](../DEVELOPMENT.md)（H-08 需求行）· [docs/README.md](./README.md)（索引）·
  [docs/filesystem.md](./filesystem.md)（VFS/RAMFS/FAT 现状）· [docs/ckm.md](./ckm.md)（权限模型、能力位、判定点、号段）·
  [docs/drivers.md](./drivers.md)（块设备与 virtio-blk）· [docs/syscall.md](./syscall.md)（syscall ABI 与路径语义）·
  [docs/project-structure.md](./project-structure.md)（目录归属）· [docs/package-manager.md](./package-manager.md)（CKM 的包格式层）

> 本文档顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。
> 约定：**文档即规格** —— 带「必须」的条款是参考实现要照着做并被验收的条款，
> 其余是理由与开放问题。不写「大概」「可能」这类无法验收的表述。
>
> 第 3/4 节的 `C:`/`D:`/`D:\Projects\demo.rs` 中的**字母与 `C:\` → POSIX 重写**已随 CSS-3 落地，
> 但表里的 `ext4`/`NTFS`/`exFAT` 后端与 `/volumes/data`、`/media/usb` 这类路径仍是**规划示例**
> （现状见第 2 节；第一版可挂载的后端只有 FAT 与 RAMFS）。

## 0. 一句话、分层与边界

CSS 的目标是一句话：**像 Windows 一样管理磁盘与卷（盘符、卷标、卷 GUID/稳定标识），
像 Linux 一样通过统一 VFS 兼容多种文件系统（挂载点、多后端、统一文件接口）。**

```text
CSS Manager（管理界面）              用户态、无特权：列出磁盘/分区/卷、发起操作、看健康状态
   ckm storage（CLI，第一版唯一界面）  ↕ 受内核校验的接口（CKM 能力位）
卷管理与路径解析层                    内核内：卷对象、稳定标识、盘符/卷标、多挂载点、路径别名
   kernel/src/storage/                ↕ 唯一接入面：FileSystem trait + 挂载表
Cerlesse VFS                          统一文件操作、目录、文件描述符、挂载点路由、访问控制
   kernel/src/fs/{vfs,mount,inode,file}.rs
Windows 兼容格式   FAT12/16/32（现状：只读）、exFAT（规划）、NTFS（规划：只读）
Linux 兼容格式     ext2（规划：读写）、ext4（规划：只读）、XFS / Btrfs（后续）
特殊文件系统       ISO9660（规划：只读）
内存文件系统       RAMFS（现状：v0.7 已实现）
块设备与分区层     PCI → virtio-blk（现状）→ 磁盘设备 → GPT/MBR 分区表（规划）
   kernel/src/driver/block.rs + kernel/src/storage/{partition,gpt,mbr}.rs
```

三条边界（写下来避免误读）：

1. **盘符不属于文件系统驱动**：`fs/*` 的后端（`ramfs`、`fat`、未来的 `exfat`/`ext2`…）**不认识**
   `C:`、卷标或挂载历史，它们只实现 `FileSystem`；盘符与卷标由 `storage/` 的卷表维护。
2. **CSS 不重写 VFS**：`fs/vfs.rs::FileSystem`（v0.7 定义、v0.8 已被第二个后端验证）是**唯一**接入面；
   新文件系统是「再实现一个 `FileSystem`」，不是「改 VFS 或加一条盘符分支」。
3. **第一版以只读识别为主**：分区表**只扫描不修改**（第 11 节），写能力按 CSS-4 逐格式开放（第 7 节）。

## 1. 目标、验收面与非目标

| 目标 | 落成什么条款 | 在哪一节 |
| ---- | ---- | ---- |
| **磁盘/分区识别** | `BlockDevice` → 分区扫描（GPT/MBR）→ 分区对象 + 确定性枚举顺序 | 3.1 / 3.3 |
| **卷与稳定标识** | `Volume` 记录（`id` = 稳定标识，盘符只是别名）+ 生成规则（不依赖盘符、不用随机数） | 3.2 / 3.4 |
| **Windows 式管理** | 盘符 `C:`…、卷标、多挂载点、路径别名；`C:\x` 与 `/volumes/x` 指向同一文件 | 4 |
| **Linux 式文件系统** | 统一的 `FileSystem` 接入面 + 文件系统探测（魔数）+ 能力/只读原因声明 | 5 |
| **与 CKM 集成** | 2 个新能力位、3 个新判定点、号段 261–266、卷/分区两份定长记录、`/storage/**` 状态树 | 6 |
| **可验收** | 第 10 节把 11 条落成串口断言 / 主机侧断言 / QEMU 场景名（`make test-storage`） | 10 |

**非目标**（第一版明确不做，理由见第 11 节）：写分区表、无损调整/迁移分区、RAID/LVM/加密卷/快照、
磁盘配额、网络文件系统、GUI。

## 2. 现状审计（每条都有据）

| 项 | 现状 | 证据 | CSS 需要它吗 |
| ---- | ---- | ---- | ---- |
| 块设备抽象 | ✅ 已有：`BlockDevice`（`name`/`sector_size`/`sectors`/`read`/`write`/`capacity_bytes`），扇区固定 512，`lba` 是**整盘绝对地址** | `kernel/src/driver/block.rs`（模块文档明写「v0.8 没有分区表解析」） | ✅ 下层不动，CSS-1 在它之上加分区分层 |
| 设备注册表容量 | **已扩到 8**（`MAX_BLOCK_DEVICES = 8`）；固定容量数组 + 满了明确拒绝 | `driver/block.rs` | ✅ CSS-1 已完成（2 → 8） |
| 分区表 | ✅ **已有**：GPT（主/备表头 + 两份 CRC32 + 128 项表项）与 MBR（4 主分区 + EBR 链）**只读扫描**；主表头损坏回退备用表头并在证据行标出用的是哪张表 | `kernel/src/storage/{gpt,mbr,partition}.rs` + `make test-storage` | ✅ CSS-1 已交付（写表仍属非目标，见第 11 节） |
| 挂载方式 | ✅ **CSS-2 已改**：`fs::mount_disks` 仍把 `dev(0)`→`/disk`、`dev(1)`→`/mnt/fat` 作为**兼容默认挂载点**，但现在经 `fs::mount::MountTable::mount_volume` 挂载并带上卷身份（`vol_id` 取自 CSS-1 的卷表）；分区里的 FAT 仍拿不到卷身份（FAT 后端还不接受分区偏移），退化为非卷挂载而不伪造 id | `kernel/src/fs/mod.rs::mount_disks`、`fs/mount.rs` | ✅ CSS-2 已交付（按卷身份挂载 + 兼容点保留） |
| 挂载错误语义 | ✅ **CSS-2 已改**：`MountTable::mount/mount_volume/alias_volume/umount` 全部返回 `Result<_, i64>`（`EEXIST`/`ENOSPC`/`ENOENT`/`EINVAL`）；`FatFs::open` 返回 `OpenError { errno, reason }`（`EIO`/`ENOTSUP`/`EUCLEAN`），原因串只用于证据行 | `fs/mount.rs`、`fs/fat.rs::OpenError` | ✅ CSS-2 已交付（拒绝理由可断言） |
| 文件系统接入面 | ✅ `FileSystem` trait 已有 `name`/`stat`/`read`/`write`/`create`/`mkdir`/`list`，路径为挂载点内相对路径；挂载表**最长前缀路由**已实现并被双盘验证；**CSS-2 追加 `caps()`/`volume_info()` 两个只读查询**（带保守缺省实现，因此已有后端不会被强制改签名） | `kernel/src/fs/vfs.rs`、`fs/mount.rs`、`make test-fat`（`/disk` 与 `/mnt/fat` 同时挂载） | ✅ CSS-2 已交付（能力位与卷信息） |
| 磁盘文件系统 | 只有 **FAT12/16/32 只读**（BPB 解析、固定根目录区/簇链目录、8.3 + NT 大小写标志 + LFN）；写操作 `EROFS`，结构损坏 `EUCLEAN` | `kernel/src/fs/fat.rs`、`docs/filesystem.md` | ✅ CSS-4 先补 FAT 写，再按第 7 节扩格式 |
| path 语义 | **只支持绝对路径**；`.`/`..` 一律 `EINVAL`（没有 cwd）；`//` 与结尾 `/` 折叠 | `fs/vfs.rs` 模块文档 | ⚠️ 因此 Windows 式路径**不能**直接塞进内核 VFS（4.2 定了解析位置） |
| errno 集合 | 内核侧已有：`ENOENT`/`ENOEXEC`/`EBADF`/`EINVAL`/`EEXIST`/`EISDIR`/`ENOTDIR`/`ENOSPC`/`EROFS`/`EIO`/`EUCLEAN`/`ENOMEM` | `fs/vfs.rs` 常量区 | ✅ CSS 只**追加**需要的（`ENODEV`/`ENOTSUP`，见 4.5） |
| 卷对象 / 卷标 / 盘符 | ✅ **CSS-1 有卷表**（`storage/registry.rs`：设备→分区→卷 + `vol_id`）；✅ **CSS-3 有盘符表**（`storage/letters.rs`：确定规则 `C`/`D`/… + 自定义映射持久化到 CKM 状态区 `storage/letters`，`storage_assign_letter`(264)）；卷标目前**只读展示**（来自 BPB，写/改名未做）；Shell 提示符里的 `C:` 仍是常量字符串（未改为查盘符表） | `kernel/src/storage/{registry,letters}.rs`、卷记录 `letter=` 字段；`tests/test-storage`/`test-storage-mbr` 的 `letter=` 断言 | ✅ CSS-3 盘符已交付；状态树/卷标写仍待做 |
| syscall 号段 | `0..=255` = Linux 语义、`>= 256` = Cerlesse 扩展；`256` = `getprocs`；`257–260` 由 CKM 提案预留；**CSS-1 后 `SYS_MAX = 267`**（CSS 占 261–266） | `kernel/src/syscall/syscall_table.rs`、`docs/ckm.md` 4.3 | ✅ CSS 从 **261** 起编号（6.3） |
| 可写持久化 | RAMFS 在内存、FAT 只读 ⇒ **文件语义**的可写落盘点仍然没有（CSS-4） | `docs/filesystem.md`、`fs/fat.rs` | ✅ **CKM-2 已于 2026-10-12 交付内核侧的「原子状态区」**（系统盘尾部 1 MiB，双槽 A/B + CRC32，`kernel/src/ckm/store.rs`）—— 它是**键值**持久化，不是文件；字母表/卷标的持久化落点因此就绪（6.6），FAT 写仍是 CSS-4 的第一件事 |
| 镜像工具 | `tools/mkfs/mkfs.py` 自己生成 FAT12/16/32 镜像（不调 `mformat`），正确性由 **mtools 独立复核**；`tools/image-builder` 组装 64 MiB FAT 启动盘；✅ CSS-1 已在同一文件里追加 **GPT/MBR 分区表 + FAT 卷**的夹具生成（GPT 两分区、MBR 双分区、主表头损坏、无分区表四类） | `tools/mkfs/mkfs.py`、`tools/README.md`、`make fat-image`/`make storage-images` | ✅ CSS-1 已完成 |
| 宿主机复核工具（**实测**） | ✅ 有：`mtools`（`mdir`/`mtype`/`mformat`/`mlabel`）、`blkid`、`partx`、`mkfs.ext2`/`mkfs.ext4`/`dumpe2fs`/`debugfs`/`e2fsck`；❌ **没有**：`sfdisk`/`fdisk`/`parted`/`sgdisk`/`mkfs.fat`/`mkfs.exfat`/`ntfs-3g` | 本仓库当前开发环境实测（`command -v`）+ 手工 GPT/DOS 镜像实验（见下行） | ⚠️ 决定第 10 节的复核手段（下表） |
| 分区表的独立复核能力（手工镜像实验） | `partx -s <img>` **能**枚举 DOS/MBR 分区（两分区镜像实测输出 NR/START/END/SECTORS）；`blkid -p <img>` 对镜像只报最外层类型（GPT 镜像 `PTTYPE="PMBR"`、DOS 镜像 `PTTYPE="dos"`）**不枚举**表项；手工构造的合规 GPT（主/备表头 + CRC 正确）在 `blkid`/`partx` 下同样**不被枚举**（无输出） | 本地实测：自写 GPT（含备表与两份 CRC）与 DOS 双分区镜像分别跑 `blkid -p`/`partx -s` | ⚠️ **GPT 在本机没有现成独立复核工具**（开放问题 4 与 10 节） |
| 内核依赖门禁 | `tools/check-boundary.py` K4：`kernel/Cargo.toml` 依赖**只允许** `shared`/`abi` | `tools/check-boundary.py::ALLOWED_KERNEL_DEPS`、`make check-boundary` | ⚠️ 「引入第三方文件系统 crate」必须先改这条门禁（开放问题 1） |

### 2.1 现状小结：缺的是一层，不是一套

- 已有的两层都**不需要重做**：下面的 `BlockDevice`（按扇区读写）与上面的 `FileSystem` + 挂载表路由；
- 缺的是中间这层：**设备 → 分区 → 卷 → 挂载点/盘符** 的对象与登记（以及它需要的分区表解析、
  文件系统探测、稳定标识）；
- 另一处必须修的现状：**挂载失败返回字符串**（`fs/mod.rs`），改成 errno 才能进验收。

> CSS-2 之后：上表的「设备注册表容量」「分区表」「镜像工具」由 CSS-1 补齐，
> 「挂载方式」「挂载错误语义」「文件系统接入面」由 CSS-2 收口；
> CSS-3 已补上**盘符表与持久化**（`storage_mount`(262)/`unmount`(263)/`assign_letter`(264) +
> `storage/letters.rs` → CKM 状态区键 `storage/letters`）与 `libc` 的 `C:\` 重写；
> 仍缺的是 **`/storage/**` 只读状态树、卷标改名/持久化** 与**可写持久化**（CSS-4）。

## 3. 存储对象模型（定稿）

### 3.1 四层与各自归属

```text
设备（disk）      BlockDevice（现状）                  driver/block.rs + storage/registry.rs
  └ 分区（part）  GPT/MBR 表项 → 起止 LBA + 类型       storage/{partition,gpt,mbr}.rs
      └ 卷（vol） 文件系统实例（BPB/superblock 探测）   storage/volume.rs
          └ 访问    挂载点（0..n 个）+ 盘符（0..1 个）  storage/registry.rs + fs/mount.rs
```

| 层 | 稳定标识 | 可变属性（允许变，不属于身份） |
| ---- | ---- | ---- |
| 设备 | `disk_id`（6.4 记录里的 `disk_id`） | 枚举序号、探测到的型号、容量 |
| 分区 | `(disk_id, 分区序号)`（GPT 另可取 `PARTUUID`） | 起止 LBA（重分区即换身份） |
| 卷 | `vol_id`（3.4） | 盘符、卷标、挂载点、只读状态 |
| 挂载点/盘符 | 无（纯别名） | 随时可变 |

### 3.2 `Volume`（卷管理器内部引用的对象）

```rust
struct Volume {
    id: VolId,             // 稳定卷标识（3.4）：盘符、挂载点都是别名
    disk_id: u16,          // 所属块设备（注册表序号）
    part_index: u8,        // 分区序号（无分区表 = 0，表示「整盘一个卷」）
    fs: FsKind,            // 探测到的文件系统（FAT12/16/32、EXFAT、EXT2、EXT4、NTFS、ISO9660…）
    fs_uuid: Option<[u8; 16]>, // 文件系统自带 UUID/序列号（FAT 的卷序列号、ext 的 UUID…）若存在
    label: VolLabel,       // 用户可见卷标：[A-Za-z0-9._-]{0,11}，未知即空
    letters: [Option<u8>; MAX_LETTERS_PER_VOL], // 盘符（0 或 1 个；数组留给未来多字母）
    mounts: [MountPoint; MAX_MOUNTS_PER_VOL],   // 一个或多个挂载路径（4.3）
    read_only: ReadOnlyReason, // 只读**原因**，不是布尔（3.5）
    health: Health,        // ok / degraded / offline / unsupported（3.6）
    geometry: Option<FsGeometry>, // 容量/可用（后端算得出才填，算不出必须 unknown）
}
```

- 容量与占用的约定（必须）：**算不出就报 `unknown`，不许用 0 冒充**（0 会让「盘满」与「没实现」无法区分）；
- `MAX_MOUNTS_PER_VOL` 第一版 **2**（默认挂载点 + 一个别名挂载点），槽位满 → `ENOSPC` + 证据行；
- 卷表固定容量 **16**（与 `ckm/state.rs` 的 32 槽、进程表同样的「固定容量 + 明确拒绝」风格）。

### 3.3 分区表支持范围（CSS-1）

| 表 | 第一版做什么 | 明确不做什么 |
| ---- | ---- | ---- |
| **GPT** | 只读扫描：保护性 MBR 检查、主表头（LBA 1）解析、`128` 个表项、类型 GUID 索引、主/备表头 CRC32 与表 CRC32 校验、按类型 GUID 给「可挂载/不支持」判定 | 写表项、创建/删除分区、分区属性位编辑、`PARTLABEL` 改名 |
| **MBR** | 只读扫描：4 个主表项、扩展分区链（EBR 跳转，最多 8 层，防环）、`0x55AA` 签名校验 | 写表项、CHS 相关语义（只按 LBA）、逻辑分区创建 |
| 无表 | **合法状态**：整盘当作一个分区（`part_index = 0`），直接对偏移 0 做文件系统探测 —— 这正是当前的 `disk.img`/`fat.img` 形态，CSS-1 必须继续支持 | 自动「帮忙」写一张表 |

必须的判定顺序（确定性，不得并行猜测）：① 扇区 0 有 `0x55AA` → ② 若 MBR 类型为 `0xEE`（保护性）则按 GPT 解析
（CRC 不过 → `health = degraded`，且**不**回退去解析 MBR，避免把保护表当成分区表）→ ③ 否则按 MBR 解析 →
④ 表里没有任何可识别项或表损坏 → 退化为「整盘一个卷」并在证据行里写清原因（`table=none|invalid`）。

### 3.4 稳定卷标识 `vol_id` 的生成规则（必须确定性）

`vol_id` 是内部引用卷的唯一依据；**盘符、卷标、挂载点都不是身份**（盘符会随插拔与分区变化而变，
Windows 也是用卷 GUID 路径解决同一问题）。第一版不引入随机数与时间（内核没有熵源，且同一台机器
两次启动必须逐字节相同）：

| 优先级 | 条件 | `vol_id` | 例子 |
| ---- | ---- | ---- | ---- |
| 1 | 文件系统自带 UUID/序列号可读 | `fs:<uuid/序列号 的十六进制>` | `fs:1a2b3c4d`（FAT 卷序列号，BPB `0x43` 处 4 字节）、`fs:9f2c…`（ext UUID 前 8 字节） |
| 2 | 有分区表（或探测不到 UUID） | `pt:<disk_id>-<part_index>-<起 LBA>` | `pt:0-1-2048` |
| 3 | 无分区表且无 UUID | `raw:<disk_id>` | `raw:1` |

- 同一卷的三种形态**必须给出同一个** `vol_id`（同一镜像两次启动、两次枚举 → 逐字节相同的卷表）；
- `vol_id` 变化意味着「这是另一个卷」：写字母映射（CSS-3）时按 `vol_id` 记，拔插后字母不漂移；
- 两个卷 `vol_id` 冲突（如两块盘被克隆出同一序列号）→ **必须**报出冲突（`health = degraded` +
  证据 `storage: vol conflict id=… disk=0,1`），而不是静默合并。

### 3.5 只读原因（枚举，不是布尔）

| 值 | 含义 | 第一版谁会命中 |
| ---- | ---- | ---- |
| `rw` | 可写 | RAMFS；CSS-4 之后的 FAT/exFAT/ext2 |
| `ro-backend` | 后端只实现了只读 | 现状 FAT（`EROFS`） |
| `ro-mount` | 挂载参数要求只读（用户或 CKM 指定） | 任意卷 |
| `ro-device` | 设备只读 | 未来的只读介质/`-device ro` |
| `ro-dirty` | 检测到「上次写入未完成」的痕迹（如 FAT 的 `0x25`/`0x27` 脏标志、ext 的未清 journal） | CSS-4 的路障：宁可拒绝写 |

- 只读原因必须能从用户态读到（6.4 卷记录的 `read_only` 字节）并可打印（6.7 证据行）；
- `EROFS` 的语义保持不变（写接口返回 `-EROFS`），只是现在**同时**能回答「为什么只读」。

### 3.6 健康状态（枚举）

| 值 | 触发 | 行为 |
| ---- | ---- | ---- |
| `ok` | 探测与校验全过 | 正常挂载 |
| `degraded` | 结构可疑但可读：GPT CRC 失败但主表可用、FAT 簇链有环/越界、`vol_id` 冲突、目录项非法 | **仍然挂载**（只读），每次挂载打印原因 |
| `offline` | 设备消失（热插拔）或读扇区失败（`EIO`） | 卸载该卷的挂载点，路径查询返回 `ENODEV` |
| `unsupported` | 探测到文件系统但内核没有对应后端 | 不挂载；`storage_volumes` 仍**列出**它（这是「可见」的关键） |

## 4. 路径、盘符与挂载（定稿）

### 4.1 双视图（同一份数据的两个入口）

假设系统识别出三个卷，则目标视图如下（**字母与双入口已随 CSS-3 落地**：盘符由
`storage/letters.rs` 按确定规则/自定义映射给出，`D:\…` 由 `libc::path` 重写为 POSIX；
表内的 `ext4`/`NTFS`/`exFAT` 后端仍是规划）：

| 卷标识 | 文件系统 | Windows 式路径 | Linux 式挂载路径 |
| ---- | ---- | ---- | ---- |
| 系统卷 | ext4 | `C:\` | `/` |
| 数据卷 | NTFS | `D:\` | `/volumes/data` |
| U 盘 | exFAT | `E:\` | `/media/usb` |

同一个文件必须能通过两个入口访问，且**不是两份数据**：

```text
D:\Projects\demo.rs
/volumes/data/Projects/demo.rs
```

- 验收方式（必须）：两条路径各自 `open` + `read`，字节逐一相等；且用 `storage_volumes` 证明两者
  `vol_id` 相同（10 节条目 4）。

### 4.2 Windows 式路径在哪一层解析（必须）

**内核 syscall 只接受 POSIX 绝对路径**（`/` 开头，与 `fs/vfs.rs` 现有语义一致）。`C:\x` 的解析在
**用户态库**（`userland/libc/src/path.rs`，✅ CSS-3 已落地：`is_windows`/`resolve`/`resolve_str`）
里做，流程固定为：

```text
D:\Projects\demo.rs
   ↓  libc::path 识别「<字母>:」
   ↓  查卷表（storage_volumes，不硬编码字母）
   ↓  得到该卷的默认挂载点
/volumes/data/Projects/demo.rs
   ↓  VFS 最长前缀路由（fs/mount.rs，已有）
后端（NTFS/FAT/…）访问文件
```

三条理由（写下来避免以后又把盘符塞回内核）：

1. **syscall ABI 不变**：路径参数仍是「内核认识的绝对路径」，Windows 式写法只是库的便利层；
2. **不破坏 POSIX 视图**：`/volumes/data/…` 直接可用，不依赖任何 Windows 语法；
3. **字母表是内核维护、库只查询**：库不缓存硬编码映射，`storage_volumes` 是唯一来源（字母变了立刻生效）。

### 4.3 挂载点规则（必须）

| 卷类别 | 默认挂载点 | 规则 |
| ---- | ---- | ---- |
| 系统卷 | `/`（保留） | 由引导/CSS-1 指定；现状的 `/disk` 继续作为兼容别名挂载 |
| 固定数据卷 | `/volumes/<label>`，标签为空或重名时退化为 `/volumes/<vol_id>` | 挂载前必须做**重名检测**：重名不得覆盖已有挂载点（`EEXIST`） |
| 可移动卷 | `/media/<label>`（同上退化规则） | 插拔后自动挂载/卸载（CSS-3 起；第一版由重扫触发） |
| 测试/兼容 | `/disk`、`/mnt/fat`（现状） | CSS-2 必须保持这两个点可挂，现有 `make test-fat` 断言不得失效 |

- 一个卷**可以同时挂到多个点**（`MAX_MOUNTS_PER_VOL = 2`），两处的文件解析到同一个卷；
- 卸载只移除别名：卷本身仍在卷表里（`health`/`mounts` 更新），再次挂载不需要重新探测设备。

### 4.4 大小写与命名语义（必须）

- **不统一转小写**：路径解析保留原始字节，按目标文件系统的规则匹配（NTFS/FAT 不区分大小写但保留
  原名，ext2/4 区分）—— 现状 FAT 后端已经读 NT 大小写标志（`fat.rs`），CSS-2 把这条规则显式化为
  后端能力位（`FsCaps::case_insensitive`），由路径解析层查询，而不是各自实现一遍；
- 盘符本身大小写不敏感（`c:\` ≡ `C:\`），字母映射表存大写。

### 4.5 错误语义（CSS 追加/复用，必须可区分）

| 情形 | errno | 说明 |
| ---- | ---- | ---- |
| 路径不存在 / 字母未分配 | `ENOENT(-2)` | 与现状一致 |
| 非法路径 / 非法字母 / 非法卷标 | `EINVAL(-22)` | `.`/`..` 沿用现状 |
| 挂载点已被占用 | `EEXIST(-17)` | **现状返回 `&'static str`，CSS-2 改这里** |
| 挂载表/卷表/槽位满 | `ENOSPC(-28)` | 沿用「固定容量 + 明确拒绝」 |
| 写只读卷 | `EROFS(-30)` | 沿用；原因见 `read_only` 枚举 |
| 结构损坏 | `EUCLEAN(-117)` | 沿用 |
| 设备读失败 / 设备消失 | `EIO(-5)` / **`ENODEV(-19)`（CSS 新增）** | `ENODEV` 让「拔盘」与「坏盘」可区分 |
| 识别到但没有后端 | **`ENOTSUP(-95)`（CSS 新增）** | 与「不存在」区分开：盘在、格式不支持 |
| 能力不足（挂载/卸载/分配字母/格式化） | `EPERM(-1)` | 走 CKM 判定（6.2），与 `EACCES` 分开 |

## 5. VFS 收口与模块划分（定稿）

### 5.1 `FileSystem` trait 的增量（✅ CSS-2 已落地）

**不重写、不删现有方法**；CSS-2 只追加两个只读查询（其余方法签名不动）。
与下图的唯一差别：两个方法都带**保守缺省实现**（`FsCaps::read_only_unknown()` / `VolumeInfo::unknown()`），
因此「没声明能力的后端」被当作只读，而不是被当作可写 —— 猜错的代价是数据被写坏。

```rust
trait FileSystem {
    // …现状：name / stat / read / write / create / mkdir / list（v0.7–v0.8）

    /// 后端能力（CSS-2 新增）：只读 / 是否支持写 / 大小写策略 / 最大文件名长度 / 是否有卷标
    fn caps(&self) -> FsCaps;

    /// 卷信息（CSS-2 新增）：探测阶段得到的几何与自带标识（UUID/序列号/卷标）
    fn volume_info(&self) -> VolumeInfo;
}
```

- 探测入口（新增，必须）：「这个偏移上有没有我的格式」只能自己回答，探测顺序由 CSS 固定（5.3），
  不是由后端互相争抢。**落地方案与本节初稿不同**：`probe` 没有做成 trait 方法，而是
  `fs::fat::probe(dev, start_lba, sectors)` + 纯函数内核 `probe_sector(boot, capacity)` ——
  trait 方法需要**先有后端实例**，而探测的对象恰好是「还没有实例」的偏移（没有实例就没有 `Self`）；
  真正要消除的不是「谁回答」而是**重复**：CSS-1 时 `storage/volume.rs` 抄了一份 BPB 判定，
  CSS-2 已删掉它，FAT 的规则现在只有一处（`fs/fat.rs` 的 `Geometry::parse` + `bp_identity`），
  卷表与后端读的是同一份代码。未来新增 exFAT/ext2 探测时按同一形状加 `fs::<backend>::probe`；
- 能力位与卷信息**都从后端读**，不写死在挂载行里：`ro=` 来自 `caps().read_only`，
  `storage: volcheck` 行会把后端报的序列号/卷标与 `storage_volumes` 记录逐字段比对；
- 写入仍走既有 `write`/`create`/`mkdir`：**没有第二条写路径**（这是「所有写入通过 VFS」的制度化）。

### 5.2 挂载表带上卷身份（✅ CSS-2 已落地）

- `fs/mount.rs::MountTable` 拆成 **`backends`（后端存储）+ `mounts`（挂载点条目）**：条目带
  `vol_id`（哪个卷）、`backend`（下标）与 `options`（`read_only`）；拆分的原因就是本节第 4 节 4.3 的
  「一个卷可以同时挂到多个点」—— v0.7 的「挂载点里直接放 `Box<dyn FileSystem>`」把
  「挂载点数 = 后端数」写成了隐含假设，做不到别名；
- 新增 `alias_volume(point, vol_id, options)`（**不重新打开设备**，因此两个路径下是同一份数据）与
  `umount(point)`（返回被卸条目的 `vol_id`；`/` → `EINVAL`，非挂载点 → `ENOENT`；
  后端**不释放** —— 可能还有别的挂载点引用它）；挂载/卸载都会把「哪个卷挂了几个点」写回卷记录
  （偏移 56）—— 记录里的 `mounts` 与挂载表只能有一个事实源；
- 现有最长前缀路由**不变**；`stat` 行尾追加 `vol=<vol_id>`（非卷挂载如 RAMFS 根是 `vol=-`，
  不是空值也不是猜一个 id），让「路径 → 卷」在串口可断言；另外 `fs::vfs::route(path)` 给出
  挂载点 / 后端 / `vol_id` / 选项，`fs: route path=… vol=…` 是它的证据行；
- `mount`/`umount` 的返回值已是 `Result<(), i64>`（errno，见 4.5 / 9 节 CSS-2 行）。

### 5.3 目录结构（规划，进入 CSS-1/CSS-2 时创建并回填）

```text
kernel/src/
├── storage/                     # 新增（CSS-1 起）：设备/分区/卷的登记与稳定标识
│   ├── mod.rs                   # init()、常量（卷表容量）、统一入口
│   ├── registry.rs              # 设备↔分区↔卷登记、枚举顺序、vol_id 分配
│   ├── partition.rs             # 分区抽象与探测调度
│   ├── gpt.rs                   # GPT 只读解析 + CRC32
│   ├── mbr.rs                   # MBR/EBR 只读解析
│   └── volume.rs                # Volume/VolId/Label/Health/ReadOnlyReason
├── fs/                          # 现状保留，只按下表扩展
│   ├── vfs.rs / path 语义        # 现状（v0.7）
│   ├── mount.rs                 # + vol_id/options（CSS-2）
│   ├── inode.rs / file.rs       # 现状
│   ├── ramfs.rs                 # 现状
│   ├── fat.rs                   # 现状（只读）→ CSS-4 加写
│   ├── exfat/                   # 规划（CSS-4）
│   ├── ext2/                    # 规划（CSS-4）
│   ├── ext4/                    # 规划（CSS-6）
│   ├── ntfs/                    # 规划（CSS-6）
│   └── iso9660/                 # 规划（CSS-6）
├── syscall/
│   └── storage.rs               # 新增（CSS-1/CSS-3）：6.3 的 6 个号 + 记录导出
└── ckm/                         # CKM-1 起（见 ckm.md）
```

文件系统探测顺序（必须确定性，命中多个即视为歧义）：`GPT/MBR 分区`（先）→ 按固定顺序尝试
`FAT` → `EXFAT` → `EXT2/EXT4`（superblock 魔数 `0xEF53` 再按特性位区分）→ `NTFS`（`NTFS    `）→
`ISO9660`（扇区 16 的 `CD001`）；**多个后端同时声称命中** → 不挂载、`health = degraded`、
证据行写明 `ambiguous=<列表>`（不猜、不取第一个）。

## 6. 与 CKM 的接口（定稿）

### 6.1 分工：CSS 是 CKM 之下的存储子系统，不是第四个角色

| 层 | 归属 | 做什么 |
| ---- | ---- | ---- |
| 机制（卷表、分区、挂载、字母） | **CSS / 内核**（`kernel/src/storage/`） | 只回答「有哪些卷/分区」「挂在哪」「能不能写」，并**强制**权限 |
| 判定（谁能挂载/卸载/分配字母/格式化） | **CKM Core**（`kernel/src/ckm/policy.rs`） | CSS 的每个受控动作都是 CKM 的一个判定点（6.2） |
| 界面（列出磁盘/卷、发起操作、显示健康与只读原因） | **CKM Manager（`ckm storage`，CSS-5）** | 用户态、无特权：所有写操作经 `ckmd`/能力校验 |
| 持久化（字母表、卷标偏好） | **CKM 状态区**（CKM-2 的可写区） | CSS 不自己造持久化格式（6.6） |
| 覆盖层 / 回滚 / 恢复 | **CKM**（[ckm.md](./ckm.md) 第 6–8 节：基线永不被写、覆盖表、事务回执、恢复镜像） | CSS 只提供可见性与只读状态；**覆盖层是 CKM 的注册表项，不是 CSS 挂载的文件系统**（ckm.md 6.2）——CSS 不实现第二套覆盖/回滚机制 |

### 6.2 CKM 侧必须同步的三处（本文不改 ckm.md 的既有条款，只**追加**；已在 ckm.md 同步）

| 项 | 追加内容 | 落点（ckm.md） |
| ---- | ---- | ---- |
| 能力位 | `CAP_STORAGE_ADMIN = 0x10`（挂载/卸载/分配盘符/重扫）、`CAP_STORAGE_FORMAT = 0x20`（创建与格式化）。uid 1000（管理员）持有两者；`/init`(uid 0) 与 `ckmd`(1001) **不**持有（最小权限） | 3.2 |
| 判定点 | 8：`storage_mount`/`storage_unmount`/`storage_assign_letter`/`storage_rescan` → `CAP_STORAGE_ADMIN`；9：`storage_format` → `CAP_STORAGE_FORMAT`；10：写 `/storage/**` 状态树 → `CAP_STORAGE_ADMIN` | 3.4 |
| 事件枚举 | `12 storage_mount`、`13 storage_unmount`、`14 storage_letter`、`15 storage_format`、`16 storage_rescan`（串口短名同字面） | 4.6 |

- 拒绝时的三样东西**完全沿用** CKM：`-EPERM` + 一条 48 字节审计记录 + 一行串口证据；
- 只读查询（`storage_volumes`、读 `/storage/**`）**不需要**能力位（与 `ckm_component_state` 的只读面一致）。

### 6.3 CSS 系统调用（Cerlesse 扩展段，接在 CKM 的 260 之后）

| 号 | 名字 | 签名（5 参数 ABI） | 权限 | 行为 |
| ---- | ---- | ---- | ---- | ---- |
| 261 | `storage_volumes` | `(buf_ptr, buf_len, out_count_ptr, 0, 0)` | 无（只读） | 按 6.5 的**确定性顺序**导出卷记录（64 字节/条），返回**条数** |
| 262 | `storage_mount` | `(vol_id_ptr, vol_id_len, point_ptr, point_len, flags)` | `CAP_STORAGE_ADMIN` | 把卷挂到路径（`flags` bit0 = 只读挂载） |
| 263 | `storage_unmount` | `(point_ptr, point_len, 0, 0, 0)` | `CAP_STORAGE_ADMIN` | 卸载别名；卷仍在卷表 |
| 264 | `storage_assign_letter` | `(vol_id_ptr, vol_id_len, letter, 0, 0)` | `CAP_STORAGE_ADMIN` | 分配盘符；`letter = 0` 表示取消；占用中的字母 → `EEXIST` |
| 265 | `storage_rescan` | `(disk_index, 0, 0, 0, 0)` | `CAP_STORAGE_ADMIN` | 重扫某设备的分区与文件系统（`0xFFFFFFFF` = 全部），更新卷表 |
| 266 | `storage_format` | **CSS-4 才定义** | `CAP_STORAGE_FORMAT` | 第一版**不实现**（CSS-1…CSS-3 落公共 `ENOSYS` 桩） |

- `SYS_MAX` 由 **260** 改为 **267**（即 `0..=266` 可用），`syscall_table.rs` 的号段说明同步；
- 号一旦发布不再回收（与 Linux 号段纪律一致）；`storage_format` 之外的 5 个号在 CSS-1（261/265）与
  CSS-3（262/263/264）分两批实现，未实现期间落 `ENOSYS` 桩（可区分「号已定义但未实现」）。
- ✅ **现状（2026-10-12）**：261/262/263/264/265 均已实现，只有 266 `storage_format` 仍是
  `ENOSYS` 桩（CSS-4 才定义）；262/263/264 的失败语义固定在 `syscall/storage.rs` 的表头里
  （`ENOENT` 卷不存在/`ENOTSUP` 没后端/`EEXIST` 挂载点或字母重复/`EINVAL` 卸根或非法字母）。

### 6.4 卷记录（64 字节）与分区记录（48 字节）

**权威定义（阶段三起）**：卷记录的尺寸/字段宽度在 `abi::volume`（`abi/src/volume.rs`），
内核 `storage/volume.rs` 是**写记录的一方**、`libc` 是**解析的一方**，两侧引用同一处。

**卷记录（64 字节）** —— `storage_volumes` 的单元：

| 偏移 | 大小 | 字段 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 16 | `id` | `vol_id` 的可读形式（如 `fs:1a2b3c4d`），NUL 填充，`[A-Za-z0-9._:-]{1,15}` |
| 16 | 2 | `disk_id` | 块设备注册表序号 |
| 18 | 1 | `part_index` | 分区序号（无表 = 0） |
| 19 | 1 | `fs_kind` | 1 FAT12 / 2 FAT16 / 3 FAT32 / 4 EXFAT / 5 EXT2 / 6 EXT4 / 7 NTFS / 8 ISO9660 / 0 = unsupported |
| 20 | 1 | `read_only` | 0 rw / 1 ro-backend / 2 ro-mount / 3 ro-device / 4 ro-dirty |
| 21 | 1 | `health` | 0 ok / 1 degraded / 2 offline / 3 unsupported |
| 22 | 2 | `flags` | bit0 = 可移动，bit1 = 有 `fs_uuid`，bit2 = 容量未知（`free_bytes` = unknown），bit3 = `label` 被截断（见下）；其余位必须为 0，非 0 视为 ABI 不匹配 |
| 24 | 1 | `letter` | 盘符（`0` = 未分配；其它 = `'A'..=Z'` 的 ASCII） |
| 25 | 11 | `label` | 卷标（11 字节内直接存放，NUL 填充）；**超过 11 字节必须置 `flags.bit3`** 而不是静默截断（用户据此知道看到的是简称） |
| 36 | 4 | `fs_uuid_prefix` | 文件系统 UUID/序列号前 4 字节（无则全 0，且 `flags.bit1 = 0`） |
| 40 | 8 | `total_bytes` | 总容量（`u64`，LE） |
| 48 | 8 | `free_bytes` | 可用容量（`0xFFFF_FFFF_FFFF_FFFF` = unknown，见 3.2） |
| 56 | 4 | `mount_count` | 当前挂载点数 |
| 60 | 4 | `reserved` | 0 |

**分区记录（48 字节）** —— 供 `ckm storage list` 与主机侧断言使用（同一个 syscall 的第二个视图由
`storage_rescan` 后的卷表推导；第一版只要求「卷记录」进 ABI，分区记录经 `/storage/partitions` 只读暴露）：

| 偏移 | 大小 | 字段 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 2 | `disk_id` | 块设备序号 |
| 2 | 1 | `part_index` | 1..=128（GPT）/ 1..=4 主 + 5.. 逻辑（MBR） |
| 3 | 1 | `table` | 0 none / 1 MBR / 2 GPT |
| 4 | 8 | `start_lba` | 起始 LBA（512 字节扇区） |
| 12 | 8 | `sectors` | 扇区数 |
| 20 | 16 | `type_raw` | GPT 类型 GUID 原始 16 字节（字节序按规范，不做主机序转换）；MBR 时前 1 字节 = `0x00`–`0xFF` 分区类型 |
| 36 | 4 | `flags` | bit0 = 启动/EFI 分区，bit1 = 只读建议（GPT 属性位），其余保留 0 |
| 40 | 8 | `reserved` | 0 |

两份布局与 6.3 的号段在 [docs/api.md](./api.md) 与 `userland/libc` 里有文字说明与解析器；
**数字不镜像**（阶段三起与 `DIRENT_SIZE`/`PROCINFO_SIZE` 同一做法：`abi` 是唯一权威定义，用户态用
`volume_*`（已落地）/`partition_*`（CSS-3）解析器，不自己算偏移）。

### 6.5 枚举顺序（必须确定性）

`storage_volumes` 输出顺序：`disk_id` 升序 → `part_index` 升序 → `id` 字节序升序。
理由与 `fs: ls`、`getprocs`、`ckm_component_state` 一致：**确定性是可断言的前提**
（两次调用对同一组设备必须给出逐字节相同的输出）。

### 6.6 状态树与持久化

- 只读状态树（**仍待做**，与 `/ckm/**` 同风格）：`/storage/volumes`（64 字节记录拼接）、
  `/storage/partitions`（48 字节）、`/storage/letters`（`<letter> <vol_id>\n` 文本视图，便于人读）；
  写它们需要 `CAP_STORAGE_ADMIN`（判定点 10）。**现状**：卷表/盘符不经该树暴露，而是由
  `storage_volumes`(261) 导出卷记录（含 `letter` 字段，偏移 24）；`/bin/volumes` 已是它的消费者；
- **字母表与卷标偏好的持久化落在 CKM 状态区**，命名空间 `storage/letters`、`storage/labels`：
  CSS 只定义键与语义，不定义持久化格式（避免与 CKM 的原子写/回执机制重复造一套）；
- ✅ **状态区已就绪**（2026-10-12，CKM-2）：系统盘（块设备 0）尾部 1 MiB 的双槽 A/B + CRC32 键值区
  （`kernel/src/ckm/store.rs`；镜像侧由 `tools/image-builder/build_image.py` 在 63 MiB 上 `mformat`
  后再追加保留区 —— 设备仍是 64 MiB，FAT **看不到**状态区）。它是**扇区级**的，因此不经过
  VFS，也不需要可写文件系统；CSS 侧只需给定键名与值编码（待 CSS-3/4 接入）；
- ✅ **CSS-3 已把 `storage/letters` 接上（2026-10-12）**：缺省 C 给系统卷、其余按 6.5 顺序给 D/E/…，
  这部分是**纯计算规则**，每次启动重算，因此「两次启动字母相同」不依赖盘；只有**用户自定义过的字母**
  才写盘（键 `storage/letters`，值是人可读文本 `C=fs:1a2b3c4d;Z=raw:4`）。只写自定义映射的理由：
  把算出来的缺省也写盘会让「盘上内容」与「规则」两份真相漂移；
- 生命周期：`storage::init` 在卷扫描**之前**调 `letters::load_from_store()`（先装盘上映射，再枚举并
  用确定规则补缺）；`storage_assign_letter`(264) 的每一次自定义都同步 `persist()`；`letter = 0` 取消
  自定义并立刻回到规则。证据行 `storage: letters load n=… source=store|none`、
  `storage: letters table n=… present=… first=C`、`ckm: store put key=storage/letters …`、
  `storage: letters saved custom=…`（`make test-storage` 断言）；跨重启读回由 `make test-ckm-store` 的
  两次启动给出（`storage: letters persisted probe=Z vol=raw:4` → 第二次启动 `letters load … source=store`）。

### 6.7 串口证据格式（必须）

```text
storage: init MAX_VOLUMES=16 MAX_MOUNTS_PER_VOL=2 max_devices=8
storage: disk index=0 dev=virtio-blk0 sectors=131072 table=GPT partitions=2
storage: part disk=0 index=1 table=GPT start_lba=2048 sectors=129024 type=linux-fs
storage: vol id=fs:1a2b3c4d disk=0 part=1 fs=fat16 label=CERLESSE letter=C mounts=/ ro=ro-backend health=ok
storage: mount point=/volumes/data vol=fs:1a2b3c4d ro=ro-backend
storage: policy op=storage_mount pid=3 uid=1000 result=allow need=0x10 detail=0x0
storage: unload point=/media/usb vol=fs:9f2c0001 reason=device-gone
storage: selftest PASS (checks=N)
```

- 每个受控动作**必须**打印一行（allow 与 deny 都打），deny 行沿用 CKM 的 4.5 格式；
- 挂载行只在挂载/卸载时打印（不是每次查询），因此 `make test-storage` 能断言
  「谁在什么时候把哪个卷挂到了哪里」；
- 枚举顺序与 6.5 一致：同一组设备两次启动的 `storage: vol` 行**逐行相同**；
- **兼容性必须完整展示**（CSS-5 界面同样受此约束）：卷行/记录里的 `fs=`（有没有后端）+ `ro=`（为什么只读）+
  `health=`（结构可不可信）三个字段合起来才是答案，不得压成一个「可用/不可用」布尔 ——
  用户必须能区分 `unsupported`（盘在、内核没后端）与 `ro-backend`（有后端、当前只读）。

### 6.8 文件系统后端如何装载（必须）

- 第一版**全部编译内置**：`fs/*` 后端随内核构建进映像（构建期决定开关），**不**通过 CKM 扩展体系
  运行时装卸 —— 文件系统实现运行在内核空间，装载它等于装载内核代码，普通扩展的安装流程不适用；
- CKM 的 `kind=4`（内核模块）在第一版**只登记状态、不装载**（[ckm.md](./ckm.md) 5.1 与第 11 节），
  因此「像装扩展一样独立更新文件系统驱动」**不在第一版承诺内**；未来安全的可加载模块机制就绪后，
  文件系统后端必须另行 ABI/签名/权限评审，不得因为「它只是文件系统」而绕过；
- 对用户可见的结果：第一版「支持哪些格式」由**内核映像**决定（`fs_kind` 枚举 + `health=unsupported`），
  升级格式支持 = 升级内核，而不是装一个扩展。

## 7. 文件系统支持路线与依赖政策

### 7.1 优先级（**这是规划，不是现状**；现状只有 FAT 只读 + RAMFS）

| 文件系统 | 初始目标 | 推荐程度 | 为什么这个顺序 |
| ---- | ---- | ---- | ---- |
| FAT12/16/32 | 保留只读实现 → **补写入** | 高 | 已有只读后端与 `mkfs` 夹具，写支持是「验证写入路径」的最短闭环（CSS-4 第一件） |
| exFAT | **读写**，优先照顾 U 盘/移动硬盘 | 高 | U 盘事实标准；无 4 GiB/簇数限制；结构比 NTFS 简单得多 |
| ext2 | **先读写**，作为第一种 Linux 可写文件系统 | 高 | 无日志 ⇒ 写入语义可控，是「Linux 侧可写」的合理起点 |
| ext4 | **先只读**（日志恢复与元数据校验做完再谈写） | 高 | 写 ext4 要正确处理 journal、extent、延迟分配；只读先给出可用价值 |
| NTFS | **先只读**，再评估可写 | 高 | 只读要解 MFT/属性/运行列表；写入还涉及日志与 `$Bitmap` 一致性 |
| ISO9660 | 只读（光盘/安装介质） | 中 | 结构最简单，能做「只读特殊文件系统」这一类的样板 |
| XFS | 后续只读 | 中 | 结构复杂（B+ 树全盘），先只读验证探测与挂载路径 |
| Btrfs | 后续研究（快照/校验和/子卷） | 中 | 高级功能多，第一版不承诺 |
| F2FS、其他 | 按设备需求逐步扩展 | 后期 | 无当前设备需求 |

第一阶段明确**集中做好 FAT + ext2 + exFAT**，然后扩 ext4/NTFS 只读。原因：文件系统不只是「解析目录」——
写入还要正确处理空间分配、缓存、元数据一致性、故障恢复与权限语义；对 ext4/NTFS/Btrfs 直接承诺
完整读写会显著抬高开发与维护成本，而收益在第一版很低。

### 7.2 从零实现还是引入第三方 crate（需要定夺，开放问题 1）

现状是**零第三方依赖**，而且这是**被门禁强制的**：`tools/check-boundary.py` 的 K4 要求
`kernel/Cargo.toml` 只依赖 `shared`/`abi`（`make check-boundary` 会红）。因此「复用现成实现」
不是改一行 `Cargo.toml`，而是要先改门禁与依赖政策。若决定引入，筛选条件（必须逐条写进评审）：

1. `no_std` + 不依赖 `std`/`libc`/浮点；`alloc` 使用方式与现状兼容；
2. 许可证在允许列表内（MIT / Apache-2.0；GPL 类需单独决定，因为会影响整个内核的许可）；
3. 无未审计的大块 `unsafe`（或 `unsafe` 边界可写清）、无网络/时间/随机数隐式依赖；
4. 可维护性：上游活跃度、代码体量（内核映像大小是硬约束）、能否只取解析层（只读用）；
5. 若引入，**只读先取**（解析），写入仍按第 8 节自己实现（写入路径需要与 VFS 的语义、错误码、
   一致性策略对齐，这部分很难外包）。

## 8. 写入语义（为 CSS-4 立规矩，避免边写边定）

1. **所有写入通过 VFS**：没有「磁盘工具直接写扇区」的第二条路径；分区表在第一版**只读**，
   因此不涉及「绕过 VFS 改盘」；
2. **写前校验**：写 FAT/exFAT/ext2 前必须校验目标位置在卷范围内、簇/extent 分配表未被破坏
   （`EUCLEAN` 优先于「尽力写」）；
3. **多结构更新**（FAT 的簇链 + FAT 表 + 目录项三处；ext2 的 inode + 位图 + 目录块）：
   第一版**必须**写明更新顺序与「中途失败」的收敛结果，并把「掉电一致性」作为显式选择：
   (a) 做最小日志/双写；(b) 明确不保证掉电一致（`ro-dirty` 检测 + 只读保护）——
   二选一在 CSS-4 开工前定（开放问题 3）；
4. **缓存策略**：第一版不引入 page cache（现状 `fs` 没有缓存层，块设备驱动已有页对齐 bounce
   buffer）；写入直落块设备，`storage: write` 证据行记 `lba/bytes`，让「真的写下去了」可断言；
5. **只读降级**：任何写入失败/校验不过 → 该卷立刻标记 `ro-dirty`/`degraded`，并**不再接受写**
   （宁可拒绝，不要半写）。

## 9. 分期计划（每期一个可验收出口）

| 期 | 交付物 | 依赖 | 验收出口 |
| ---- | ---- | ---- | ---- |
| **CSS-1 底层识别** ✅ | `kernel/src/storage/{mod,registry,partition,gpt,mbr,volume}.rs`（+ `selftest.rs`）+ `BlockDevice` 注册表扩容到 8 + `mkfs.py` 增加 GPT/MBR 镜像生成 + `storage_volumes`(261)/`storage_rescan`(265) + 卷记录布局 + 串口证据 | 无（`BlockDevice` 与 `mkfs.py` 已有） | ✅ 已过：`make test-storage`（26 项，GPT 两分区镜像枚举确定性 + MBR 镜像 + **无分区表镜像仍识别为整盘一卷**）、`make test-storage-fixtures`（夹具结构）、`make test-storage-mbr`（MBR 场景）、内核解析自测（11 例 / 45 断言，含**主表头损坏回退备用表头**） |
| **CSS-2 VFS 收口** ✅ | `FileSystem::caps`/`volume_info`（缺省保守）+ FAT 探测唯一入口 `fs::fat::probe`/`probe_sector`（删掉 `storage/volume.rs` 里重复的 BPB 判定）+ 挂载表拆 `backends`/`mounts` 并带 `vol_id`/`options` + `alias_volume`/`umount` + 挂载错误改 errno（`EEXIST`/`ENOSPC`/`ENOENT`/`EINVAL`/`ENOTSUP`/`EUCLEAN`）+ `stat` 行尾 `vol=` + `fs::storage_mount_selftest`（开机自测） | CSS-1 | ✅ 已过：`make test-storage`（31 项 + 2 正则：同一卷多挂载点、卸载后路径消失、只读原因可读、`volcheck` 记录一致）、`make test-storage-mbr`、`make test-fat`/`make test-fs`/`make test-user`/`make test-programs`/`make test-kernel-only` 不回归；编排场景 `fs`/`fat`/`storage`/`storage-mbr`/`programs` 全 PASS |
| **CSS-3 Windows 式卷管理** 🔨 | 盘符表 `storage/letters.rs`（确定规则 + 自定义持久化）+ `storage_mount`(262)/`storage_unmount`(263)/`storage_assign_letter`(264) + CKM 能力位/判定点 8 + libc 的 `C:\` 重写 + `/bin/volumes` 子命令。**已交付**；**仍缺**：卷标（写/改名）、`/storage/{volumes,partitions,letters}` 只读状态树、Shell 提示符改用盘符表 | CSS-2；字母持久化依赖 **CKM-2 可写区**（✅ 已就绪） | 🔨 已过：`make test-storage`（`letter=` 字段：系统卷 `C`、其余 `D`/`E`/…；`storage: letters selftest PASS (checks=36)`；克隆卷第二个不带字母）、`make test-storage-mbr`（`letters selftest PASS (checks=22)`）、`make test-programs`（`volumes demo` 两个入口读同一段字节 + 卸载后 `ENOENT`）、`make test-ckm`（零能力进程 `storage_mount` → `-1` need=0x10）。**待补**：两次启动字母相同的跨重启断言（现由 `test-ckm-store` 的 `storage/letters` 读回部分覆盖）与 `/storage/**` 状态树 |
| **CSS-4 可写存储** | FAT 写（簇分配 + 目录项 + 原子替换）→ exFAT 读写 → ext2 读写；`ro-dirty` 检测 | CSS-2（**先**收口 VFS）+ 开放问题 3 定夺 | 写-重启-读回逐字节一致；宿主机 mtools/`dumpe2fs` 独立复核读到同一结果 |
| **CSS-5 CSS 管理程序** | CLI `ckm storage list/vol/part/mount/umount/letter/fsck`（`fsck` **只读检查**，写回修复见第 11 节；并入 CKM Manager）；随后 TUI | CSS-3/4 + CKM-4（CLI 骨架） | 场景断言 CLI 输出与 `storage_volumes` 记录逐字段一致（界面不是第二事实源） |
| **CSS-6 兼容性扩展** | ext4 只读 → NTFS 只读 → ISO9660 只读；XFS/Btrfs 研究 | CSS-4（写路径先立） | 每格式一个夹具镜像 + 只读挂载 + 探测歧义负例（5.3） |

顺序上的三条硬约束（必须）：

1. **CSS-1 先于 CSS-3**：没有稳定 `vol_id` 就没有可信的字母映射；
2. **CSS-4 之前必须有 CSS-2**：写入必须走收口后的 VFS，否则会出现第二条写路径；
3. **CSS-3 的持久化依赖 CKM-2**：可写区没有落地前，字母表只按确定规则重算（不假装持久化）。

## 10. 第一版验收标准（可脚本断言）

`make test-storage`（新增 QEMU 场景 `storage`/`storage-mbr`，当前编排 19 个场景）+ 主机侧断言必须覆盖下表；
每条都不依赖人工观察。第 1、2、3、6、7 条已随 CSS-1 落地（卷枚举 / 无分区表退化 / 确定性 /
只读原因 / `degraded`）；第 8 条（卸载）与「记录布局」的**后端侧一致性**已随 CSS-2 落地
（`storage: unmount … mounts=1` + `fs: after unmount … errno=2`，以及 `storage: volcheck` 把
后端报的序列号/卷标与记录逐字段比对）；第 4/5/10 条已随 CSS-3 落地（双入口同内容同 `vol_id`、
盘符稳定与持久化、未授权被拒）；第 9 条（不支持的文件系统 `health=unsupported`）已随 CSS-1/CSS-2 可见，
`ENOTSUP` 读取路径待 CSS-4 后端接入。

| # | 测试 | 合格标准 | 断言方式（必须实现其中至少一条） |
| ---- | ---- | ---- | ---- |
| 1 | 有分区表的磁盘 | 按 GPT/MBR 正确枚举分区 | `storage: disk index=0 … table=GPT partitions=2` + 两条 `storage: part` 行（起止 LBA 与夹具一致）；MBR 侧同时要求 `partx -s <img>` 给出**同一组** LBA（第三方证据） |
| 2 | 无分区表的镜像 | 仍识别为「整盘一个卷」 | `storage: disk index=1 … table=none partitions=1` + 卷记录可读（现状 `fat.img`/`disk.img` 形态不回归） |
| 3 | 卷标识稳定 | 同一镜像两次枚举逐字节相同 | 两次 `storage: vol …` 行完全相同（`make test-storage` 比对两份串口日志） |
| 4 | 同一文件两个入口 | 两条路径同内容、同卷 | ✅ `volumes demo`：`C:\KERNEL.ELF`（libc 重写）与 `/volumes/<label>/KERNEL.ELF` 各自 `open/read` 字节相等 + `storage_mount` 记录同一 `vol_id` |
| 5 | 盘符不随插拔漂移 | 重启后字母不变 | ✅ 确定规则：`storage: letters table n=… first=C` 且每个卷记录的 `letter=` 与表一致（`check_consistency`）；✅ 自定义：`storage/letters` 写盘 → `make test-ckm-store` 第二次启动 `letters load … source=store` 读回 `Z` |
| 6 | 只读原因可见 | 只读卷能说出原因 | `ro=ro-backend`（现状 FAT）出现在卷行与卷记录偏移 20 |
| 7 | 健康状态 | 损坏结构标 `degraded` 且仍可读 | 夹具注入 GPT CRC 错误 → `health=degraded`；FAT 坏目录项 → `EUCLEAN` 而非崩溃 |
| 8 | 卸载 | 卸载后路径消失、卷仍在表 | 卸载后 `open` 返回 `ENOENT`，`storage_volumes` 仍列出该卷（`mount_count=0`） |
| 9 | 不支持的文件系统 | 可见但不挂载 | 未识别格式 → 卷记录 `fs_kind=0 health=unsupported` + 访问返回 `ENOTSUP`（**不是** `ENOENT`） |
| 10 | 权限 | 未授权挂载/分配字母被拒 | ✅ `make test-ckm`：uid 2000 的 `/bin/hello` 三次存储受控动作均 `-1` + `need=0x10`；对偶的是 Shell(uid 1000) `volumes demo` 三次成功（`make test-programs`） |
| 11 | 记录布局 | 内核与 libc 偏移一致 | ✅ 阶段三后**结构上不可能不一致**：尺寸与字段宽度定义在 `abi::volume`（`make abi-check` 固化 + `abi/src/record.rs` 编译期断言）；libc 解析器另有固定 64/48 字节样本的字段断言 |

补充（必须同批验收，避免「功能对、机制没生效」）：

- **确定性**：`storage_volumes` 两次调用逐字节相同（6.5 顺序）；
- **可区分**：`ENOENT`（没有）/`ENOTSUP`（有但没后端）/`ENODEV`（设备消失）/`EIO`（读失败）四者可区分；
- **不回归**：`make test-fs`、`make test-fat`、`make test-user` 全部保持 PASS（CSS-2 改了挂载错误语义，
  这是最容易踩的回归点）；
- **主机侧独立复核**（不经过内核，与「`mkfs` + mtools」「`ckc` + 系统 `tar`」同一思路）—— 手段按**实测结果**分工：

| 对象 | 独立复核手段 | 实测结论 |
| ---- | ---- | ---- |
| MBR 分区表 | `partx -s <img>` | ✅ 可用：两分区 DOS 镜像正确列出 `NR/START/END/SECTORS` |
| GPT 分区表 | （缺） | ❌ `partx`/`blkid` 对镜像文件**不枚举** GPT（合规主/备表头也一样）；本机没有 `sfdisk`/`parted`/`sgdisk` |
| FAT 文件内容 | `mtools`（`mdir`/`mtype`） | ✅ 可用（v0.8 起已在用） |
| exFAT | （缺） | ❌ 没有 `mkfs.exfat`/`exfatprogs`，也没有可靠的宿主侧解析器 |
| ext2/ext4 | `mkfs.ext2`/`mkfs.ext4` 生成、`dumpe2fs`/`debugfs` 复核 | ✅ 可用（CSS-4/CSS-6 用） |

  GPT 与 exFAT 这两项因此**没有第三方证据**：CSS-1 的 GPT 验收只能靠「`mkfs.py` 生成 + 内核解析」
  两套自写实现互查（弱于第三方复核），并在文档里把它标成已知限制（开放问题 4）；
  若后续开发机装上 `gdisk`/`parted`，一次性跑一次并把输出贴进文档即可升级为独立复核。

## 11. 非目标（第一版明确不做）

1. **不写分区表**：不创建/删除/调整分区，不迁移数据（只读扫描；写表需要独立的「危险操作」设计与恢复路径）；
2. **不做 RAID / LVM / 加密卷 / 快照**：属卷管理层之上的功能，先在 CKM 的扩展体系里做设计；
3. **不做磁盘配额 / 访问控制列表**：用户/组/权限先由 v1.0 的 `security/` 提供（见 ckm.md 3.3）；
4. **不把盘符判断写进文件系统驱动**（架构性非目标，第 0 节边界 1）：这是本节最重要的那条；
5. **不做网络文件系统**（NFS/SMB）：网络栈现状只有 Ethernet/ARP/IPv4/UDP 的简化实现；
6. **不做 GUI**：第一版只有 CLI（`ckm storage`），TUI 属 CSS-5 之后；
7. **不替换 VFS**：`FileSystem` + 挂载表路由保留（第 0 节边界 2）；
8. **不承诺 ext4/NTFS/Btrfs 的完整读写**（第 7 节只承诺只读优先）；
9. **不做写回修复**：`fsck` 只做**只读检查**（复算结构、报 `health`/证据，不改盘）；修复（写回元数据、
   重建结构）与写入同级风险，依赖 CSS-4 的写路径与开放问题 3 的掉电一致性定夺，第一版明确不做。

## 12. 风险与开放问题（需要定夺，未定不进实现）

1. **第三方文件系统 crate 政策**：从零实现（零依赖，但要自己写 ext2/exFAT/NTFS）还是引入受审依赖
   （先改 `check-boundary.py` 的 K4 与依赖白名单）？筛选条件见 7.2。**定夺前不开工 CSS-4**
   （只读的 CSS-1…CSS-3 不受影响）。
2. **卷标与字母的持久化落点**：CSS-3 依赖 CKM-2 的可写区（6.6）。若 CKM-2 延后，CSS-3 的验收只到
   「确定规则下字母稳定」，不假装已持久化。
3. **写入的掉电一致性**：FAT/exFAT/ext2 的多结构更新如何收敛（最小日志 vs 明确不保证 + `ro-dirty`）？
   见第 8 节第 3 条，CSS-4 开工前定。
4. **GPT / exFAT 的独立复核手段缺失**（两个不同对象、同一类问题）：GPT —— 实测 `partx`/`blkid`
   对**镜像文件**不枚举 GPT（合规主/备表头也一样），本机也没有 `sfdisk`/`parted`/`sgdisk`；
   exFAT —— 没有 `mkfs.exfat`/`exfatprogs`，也不能依赖 Linux 内核 `exfat` 驱动挂载（沙箱内无权限/
   无 loop 设备保证）。备选：(a) 自己写第二种独立解析器（弱证据，但两套实现的差异仍有价值）；
   (b) 在装了 `gdisk`/`parted`/`exfatprogs` 的环境跑一次性复核并把输出记进文档。
   **这是 CSS-1（GPT）与 CSS-4（exFAT）两处的实际风险**，不是可以含糊过去的细节。
5. **NTFS 只读的实现成本**：MFT、属性/运行列表、`$Bitmap`、压缩/稀疏、USN —— 第一版只承诺
   「能列出目录、能读文件内容」，不承诺元数据完整（时间戳/ACL/交替数据流）。
6. **大小写语义的统一**：能力位（5.1）+ 解析层查询（4.4）只是机制；「NTFS 卷上 `/data` 与 `/DATA`
   是否同一文件」这类语义需要一个**逐格式的测试夹具**，否则容易只写在文档里。
7. **探测歧义**：多个后端同时「命中」（5.3）时只报 `degraded` 不挂载 —— 是否需要一个显式的
   「按用户指定格式挂载」参数（`flags` 里预留位）？需要一次 ABI 评审（加参数会改 262 的语义）。
8. **卷标重名**：`/volumes/<label>` 重名退化到 `/volumes/<vol_id>`（4.3）后，用户看到的路径与
   预期不一致；是否需要「重名时加序号」（`data-2`）？倾向不加（隐式改名更难排查）。
9. **GPT 主备表不一致**：主表 CRC 失败但备表完好时，(a) 用备表继续（并报 `degraded`）还是
   (b) 拒绝解析？第一版倾向前者，但必须把「用的是哪张表」打进证据行。

## 13. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-12 | **CSS-3 大部分落地（盘符）** —— 新增 `kernel/src/storage/letters.rs`（固定容量盘符表：`LETTER_SLOTS = MAX_VOLUMES`，缺省规则 `C` 给系统卷、其余按 6.5 顺序给 `D`/`E`/…，**自定义**映射持久化到 CKM 状态区键 `storage/letters`；`letter = 0` 取消回到规则；非法字母 `EINVAL`、占用 `EEXIST`、超容量 `ENOSPC`、未知卷 `ENOENT`）；`storage/registry.rs` 增 `letter_of`/`slot_of`/`assign_letter`/`reassign`（盘符字段写进卷记录偏移 24）；`syscall/storage.rs` 把 **262 `storage_mount`**（已挂载卷只加别名、flags bit0 只读）/ **263 `storage_unmount`**（卸别名、卸 `/` → `EINVAL`）/ **264 `storage_assign_letter`** 从 ENOSYS 桩上拿下（三者的 CKM 判定点 8 `CAP_STORAGE_ADMIN`，参数校验先于能力判定）；`kernel/src/fs/mod.rs` 增 `mount_volume_by_id`/`unmount_point`；`userland/libc/src/path.rs` 新增 `is_windows`/`resolve`/`resolve_str`（查 `storage_volumes` 把 `C:\x` 重写为 POSIX，不硬编码字母）+ `storage_mount`/`storage_unmount`/`storage_assign_letter` 封装；`userland/user/volumes` 扩为 `list`/`mount`/`unmount`/`letter`/`demo` 子命令；内核自测 `letters::selftest`（默认规则/冲突拒绝/持久化往返/取消回退，`make test-storage` 断言 `checks=36`、`test-storage-mbr` `checks=22`）；`make test-storage` 增 `letter=` 字段与盘符断言，`test-programs` 增 `volumes demo`（双入口 + 卸载），`test-ckm` 增零能力拒绝；本文第 2/4/6/9/10 节同步标注现状（仍是唯一**未**做：`/storage/**` 只读状态树、卷标写/改名、Shell 提示符改用盘符表） |
| 2026-10-11 | **CSS-2 落地（VFS 收口）** —— `fs::mount::MountTable` 拆成 `backends`/`mounts`（一个后端可被多个挂载点引用）、条目带 `vol_id`/`options`、新增 `alias_volume`/`umount`/`route`/`sync_counts`（挂载计数写回卷记录偏移 56）；`FileSystem` 增 `caps`/`volume_info`（保守缺省）并在 RAMFS（可写）/FAT（只读 + 大小写不敏感 + 卷标）落地；FAT 探测收归 `fs/fat.rs`（`probe`/`probe_sector`/`bp_identity`，删掉 `storage/volume.rs` 里重复的 `probe_fat`，`Geometry::parse` 校验卷不大于其所在分区）、`FatFs::open` 改返回 `OpenError { errno, reason }`；`fs::mount_disks` 按卷表挂载（带 `vol_id`，分区里拿不到身份的退化为非卷挂载）并保留 `/disk`、`/mnt/fat` 兼容点；`fs: stat` 行尾带 `vol=`、新增 `storage: mount point=… vol=… ro=…`/`storage: volcheck`/`storage: unmount`/`fs: route path=… vol=…` 证据行与 `fs::storage_mount_selftest`（开机跑：只读原因来自能力位、后端与记录一致、同一卷多挂载点同字节、卸载后 `ENOENT` 且卷仍在表、`ENOENT`/`EINVAL`/`EEXIST` 反例）；`tests/test_boot.py` 增 `--expect-re`（带反向引用，因为启动盘卷序列号随镜像构建变化）；`make test-storage` 31 项 + 2 正则、`test-storage-mbr` 14 项 + 1 正则，`make test-programs` 的 `mounts=0` → `mounts=1`（记录现在真的反映挂载表）；本文档第 2/5/9/10 节同步标注现状 |
| 2026-10-11 | 创建本文档：定稿**卷模型**（四层：设备→分区→卷→挂载点/盘符；`Volume` 记录；`vol_id` 三条确定性生成规则；只读原因与健康状态各成枚举；`vol_id` 冲突必须报出）、**路径/盘符/挂载**（双视图、`C:\` 由用户态库重写而内核只认 POSIX 路径、默认/可移动挂载点与重名规则、大小写按后端能力位、错误语义追加 `ENODEV`/`ENOTSUP`）、**VFS 收口与 CKM 接口**（`FileSystem` 只加 `caps`/`volume_info`/`probe` 三个只读方法、挂载表带 `vol_id`/`options`、挂载错误从 `&'static str` 改 errno、`kernel/src/storage/` 模块划分、探测顺序与歧义处理；CKM 侧 2 个能力位 + 3 个判定点 + 5 个事件 + 号段 261–266 与 `SYS_MAX` 267 + 卷 64 字节/分区 48 字节定长记录 + `/storage/**` 状态树 + 串口证据格式）；另含文件系统支持路线（FAT→exFAT→ext2 优先，ext4/NTFS 只读优先）与第三方依赖政策、写入语义五条、CSS-1…CSS-6 分期与 11 条验收（含宿主工具**实测**：`mtools`/`partx`/`blkid`/`mkfs.ext2`/`dumpe2fs` 可用，**没有** `sfdisk`/`parted`/`sgdisk`/`mkfs.fat`/`mkfs.exfat`；并用手工 GPT/DOS 镜像实测出「`partx` 能枚举 MBR 分区但不枚举 GPT、`blkid` 只报最外层 `PTTYPE`」，因此 GPT 在本机没有独立复核工具 —— 已记入开放问题 4）、非目标与 9 个开放问题；同步 ckm.md（能力位表、判定点表、事件枚举、号段说明）、docs/README.md、DEVELOPMENT.md（H-08 行 + 5.13）、roadmap.md、architecture.md 第 5 节、project-structure.md（规划目录）、README.md、tools/README.md、filesystem.md |
| 2026-10-11 | 按「Windows 式管理 + Linux 式文件系统」评审补四处边界：**6.1** 增「覆盖层/回滚/恢复归 CKM」行（CSS 不实现第二套覆盖机制）；新增 **6.8 文件系统后端装载政策**（第一版编译内置、不经 CKM 运行时装卸、可独立更新驱动不在第一版承诺内）；**6.7** 明确 `fs=`/`ro=`/`health=` 即「兼容性」的完整展示、不得压成布尔；**第 9 节** 的 `fsck` 标注只读检查、**第 11 节** 增第 9 条「不做写回修复」 |
| 2026-10-11 | **CSS-1 落地** —— 新建 `kernel/src/storage/{mod,registry,partition,gpt,mbr,volume,selftest}.rs`：GPT（主/备表头 + CRC32 + 128 项）与 MBR（4 主分区 + EBR 链）只读扫描，无分区表退化为整盘一卷，探测识别 FAT；`driver/block.rs` 注册表 2 → 8；`syscall/storage.rs` 新增 `storage_volumes`(261)/`storage_rescan`(265)（定长 64 字节卷记录、只读、确定性顺序、`storage_volumes` 无能力位要求）；`tools/mkfs/mkfs.py` 增 GPT/MBR/主表损坏/无表四类夹具；`libc` 增 `storage_volumes` 解析器，新增用户程序 `/bin/volumes`；新增 `make storage-images`/`test-storage-fixtures`/`test-storage`/`test-storage-mbr` 与编排场景 `storage`、`storage-mbr`（15 → **17 场景**）；内核解析自测 11 例 / 45 断言（含主表头损坏回退备用表头、CRC 失败 → `degraded`）；本文档第 2/9/10 节同步标注现状 |
