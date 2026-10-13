//! 系统调用号与号段规则（`docs/syscall.md` 3 节是文字规格，本模块是**唯一数字来源**）。
//!
//! 编号沿用 **Linux x86_64** 语义（便于以后直接跑现成的用户态程序）：
//! `read=0, write=1, open=2, close=3, mmap=9, mprotect=10, munmap=11, brk=12,
//! `nanosleep=35, getpid=39, socket=41, sendto=44, recvfrom=45, bind=49,
//! fork=57, execve=59, exit=60, wait4=61`，
//! 目录列举沿用 Linux 的 `getdents=78`（**记录格式不同**，见 [`crate::dirent`]）。
//!
//! **号段规则（阶段六冻结，现已生效）**：
//!
//! | 号段 | 语义 | 规矩 |
//! | ---- | ---- | ---- |
//! | `0..=255` | Linux x86_64 | 只按 Linux 语义**新增**，不复用、不改语义 |
//! | `>= 256` | Cerlesse 扩展 | 号一旦发布**不再回收**（`docs/ckm.md` 5.3 同一原则） |
//!
//! 扩展号段内的分配：`256` = `getprocs`，`257..=260` = CKM，
//! `261..=266` = 存储（CSS），`267` = `net_config`，`268` = [`SYS_MAX`]（不含）。
//!
//! 表按 `SYS_MAX` 排满：未单独实现的号落在公共 ENOSYS 桩上，因此
//! 「号合法但未实现」与「号越界」是两种可区分的失败 —— 前者在
//! [`UNIMPLEMENTED`] 里有名字（串口打出 `syscall: ENOSYS nr=… name=…`），
//! 后者连号都不认识。

/// `read(fd, buf, count)`。
pub const SYS_READ: u64 = 0;
/// `write(fd, buf, count)`。
pub const SYS_WRITE: u64 = 1;
/// `open(path_ptr, path_len, flags)`（路径是**指针 + 长度**，不是 NUL 结尾）。
pub const SYS_OPEN: u64 = 2;
/// `close(fd)`。
pub const SYS_CLOSE: u64 = 3;
/// `mmap(len, prot, flags)`（私有匿名，**没有 offset 参数**）。
pub const SYS_MMAP: u64 = 9;
/// `mprotect(addr, len, prot)`。
pub const SYS_MPROTECT: u64 = 10;
/// `munmap(addr, len)`。
pub const SYS_MUNMAP: u64 = 11;
/// `brk(addr)`（沿用 Linux 语义：失败返回原值，不返回错误）。
pub const SYS_BRK: u64 = 12;
/// `msync(addr, len, flags)`（v1.0 第五步：把共享文件映射的页写回文件）。
///
/// 号沿用 Linux x86_64 的 `msync`。只对 `MAP_SHARED` 的文件映射页有效；
/// 其余页（匿名 / `MAP_PRIVATE`）跳过 —— 它们本来就不该落盘。
pub const SYS_MSYNC: u64 = 26;
/// `nanosleep(ticks)`（Linux 名 `nanosleep`，单位是内核 tick 而不是 `timespec`）。
pub const SYS_SLEEP: u64 = 35;
/// `getpid()`。
pub const SYS_GETPID: u64 = 39;

// ---- Socket（AF_INET + SOCK_DGRAM，v1.0）----
// 号沿用 Linux x86_64：socket=41、sendto=44、recvfrom=45、bind=49。
// `connect`/`listen`/`accept`（42/50/43）仍是后续项：这里**不占号** ——
// 「号不存在」与「号存在但未实现」是两种可区分的失败（见下 `UNIMPLEMENTED`）。
/// `socket(domain, type, protocol)`：创建一个套接字，返回 fd（≥ 3）。
/// 只支持 `AF_INET` + `SOCK_DGRAM`（UDP）；`SOCK_STREAM` 返回 `-EPROTONOSUPPORT`。
pub const SYS_SOCKET: u64 = 41;
/// `sendto(fd, buf, len, flags, addr_ptr, addr_len)`：向 `addr` 发一个 UDP 数据报。
pub const SYS_SENDTO: u64 = 44;
/// `recvfrom(fd, buf, len, flags, addr_ptr, addr_len_ptr)`：收一个 UDP 数据报，
/// 回填发送方地址（`addr_len_ptr` 是「缓冲长度进、实际长度出」的指针）。
pub const SYS_RECVFROM: u64 = 45;
/// `bind(fd, addr_ptr, addr_len)`：把套接字绑到本机地址/端口（`port = 0` 分配临时端口）。
pub const SYS_BIND: u64 = 49;
/// `fork()`。
pub const SYS_FORK: u64 = 57;
/// `execve(path_ptr, path_len, argv_ptr, argc, 0)`（`argv` 是「指针 + 长度」对数组）。
pub const SYS_EXEC: u64 = 59;
/// `exit(code)`。
pub const SYS_EXIT: u64 = 60;
/// `wait4(pid)`。
pub const SYS_WAIT: u64 = 61;
/// `getdents(path_ptr, path_len, buf_ptr, buf_len)`（Linux 同名号，记录格式见 [`crate::dirent`]）。
pub const SYS_GETDENTS: u64 = 78;
/// `getprocs(buf_ptr, buf_len)`（**Cerlesse 扩展号**：Linux 没有「列举进程」的系统调用，
/// `ps` 走 `/proc`，而本内核的 VFS 还没有虚拟文件）。
pub const SYS_GETPROCS: u64 = 256;
/// `ckm_policy_query(op, arg, out_ptr, out_len, 0)`：返回某 op 在**当前凭证**下的判定。
/// 无能力要求（只能查自己，没有参数能指定别的主体）。
pub const SYS_CKM_POLICY_QUERY: u64 = 257;
/// `ckm_audit_read(out_ptr, out_len, since_seq, 0, 0)`：需要 [`crate::ckm::CAP_AUDIT_READ`]。
pub const SYS_CKM_AUDIT_READ: u64 = 258;
/// `ckm_component_state(buf_ptr, buf_len)`：只读导出组件状态记录。
pub const SYS_CKM_COMPONENT_STATE: u64 = 259;
/// `ckm_policy_set(...)`（阶段 4 才定义行为）：需要 [`crate::ckm::CAP_CKM_ADMIN`]。
pub const SYS_CKM_POLICY_SET: u64 = 260;
/// `storage_volumes(buf_ptr, buf_len, out_count_ptr, 0, 0)`：卷表快照。
pub const SYS_STORAGE_VOLUMES: u64 = 261;
/// `storage_mount(vol_id_ptr, vol_id_len, point_ptr, point_len, flags)`。
pub const SYS_STORAGE_MOUNT: u64 = 262;
/// `storage_unmount(point_ptr, point_len, 0, 0, 0)`。
pub const SYS_STORAGE_UNMOUNT: u64 = 263;
/// `storage_assign_letter(vol_id_ptr, vol_id_len, letter, 0, 0)`。
pub const SYS_STORAGE_ASSIGN_LETTER: u64 = 264;
/// `storage_rescan(disk_index, 0, 0, 0, 0)`：`u32::MAX` = 全部设备（[`crate::volume::RESCAN_ALL`]）。
pub const SYS_STORAGE_RESCAN: u64 = 265;
/// `storage_format(...)`（CSS-4 才实现）。
pub const SYS_STORAGE_FORMAT: u64 = 266;

