//! CKM 管理接口的 ABI（`docs/ckm.md` 4 节）：记录布局、能力位、事件编号、受保护前缀。
//!
//! 与 syscall 号段一样，这里是**唯一数字来源**：内核 `ckm::*` 与 libc 都引用它，
//! 审计记录的 `need` 字段与串口证据里的 `need=0x4` 因此不可能与用户态的理解漂移。
//!
//! 边界的重申（`docs/ckm.md` 第 0 节）：内核只提供**机制**（判定点、记录格式、
//! 状态机与审计环），包安装 / 事务 / 依赖求解 / 管理 CLI 属于用户态（`ckmd`/`ckm`）。
//! 因此这个模块里**没有**目录布局、没有事务阶段、没有 catalog 格式。
//!
//! | 记录 | 尺寸 | 权威结构 |
//! | ---- | ---- | ---- |
//! | 审计记录 | 48 | `ckm::audit::Record` |
//! | 组件状态记录 | 64 | `ckm::state` 的 ABI 列 |
//!
//! `ckm_policy_query`（257）复用审计记录布局，`seq = 0` 表示「这不是事件」。

/// CKM ABI 版本（第一版 = 1）：扩展清单声明 `abi.kernel` 后按**相等**判定。
///
/// 它与 [`crate::syscall::SYS_MAX`] 是**两个独立版本号**：前者约束管理接口/记录布局，
/// 后者约束「哪个调用号存在」。
pub const ABI_VERSION: u32 = 1;

/// 审计记录长度（字节）。
pub const AUDIT_RECORD_SIZE: usize = 48;
/// 组件状态记录长度（字节）。
pub const COMPONENT_RECORD_SIZE: usize = 64;
/// 组件 `id` 字段里可读字符的上限（记录占 16 字节：15 字符 + NUL）。
pub const COMPONENT_ID_MAX: usize = 15;
/// 组件/扩展状态表容量（固定容量 + 明确拒绝，与驱动注册表、进程表同风格）。
pub const MAX_COMPONENTS: usize = 32;

/// `CAP_CKM_ADMIN`：改 CKM 策略 / 授予或撤销能力 / 决定进入安全模式。
pub const CAP_CKM_ADMIN: u32 = 0x1;
/// `CAP_CKM_SERVICE`：执行扩展事务（启停组件、写扩展状态树、注册覆盖层）。
pub const CAP_CKM_SERVICE: u32 = 0x2;
/// `CAP_FS_SYSTEM_WRITE`：写受保护前缀（[`PROTECTED_PREFIXES`]）。
pub const CAP_FS_SYSTEM_WRITE: u32 = 0x4;
/// `CAP_AUDIT_READ`：读审计环形缓冲（`ckm_audit_read` = 258）。
pub const CAP_AUDIT_READ: u32 = 0x8;
/// `CAP_STORAGE_ADMIN`：存储管理的**可逆**动作（挂载/卸载/分配或取消盘符/重扫）。
pub const CAP_STORAGE_ADMIN: u32 = 0x10;
/// `CAP_STORAGE_FORMAT`：创建/格式化文件系统（**破坏性**，与可逆动作分开）。
pub const CAP_STORAGE_FORMAT: u32 = 0x20;
/// 全部能力位（内核上下文持有）。
pub const CAP_ALL: u32 = CAP_CKM_ADMIN
    | CAP_CKM_SERVICE
    | CAP_FS_SYSTEM_WRITE
    | CAP_AUDIT_READ
    | CAP_STORAGE_ADMIN
    | CAP_STORAGE_FORMAT;

/// `fs_write`：写受保护前缀。
pub const OP_FS_WRITE: u16 = 1;
/// `state_write`：写扩展状态树 `/ckm/**`。
pub const OP_STATE_WRITE: u16 = 2;
/// `policy_set`：改 CKM 策略。
pub const OP_POLICY_SET: u16 = 3;
/// `audit_read`：读审计缓冲。
pub const OP_AUDIT_READ: u16 = 4;
/// `policy_query`：查自己的判定（无能力要求）。
pub const OP_POLICY_QUERY: u16 = 5;
/// `exec_component`：`execve` 一个组件程序（换 uid/gid/能力）。
pub const OP_EXEC_COMPONENT: u16 = 6;
/// `overlay_register`：注册覆盖层。
pub const OP_OVERLAY_REGISTER: u16 = 7;
/// `overlay_conflict`：覆盖冲突。
pub const OP_OVERLAY_CONFLICT: u16 = 8;
/// `safe_mode_enter`：进入安全模式。
pub const OP_SAFE_MODE_ENTER: u16 = 9;
/// `txn_commit`：事务提交（由用户态 `ckmd` 发起，内核只判定）。
pub const OP_TXN_COMMIT: u16 = 10;
/// `txn_rollback`：事务回滚。
pub const OP_TXN_ROLLBACK: u16 = 11;
/// `storage_mount`：挂载卷。
pub const OP_STORAGE_MOUNT: u16 = 12;
/// `storage_unmount`：卸载卷。
pub const OP_STORAGE_UNMOUNT: u16 = 13;
/// `storage_letter`：分配/取消盘符。
pub const OP_STORAGE_LETTER: u16 = 14;
/// `storage_format`：创建/格式化文件系统。
pub const OP_STORAGE_FORMAT: u16 = 15;
/// `storage_rescan`：重扫设备。
pub const OP_STORAGE_RESCAN: u16 = 16;

/// 受保护前缀：写它们额外要求 [`CAP_FS_SYSTEM_WRITE`]。**下标就是 ABI**
/// （`fs_write` 的 `arg` 与审计/证据行的 `detail` 都是它）。
pub const PROTECTED_PREFIXES: [&str; 4] = ["/etc", "/bin", "/lib", "/init"];
/// 受保护前缀下标：`/etc`（[`PROTECTED_PREFIXES`] 的顺序即下标）。
pub const PREFIX_ETC: u64 = 0;
/// 受保护前缀下标：`/bin`。
pub const PREFIX_BIN: u64 = 1;
/// 受保护前缀下标：`/lib`。
pub const PREFIX_LIB: u64 = 2;
/// 受保护前缀下标：`/init`。
pub const PREFIX_INIT: u64 = 3;
