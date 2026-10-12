//! 能力位（v1.0 落地：**位定义**；判定在 `ckm::policy`，见 `docs/ckm.md` 3.2）
//!
//! 第一版 6 位 = CKM 4 位 + 存储 2 位。位号就是 ABI：审计记录的 `need` 字段
//! （`docs/ckm.md` 4.4）与串口证据的 `need=0x4` 都是这些常量，因此**只有这一个定义点**，
//! 内核其它模块必须引用这里而不是重写数字。
//!
//! | 能力 | 位 | 语义 | 谁持有 |
//! | ---- | ---- | ---- | ---- |
//! | [`CAP_CKM_ADMIN`] | `0x1` | 改 CKM 策略 / 授予能力 / 进安全模式 | uid 1000 |
//! | [`CAP_CKM_SERVICE`] | `0x2` | 执行扩展事务：启停组件、写扩展状态树 | uid 1001 |
//! | [`CAP_FS_SYSTEM_WRITE`] | `0x4` | 写受保护前缀（`/etc`、`/bin`、`/lib`、`/init`） | uid 0 的 `/init`、1000、1001 |
//! | [`CAP_AUDIT_READ`] | `0x8` | 读审计环形缓冲 | 1000、1001 |
//! | [`CAP_STORAGE_ADMIN`] | `0x10` | 存储管理的**可逆**动作：挂载/卸载/盘符/重扫 | 1000 |
//! | [`CAP_STORAGE_FORMAT`] | `0x20` | 创建/格式化文件系统（**破坏性**，与上面分开） | 1000 |
//!
//! 存储两位拆开的理由（`docs/storage.md` 6.2）：挂载/卸载是可逆的日常操作，
//! 格式化会毁数据 —— 合成一位就无法只给其一。`/init`(0) 与 `ckmd`(1001)
//! **都不持有**存储能力：建立系统视图与包事务都不需要改字母表。

/// 六个能力位（**再导出**，位值定义在 `abi::ckm` —— 位号就是 ABI：审计记录的
/// `need` 字段与串口证据的 `need=0x4` 都是这些常量，因此只有**这一处**定义）。
///
/// | 能力 | 位 | 语义 |
/// | ---- | ---- | ---- |
/// | [`CAP_CKM_ADMIN`] | `0x1` | 改 CKM 策略 / 授予或撤销能力 / 决定进入安全模式 |
/// | [`CAP_CKM_SERVICE`] | `0x2` | 执行扩展事务（启停组件、写扩展状态树、注册覆盖层） |
/// | [`CAP_FS_SYSTEM_WRITE`] | `0x4` | 写受保护前缀（`/etc`、`/bin`、`/lib`、`/init`） |
/// | [`CAP_AUDIT_READ`] | `0x8` | 读审计环形缓冲（`ckm_audit_read` = 258） |
/// | [`CAP_STORAGE_ADMIN`] | `0x10` | 存储管理的**可逆**动作（挂载/卸载/盘符/重扫） |
/// | [`CAP_STORAGE_FORMAT`] | `0x20` | 创建/格式化文件系统（**破坏性**，CSS-4 起使用） |
///
/// 存储两位拆开的理由（`docs/storage.md` 6.2）：挂载/卸载是可逆的日常操作，
/// 格式化会毁数据 —— 合成一位就无法只给其一。`/init`(0) 与 `ckmd`(1001)
/// **都不持有**存储能力：建立系统视图与包事务都不需要改字母表。
pub use abi::ckm::{
    CAP_AUDIT_READ, CAP_CKM_ADMIN, CAP_CKM_SERVICE, CAP_FS_SYSTEM_WRITE, CAP_STORAGE_ADMIN,
    CAP_STORAGE_FORMAT,
};

/// 全部能力位（内核上下文持有）。
pub const ALL: u32 = abi::ckm::CAP_ALL;

/// 管理员（uid 1000）的能力集：**不含** [`CAP_CKM_SERVICE`]
/// —— 管理员改策略而不是执行组件事务，两者是不同主体（`docs/ckm.md` 3.1）。
pub const ADMIN: u32 = CAP_CKM_ADMIN
    | CAP_FS_SYSTEM_WRITE
    | CAP_AUDIT_READ
    | CAP_STORAGE_ADMIN
    | CAP_STORAGE_FORMAT;

/// `/init`（uid 0）的能力集：只有系统写入，**没有** CKM 管理能力与存储能力。
pub const INIT: u32 = CAP_FS_SYSTEM_WRITE;

/// 管理服务（uid 1001）的能力集：能执行事务、能读审计，**不能**改策略。
pub const SERVICE: u32 = CAP_CKM_SERVICE | CAP_FS_SYSTEM_WRITE | CAP_AUDIT_READ;

// 能力集之间的**编译期**关系（阶段三：单一事实源要能被断言，而不是靠人读注释）：
//   - `ALL` 就是 ABI 里的全集；
//   - `ADMIN` = 全集去掉「执行事务」；
//   - `SERVICE` = 全集去掉 CKM 管理与两个存储位。
// 任一条被改坏，构建就失败 —— 这正是这三个集合必须与 ABI 一致的地方。
const _: () = assert!(ALL == abi::ckm::CAP_ALL);
const _: () = assert!(ADMIN == abi::ckm::CAP_ALL & !CAP_CKM_SERVICE);
const _: () = assert!(
    SERVICE | CAP_CKM_ADMIN == abi::ckm::CAP_ALL & !CAP_STORAGE_ADMIN & !CAP_STORAGE_FORMAT
);
