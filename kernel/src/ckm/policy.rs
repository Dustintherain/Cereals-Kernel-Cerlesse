//! 判定点表（`docs/ckm.md` 3.4 / 4.6）：把「谁能做什么」变成内核里的强制点
//!
//! 这一层不读磁盘、不做事务（那是 CKM-2/CKM-3），只回答一个问题：
//! **当前主体的能力位够不够做这个 op**。每次判定都产生三样东西（缺一即验收不过）：
//!
//! 1. 返回值（`0` 或 `-EPERM` = -1）；
//! 2. 一条 48 字节审计记录（[`crate::ckm::audit`]）；
//! 3. 一行串口证据 `ckm: policy op=<短名> pid=<n> uid=<n> result=allow|deny need=0x<位> detail=0x<n>`
//!    —— **allow 与 deny 都打**，因此「内核真的判断过」是可断言的串口事实。
//!
//! 受保护前缀（[`PROTECTED_PREFIXES`]）是**内核常量**，第一版不允许运行时增删
//! （阶段 4 起可由管理员改，且改动本身进审计）。写入它们的判定失败是 `EPERM(-1)`，
//! 与文件 mode 判定失败 `EACCES(-13)` **刻意分开**：一个是「你没有这个能力」，
//! 一个是「这个文件不属于你」（`docs/ckm.md` 3.3）。

use crate::driver::serial;
use crate::security::capability::{
    CAP_AUDIT_READ, CAP_CKM_ADMIN, CAP_CKM_SERVICE, CAP_FS_SYSTEM_WRITE, CAP_STORAGE_ADMIN,
    CAP_STORAGE_FORMAT,
};
use crate::security::credential::Cred;
use crate::security::permission::EPERM;

/// 审计/判定结果：允许。
pub const RESULT_ALLOW: u8 = 0;
/// 审计/判定结果：拒绝。
pub const RESULT_DENY: u8 = 1;

/// 受保护前缀（**再导出**，定义在 `abi::ckm`：前缀**下标就是 ABI**，
/// `fs_write` 的 `arg` 与证据行的 `detail` 都用它，见 `abi::ckm::PREFIX_*`）。
/// 写它们额外要求 [`CAP_FS_SYSTEM_WRITE`]。
///
/// 边界判定按路径分隔符做（[`is_protected`]）：`/etc/motd` 命中，
/// `/etcetera` **不**命中 —— 否则「前缀」会变成「字符串开头」，这是安全判定里
/// 最经典的一类错误。
pub use abi::ckm::PROTECTED_PREFIXES;

/// 路径是否落在某个受保护前缀下（边界安全）。
pub fn is_protected(path: &str) -> bool {
    protected_index(path).is_some()
}

/// 命中受保护前缀时返回它的下标（`detail` 里用；`/etc` = 0、`/bin` = 1、`/lib` = 2、`/init` = 3）。
pub fn protected_index(path: &str) -> Option<usize> {
    let bytes = path.as_bytes();
    for (i, prefix) in PROTECTED_PREFIXES.iter().enumerate() {
        let p = prefix.as_bytes();
        if bytes.len() < p.len() || &bytes[..p.len()] != p {
            continue;
        }
        // 正好相等（`/init`）或后面紧跟目录分隔符（`/etc/motd`）
        if bytes.len() == p.len() || bytes[p.len()] == b'/' {
            return Some(i);
        }
    }
    None
}

/// 操作 / 事件类型（`docs/ckm.md` 4.6 的编号，用户态镜像一次）。
///
/// 只有**第一版真的会走到**的 op 才实现判定；其余的先是定义（号一旦发布不再回收）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    /// 1 `fs_write`：写受保护前缀
    FsWrite,
    /// 2 `state_write`：写扩展状态树 `/ckm/**`
    StateWrite,
    /// 3 `policy_set`：改 CKM 策略（阶段 4）
    PolicySet,
    /// 4 `audit_read`：读审计缓冲
    AuditRead,
    /// 5 `policy_query`：查自己的判定（无能力要求）
    PolicyQuery,
    /// 6 `exec_component`：`execve` 一个组件程序（换 uid/gid/能力）
    ExecComponent,
    /// 7 `overlay_register`：注册覆盖层（阶段 5）
    OverlayRegister,
    /// 8 `overlay_conflict`：覆盖冲突（阶段 5）
    OverlayConflict,
    /// 9 `safe_mode_enter`：进入安全模式（阶段 6）
    SafeModeEnter,
    /// 10 `txn_commit`：事务提交（阶段 3）
    TxnCommit,
    /// 11 `txn_rollback`：事务回滚（阶段 3）
    TxnRollback,
    /// 12 `storage_mount`：挂载卷（CSS-3）
    StorageMount,
    /// 13 `storage_unmount`：卸载卷（CSS-3）
    StorageUnmount,
    /// 14 `storage_letter`：分配/取消盘符（CSS-3）
    StorageLetter,
    /// 15 `storage_format`：创建/格式化文件系统（CSS-4）
    StorageFormat,
    /// 16 `storage_rescan`：重扫设备（CSS-1 已有机制，CKM-1 起受控）
    StorageRescan,
}