/// `net_config(buf_ptr, buf_len)`（**Cerlesse 扩展号**）：导出内核在 DHCP 里协商到的
/// 网络配置（本机 IPv4 / 网关 / MAC，记录布局见 [`crate::socket`]）。
/// 没有网卡时返回 `-ENODEV` —— 用户态据此判断「要不要跑网络相关自测」。
pub const SYS_NET_CONFIG: u64 = 267;

/// 系统调用号上限（**不含**）：`parse()` 对 `nr >= SYS_MAX` 返回「号越界」。
///
/// 号段 `256..SYS_MAX` 是 Cerlesse 扩展段（见模块头）。改这个数字等于扩号段，
/// 必须同时扩 [`UNIMPLEMENTED`]/表长（内核的 `SYS_TABLE` 以它为长度）。
pub const SYS_MAX: u64 = 268;

/// Linux 号段的结束（不含）：`0..LINUX_SEGMENT_END` 是 Linux x86_64 语义。
pub const LINUX_SEGMENT_END: u64 = 256;
/// Cerlesse 扩展号段的起点（含）。
pub const CERLESSE_SEGMENT_START: u64 = 256;

/// 号是否落在 Linux 号段（`0..=255`）。
pub const fn is_linux_segment(nr: u64) -> bool {
    nr < LINUX_SEGMENT_END
}

/// 号是否落在 Cerlesse 扩展号段（`256..SYS_MAX`）。
pub const fn is_cerlesse_segment(nr: u64) -> bool {
    nr >= CERLESSE_SEGMENT_START && nr < SYS_MAX
}

/// 所有**有名字**的调用号（按号升序；[`crate::record`] 与自测用它断言唯一性）。
///
/// 这张表是「已发布的号」清单：加号时同时加到这里，测试会拦住重复与越界。
pub const NAMED: [u64; 32] = [
    SYS_READ,
    SYS_WRITE,
    SYS_OPEN,
    SYS_CLOSE,
    SYS_MMAP,
    SYS_MPROTECT,
    SYS_MUNMAP,
    SYS_BRK,
    SYS_MSYNC,
    SYS_SLEEP,
    SYS_GETPID,
    SYS_SOCKET,
    SYS_SENDTO,
    SYS_RECVFROM,
    SYS_BIND,
    SYS_FORK,
    SYS_EXEC,
    SYS_EXIT,
    SYS_WAIT,
    SYS_GETDENTS,
    SYS_GETPROCS,
    SYS_CKM_POLICY_QUERY,
    SYS_CKM_AUDIT_READ,
    SYS_CKM_COMPONENT_STATE,
    SYS_CKM_POLICY_SET,
    SYS_STORAGE_VOLUMES,
    SYS_STORAGE_MOUNT,
    SYS_STORAGE_UNMOUNT,
    SYS_STORAGE_ASSIGN_LETTER,
    SYS_STORAGE_RESCAN,
    SYS_STORAGE_FORMAT,
    SYS_NET_CONFIG,
];

/// 已定义但尚未实现的系统调用号：`(号, 名字)`。
///
/// 显式列出而不是「不写」：查表时它们落在 ENOSYS 桩上，与「号越界」是两种不同的
/// 失败原因，调试时不会被混为一谈。内核 `syscall_dispatch` 命中时打印
/// `syscall: ENOSYS nr=<n> name=<名字>`，于是「谁在依赖未实现的调用」在串口上可断言。
pub const UNIMPLEMENTED: [(u64, &str); 2] = [
    (SYS_CKM_POLICY_SET, "ckm_policy_set"),
    (SYS_STORAGE_FORMAT, "storage_format"),
];

/// 查「已命名但未实现」的号对应的名字（[`UNIMPLEMENTED`] 的查询接口）。
pub fn unimplemented_name(nid: u64) -> Option<&'static str> {
    let mut i = 0;
    while i < UNIMPLEMENTED.len() {
        if UNIMPLEMENTED[i].0 == nid {
            return Some(UNIMPLEMENTED[i].1);
        }
        i += 1;
    }
    None
}
