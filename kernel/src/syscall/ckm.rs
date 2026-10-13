//! CKM 类系统调用（`docs/ckm.md` 4.3）：257 `ckm_policy_query` / 258 `ckm_audit_read`
//! / 259 `ckm_component_state`
//!
//! 与 `getprocs`/`getdents`/`storage_volumes` 同一套做法：**定长记录 + 一次调用写满**。
//! 记录布局的权威定义在内核（[`crate::ckm::audit`] 48 字节、
//! [`crate::ckm::state`] 64 字节），libc 提供解析器 —— 用户程序不自己算偏移。
//!
//! 三个调用里的**权限差别**正是 CKM 第一版的全部对外形状：
//!
//! | 号 | 调用 | 权限 | 谁被拒 |
//! | ---- | ---- | ---- | ---- |
//! | 257 | `ckm_policy_query` | 无（**只能查自己**：没有参数能指定别的进程） | 没人被拒 |
//! | 258 | `ckm_audit_read` | [`CAP_AUDIT_READ`] | `/init`(uid 0，无此位)、普通程序 |
//! | 259 | `ckm_component_state` | 无（只读） | 没人被拒 |
//!
//! 260 `ckm_policy_set` 在阶段 1–3 落在公共 ENOSYS 桩上（阶段 4 才定义）：
//! 「号已定义但未实现」与「号越界」因此是可区分的两种失败（`docs/ckm.md` 4.3）。
//!
//! [`CAP_AUDIT_READ`]: crate::security::capability::CAP_AUDIT_READ

use crate::ckm::api;
use crate::ckm::audit::AUDIT_RECORD_SIZE;
use crate::ckm::policy::Op;
use crate::ckm::state::COMPONENT_RECORD_SIZE;
use crate::driver::serial;
use crate::fs::vfs::{EINVAL, ENOSPC};
use crate::syscall::address_space::UserSpace;

/// `ckm_policy_query(op, arg, out_ptr, out_len, 0)`（257）。
///
/// 返回**该 op 在当前进程凭证下的判定**：`0` = 允许，`-EPERM` = 拒绝；
/// 同时把一条 48 字节记录写进 `out_ptr`（**复用审计记录布局**，`seq = 0`
/// 表示「这不是事件」，见 [`api::policy_query_record`]）。
///
/// `arg` 是 op 相关的细节（例如 `fs_write` 传受保护前缀下标），它会进证据行的
/// `detail`；第一版**没有**参数能指定「查别的主体」—— 这是刻意的：
/// 查询接口若能被用来枚举别人的权限，本身就是一类信息泄露。
pub unsafe fn sys_ckm_policy_query(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let (op_code, arg, out_ptr, out_len, reserved) = (args[0], args[1], args[2], args[3], args[4]);
    if reserved != 0 {
        return EINVAL;
    }
    if out_len < AUDIT_RECORD_SIZE as u64 {
        return EINVAL;
    }
    let Some(op) = Op::from_code(op_code as u16) else {
        serial::print("user: ckm_policy_query op=");
        serial::print_dec(op_code);
        serial::println(" -> EINVAL (unknown op)");
        return EINVAL;
    };
    let out = match unsafe {
        crate::syscall::file::user_bytes_mut(space, out_ptr, out_len, "ckm_policy_query")
    } {
        Ok(out) => out,
        Err(errno) => return errno,
    };

    let q = api::policy_query(op, arg);
    api::policy_query_record(&mut out[..AUDIT_RECORD_SIZE], op, arg, &q);

    serial::print("user: ckm_policy_query op=");
    serial::print_dec(op.code() as u64);
    serial::print(" result=");
    serial::print(if q.errno == 0 { "allow" } else { "deny" });
    serial::print(" -> ");
    serial::print_dec(q.errno.unsigned_abs());
    serial::println("");
    q.errno
}