impl Op {
    /// 编号（进审计记录的 `op` 字段；`docs/ckm.md` 4.6）。
    pub const fn code(self) -> u16 {
        // 编号**定义在 `abi::ckm`**（阶段三的单一事实源）：用户态按同一批编号
        // 调用 `ckm_policy_query` 并解析审计记录，两边不可能漂移。
        match self {
            Op::FsWrite => abi::ckm::OP_FS_WRITE,
            Op::StateWrite => abi::ckm::OP_STATE_WRITE,
            Op::PolicySet => abi::ckm::OP_POLICY_SET,
            Op::AuditRead => abi::ckm::OP_AUDIT_READ,
            Op::PolicyQuery => abi::ckm::OP_POLICY_QUERY,
            Op::ExecComponent => abi::ckm::OP_EXEC_COMPONENT,
            Op::OverlayRegister => abi::ckm::OP_OVERLAY_REGISTER,
            Op::OverlayConflict => abi::ckm::OP_OVERLAY_CONFLICT,
            Op::SafeModeEnter => abi::ckm::OP_SAFE_MODE_ENTER,
            Op::TxnCommit => abi::ckm::OP_TXN_COMMIT,
            Op::TxnRollback => abi::ckm::OP_TXN_ROLLBACK,
            Op::StorageMount => abi::ckm::OP_STORAGE_MOUNT,
            Op::StorageUnmount => abi::ckm::OP_STORAGE_UNMOUNT,
            Op::StorageLetter => abi::ckm::OP_STORAGE_LETTER,
            Op::StorageFormat => abi::ckm::OP_STORAGE_FORMAT,
            Op::StorageRescan => abi::ckm::OP_STORAGE_RESCAN,
        }
    }

    /// 由编号还原（读审计记录时用；未知编号返回 `None`，**不猜**）。
    pub const fn from_code(code: u16) -> Option<Op> {
        match code {
            abi::ckm::OP_FS_WRITE => Some(Op::FsWrite),
            abi::ckm::OP_STATE_WRITE => Some(Op::StateWrite),
            abi::ckm::OP_POLICY_SET => Some(Op::PolicySet),
            abi::ckm::OP_AUDIT_READ => Some(Op::AuditRead),
            abi::ckm::OP_POLICY_QUERY => Some(Op::PolicyQuery),
            abi::ckm::OP_EXEC_COMPONENT => Some(Op::ExecComponent),
            abi::ckm::OP_OVERLAY_REGISTER => Some(Op::OverlayRegister),
            abi::ckm::OP_OVERLAY_CONFLICT => Some(Op::OverlayConflict),
            abi::ckm::OP_SAFE_MODE_ENTER => Some(Op::SafeModeEnter),
            abi::ckm::OP_TXN_COMMIT => Some(Op::TxnCommit),
            abi::ckm::OP_TXN_ROLLBACK => Some(Op::TxnRollback),
            abi::ckm::OP_STORAGE_MOUNT => Some(Op::StorageMount),
            abi::ckm::OP_STORAGE_UNMOUNT => Some(Op::StorageUnmount),
            abi::ckm::OP_STORAGE_LETTER => Some(Op::StorageLetter),
            abi::ckm::OP_STORAGE_FORMAT => Some(Op::StorageFormat),
            abi::ckm::OP_STORAGE_RESCAN => Some(Op::StorageRescan),
            _ => None,
        }
    }