/// `ckm_audit_read(buf_ptr, buf_len, since_seq, out_count_ptr, 0)`（258）。
///
/// 需要 [`CAP_AUDIT_READ`]；把 `seq > since_seq` 的 48 字节记录按最旧→最新拷进
/// 用户缓冲，返回**实际写入条数**；`out_count_ptr != 0` 时把**总条数**写成一个
/// LE `u64`（便于调用方先问大小）。缓冲装不下时**不截断单条记录**，只少写几条
/// （与 `storage_volumes` 同一约定）。
///
/// [`CAP_AUDIT_READ`]: crate::security::capability::CAP_AUDIT_READ
pub unsafe fn sys_ckm_audit_read(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let (buf_ptr, buf_len, since_seq, count_ptr, reserved) = (args[0], args[1], args[2], args[3], args[4]);
    if reserved != 0 {
        return EINVAL;
    }
    if buf_len < AUDIT_RECORD_SIZE as u64 {
        return EINVAL;
    }
    // 判定点 5：读审计缓冲需要 CAP_AUDIT_READ（拒绝会产生审计 + 串口证据）
    let errno = api::enforce(Op::AuditRead, 0);
    if errno != 0 {
        return errno;
    }

    let out = match unsafe {
        crate::syscall::file::user_bytes_mut(space, buf_ptr, buf_len, "ckm_audit_read")
    } {
        Ok(out) => out,
        Err(errno) => return errno,
    };
    let written = api::audit_read(since_seq, out);

    if count_ptr != 0 {
        let out_count = match unsafe {
            crate::syscall::file::user_bytes_mut(space, count_ptr, 8, "ckm_audit_read")
        } {
            Ok(out) => out,
            Err(errno) => return errno,
        };
        out_count.copy_from_slice(&(api::audit_len() as u64).to_le_bytes());
    }

    serial::print("user: ckm_audit_read count=");
    serial::print_dec(written as u64);
    serial::print(" last=");
    serial::print_dec(api::audit_last_seq());
    serial::println("");
    written as i64
}

/// `ckm_component_state(buf_ptr, buf_len, out_count_ptr, 0, 0)`（259）。
///
/// 只读、无能力要求（读扩展状态不是敏感操作，敏感的是**写**它：判定点 3）。
/// 按**确定性顺序**（`kind` 升序 → `priority` 降序 → `id` 字节序升序）导出 64 字节
/// 记录；缓冲装不下整张表返回 `-ENOSPC`（**要么写完整记录，要么明确失败**，
/// 不做半条记录的截断）。返回写入条数。
pub unsafe fn sys_ckm_component_state(args: &[u64; 6], space: *const UserSpace) -> i64 {
    let (buf_ptr, buf_len, count_ptr, a, b) = (args[0], args[1], args[2], args[3], args[4]);
    if a != 0 || b != 0 {
        return EINVAL;
    }
    let count = api::component_count();
    if buf_len < (count * COMPONENT_RECORD_SIZE) as u64 {
        serial::print("user: ckm_component_state needs ");
        serial::print_dec((count * COMPONENT_RECORD_SIZE) as u64);
        serial::println(" bytes -> ENOSPC");
        return ENOSPC;
    }
    if count == 0 {
        if count_ptr != 0 {
            let out_count = match unsafe {
                crate::syscall::file::user_bytes_mut(space, count_ptr, 8, "ckm_component_state")
            } {
                Ok(out) => out,
                Err(errno) => return errno,
            };
            out_count.copy_from_slice(&0u64.to_le_bytes());
        }
        return 0;
    }

    let out = match unsafe {
        crate::syscall::file::user_bytes_mut(space, buf_ptr, buf_len, "ckm_component_state")
    } {
        Ok(out) => out,
        Err(errno) => return errno,
    };
    let written = api::component_state(out);

    if count_ptr != 0 {
        let out_count = match unsafe {
            crate::syscall::file::user_bytes_mut(space, count_ptr, 8, "ckm_component_state")
        } {
            Ok(out) => out,
            Err(errno) => return errno,
        };
        out_count.copy_from_slice(&(written as u64).to_le_bytes());
    }

    serial::print("user: ckm_component_state count=");
    serial::print_dec(written as u64);
    serial::println("");
    written as i64
}