    /// 串口证据里的**短名**（`docs/ckm.md` 4.5）：日志可读，改动时不会变成天书。
    pub const fn name(self) -> &'static str {
        match self {
            Op::FsWrite => "fs_write",
            Op::StateWrite => "state_write",
            Op::PolicySet => "policy_set",
            Op::AuditRead => "audit_read",
            Op::PolicyQuery => "policy_query",
            Op::ExecComponent => "exec_component",
            Op::OverlayRegister => "overlay_register",
            Op::OverlayConflict => "overlay_conflict",
            Op::SafeModeEnter => "safe_mode_enter",
            Op::TxnCommit => "txn_commit",
            Op::TxnRollback => "txn_rollback",
            Op::StorageMount => "storage_mount",
            Op::StorageUnmount => "storage_unmount",
            Op::StorageLetter => "storage_letter",
            Op::StorageFormat => "storage_format",
            Op::StorageRescan => "storage_rescan",
        }
    }

    /// 这个 op 需要的 capability 位（`0` = 只读/无能力要求）。
    ///
    /// [`Op::FsWrite`] 的返回值只对**受保护前缀**有意义：调用方先
    /// [`is_protected`] 判定，只有命中才调 [`check`]（非受保护路径不产生判定行，
    /// 也不要求任何能力）。
    pub const fn need(self) -> u32 {
        match self {
            Op::FsWrite => CAP_FS_SYSTEM_WRITE,
            Op::StateWrite => CAP_CKM_SERVICE,
            Op::PolicySet => CAP_CKM_ADMIN,
            Op::AuditRead => CAP_AUDIT_READ,
            Op::PolicyQuery => 0,
            Op::ExecComponent => 0,
            Op::OverlayRegister => CAP_CKM_SERVICE,
            Op::OverlayConflict => CAP_CKM_SERVICE,
            Op::SafeModeEnter => CAP_CKM_ADMIN,
            Op::TxnCommit => CAP_CKM_SERVICE,
            Op::TxnRollback => CAP_CKM_SERVICE,
            Op::StorageMount => CAP_STORAGE_ADMIN,
            Op::StorageUnmount => CAP_STORAGE_ADMIN,
            Op::StorageLetter => CAP_STORAGE_ADMIN,
            Op::StorageFormat => CAP_STORAGE_FORMAT,
            Op::StorageRescan => CAP_STORAGE_ADMIN,
        }
    }
}

/// 判定 `cred` 能否做 `op`：写审计记录 + 打串口证据行，返回 `0` 或 `-EPERM`。
///
/// `arg` → 证据行与审计记录里的 `detail`（op 相关：受保护前缀下标 / 组件槽位 /
/// 路径哈希）。判定**只**看能力位：文件 mode 是 `security/permission.rs` 的事。
pub fn check(op: Op, cred: &Cred, arg: u64) -> Result<(), i64> {
    let need = op.need();
    let allowed = cred.caps_ok(need);
    let result = if allowed { RESULT_ALLOW } else { RESULT_DENY };
    // 审计与证据行一一对应：拒绝了什么、谁的、缺哪一位，都可核查。
    crate::ckm::audit::record(
        crate::ckm::cred::current_pid(),
        cred.uid,
        cred.gid,
        op,
        result,
        if allowed { 0 } else { need },
        arg,
    );
    print_policy_line(op, cred, allowed, need, arg);
    if allowed {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// 判定**但不**写审计/证据（纯谓词；内核自测的能力矩阵用它与判定结果对照）。
pub fn allowed(op: Op, cred: &Cred) -> bool {
    cred.caps_ok(op.need())
}

/// **语义拒绝**：凭证够、但请求本身不合法（覆盖层前缀冲突、非法参数、槽位满）。
///
/// 与 [`check`] 的区别是**失败的理由**：那里是「缺哪一位能力」，这里是「这个请求不能成立」。
/// 两者都必须留审计与串口证据（`docs/ckm.md` 3.4 的三件套只对能力拒绝要求 errno，
/// 但「谁在什么时候被拒了」对两类失败同样重要）。
///
/// 证据行的 `need=0x0`：**没有**能力位能改变这个结果 —— 补权限不解决冲突。
/// 具体 errno 由调用方决定（冲突是 `EEXIST`、非法参数是 `EINVAL`）。
pub fn reject(op: Op, cred: &Cred, detail: u64) {
    crate::ckm::audit::record(
        crate::ckm::cred::current_pid(),
        cred.uid,
        cred.gid,
        op,
        RESULT_DENY,
        0,
        detail,
    );
    print_policy_line(op, cred, false, 0, detail);
}

/// 串口证据行（`docs/ckm.md` 4.5）。
fn print_policy_line(op: Op, cred: &Cred, allowed: bool, need: u32, arg: u64) {
    serial::print("ckm: policy op=");
    serial::print(op.name());
    serial::print(" pid=");
    serial::print_dec(crate::ckm::cred::current_pid());
    serial::print(" uid=");
    serial::print_dec(cred.uid as u64);
    serial::print(" result=");
    serial::print(if allowed { "allow" } else { "deny" });
    // `print_hex` 自带 `0x` 前缀，这里不要再补一个（否则是 `need=0x0x4`）。
    serial::print(" need=");
    serial::print_hex(if allowed { 0 } else { need as u64 });
    serial::print(" detail=");
    serial::print_hex(arg);
    serial::println("");
}
