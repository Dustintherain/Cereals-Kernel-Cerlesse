//! 审计环形缓冲（固定容量 64 条，`docs/ckm.md` 4.1/4.4）
//!
//! 为什么「拒绝必须可核查」要落成**定长记录 + 环形缓冲**，而不是一句日志：
//!
//! - 日志是给人看的，断言只能在串口文本上做（脆弱）；
//! - 审计是给**程序**读的（`ckm_audit_read` = 258，`ckmd` 与 `ckm` CLI 是消费者）：
//!   48 字节定长记录，偏移是 ABI，libc 提供解析器；
//! - 环形缓冲的意义是**容量有界**：CKM 的第一版没有可写持久化区
//!   （`docs/ckm.md` 第 12 节开放问题 1），因此审计只在内存 —— 满了就覆盖最旧的，
//!   并把 `wrapped` 置位（**不静默丢**，见 [`wrapped`]）。
//!
//! 记录布局（48 字节，权威定义在本文件，`docs/api.md` 与 libc 镜像一次）：
//!
//! | 偏移 | 大小 | 字段 |
//! | ---- | ---- | ---- |
//! | 0 | 8 | `seq`（从 1 起单调递增） |
//! | 8 | 8 | `pid` |
//! | 16 | 4 | `uid` |
//! | 20 | 4 | `gid` |
//! | 24 | 2 | `op`（事件类型，枚举见 [`crate::ckm::policy::Op`]） |
//! | 26 | 1 | `result`（0 = allow，1 = deny） |
//! | 27 | 1 | `reserved` = 0 |
//! | 28 | 4 | `need`（deny 时需要的 capability 位，否则 0） |
//! | 32 | 8 | `detail`（op 相关：被拒路径哈希 / 组件槽位 / errno） |
//! | 40 | 8 | `reserved` = 0 |

use crate::ckm::policy::{Op, RESULT_ALLOW, RESULT_DENY};
use crate::driver::serial;

/// 审计记录长度（字节）—— **再导出**，定义在 `abi::ckm`（阶段三的单一事实源；
/// 本文件上方的布局表就是它对应的字段说明）。
pub use abi::ckm::AUDIT_RECORD_SIZE;

/// 一条审计记录（内核侧结构）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Record {
    /// 单调递增序号（从 1 起）
    pub seq: u64,
    /// 发起进程
    pub pid: u64,
    /// 凭证 uid
    pub uid: u32,
    /// 凭证 gid
    pub gid: u32,
    /// 事件类型
    pub op: u16,
    /// 结果（0 = allow，1 = deny）
    pub result: u8,
    /// deny 时需要的 capability 位
    pub need: u32,
    /// op 相关细节
    pub detail: u64,
}

impl Record {
    /// 序列化进 48 字节缓冲（小端；与 libc 解析器逐字段对应）。
    pub fn write(&self, out: &mut [u8]) {
        out[0..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.pid.to_le_bytes());
        out[16..20].copy_from_slice(&self.uid.to_le_bytes());
        out[20..24].copy_from_slice(&self.gid.to_le_bytes());
        out[24..26].copy_from_slice(&self.op.to_le_bytes());
        out[26] = self.result;
        out[27] = 0;
        out[28..32].copy_from_slice(&self.need.to_le_bytes());
        out[32..40].copy_from_slice(&self.detail.to_le_bytes());
        out[40..48].copy_from_slice(&[0u8; 8]);
    }
}

/// 环形缓冲本体（固定容量，静态分配 —— 不依赖内核堆，启动早期就能用）。
struct Ring {
    entries: [Record; crate::ckm::AUDIT_CAPACITY],
    /// 下一个写入位置
    head: usize,
    /// 已写入的总条数（饱和到容量）
    len: usize,
    /// 累计写入条数（**单调**，用来判断是否回绕过）
    total: u64,
    /// 序号分配器
    next_seq: u64,
}

static mut RING: Option<Ring> = None;

/// 取环形缓冲（首次访问时建立；[`reset`] 之后重新建立）。
fn ring() -> &'static mut Ring {
    unsafe {
        let slot = (&raw mut RING).as_mut().expect("RING 是静态量");
        slot.get_or_insert_with(|| Ring {
            entries: [Record {
                seq: 0,
                pid: 0,
                uid: 0,
                gid: 0,
                op: 0,
                result: RESULT_ALLOW,
                need: 0,
                detail: 0,
            }; crate::ckm::AUDIT_CAPACITY],
            head: 0,
            len: 0,
            total: 0,
            next_seq: 1,
        })
    }
}

/// 清空审计环（[`crate::ckm::init`] 调用；序号重新从 1 开始）。
pub fn reset() {
    unsafe {
        *(&raw mut RING) = None;
    }
    ring();
}

/// 已记录条数（≤ [`crate::ckm::AUDIT_CAPACITY`]）。
pub fn len() -> usize {
    ring().len
}

/// 是否已经回绕过：写入条数超过容量即真。
///
/// 规格（`docs/ckm.md` 4.4）要求「环形缓冲满后回绕并置 `wrapped` 标志」。
/// 48 字节记录布局里**没有** `wrapped` 字段，因此第一版这样暴露它：
/// **每次回绕在串口打一行** `ckm: audit ring wrapped capacity=64`，
/// 并且内核侧保留 [`wrapped`] 供以后的 `/ckm/audit` 状态树（CKM-4）与
/// 审计持久化（CKM-6）读取。这是对规格空白的**最小补齐**，已写进
/// `docs/ckm.md` 第 13 节修订记录 —— 不假装记录里有这个字段。
pub fn wrapped() -> bool {
    ring().total > crate::ckm::AUDIT_CAPACITY as u64
}

/// 已分配的最大序号（`0` = 还没有任何事件）。
pub fn last_seq() -> u64 {
    ring().next_seq.saturating_sub(1)
}

/// 记录一条审计事件，并打印串口证据行。
///
/// 三条证据的形状（`docs/ckm.md` 4.5）：判定行由 [`crate::ckm::policy::check`] 打，
/// 审计行在这里打，两者一一对应 —— 因此「内核真的判断过」与「拒绝被记下来了」
/// 是两个可以分别断言的串口事实。
pub fn record(pid: u64, uid: u32, gid: u32, op: Op, result: u8, need: u32, detail: u64) {
    let rec = push(pid, uid, gid, op, result, need, detail);
    print_record(&rec);
}

/// 只写进环形缓冲、**不打串口**（自测里批量造事件时用，避免 65 行噪声）。
///
/// 为什么允许「不打日志」：串口证据的意义是「真实操作被判定过」；
/// 内核自测为验证**回绕**而批量注入的事件不是任何真实操作，
/// 给它们各打一行会把 64 条环容量变成日志噪声（见 [`crate::ckm::api::selftest`]）。
pub fn record_silent(pid: u64, uid: u32, gid: u32, op: Op, result: u8, need: u32, detail: u64) {
    push(pid, uid, gid, op, result, need, detail);
}

/// 内部：分配序号 → 写入槽位 → 处理回绕。
fn push(pid: u64, uid: u32, gid: u32, op: Op, result: u8, need: u32, detail: u64) -> Record {
    let r = ring();
    if r.len == crate::ckm::AUDIT_CAPACITY {
        // 已满：覆盖最旧的，并（每次回绕）留一行证据
        let wrapped_now = r.total + 1 > crate::ckm::AUDIT_CAPACITY as u64;
        if wrapped_now && r.total == crate::ckm::AUDIT_CAPACITY as u64 {
            serial::print("ckm: audit ring wrapped capacity=");
            serial::print_dec(crate::ckm::AUDIT_CAPACITY as u64);
            serial::println("");
        }
    }
    let rec = Record {
        seq: r.next_seq,
        pid,
        uid,
        gid,
        op: op.code(),
        result,
        need,
        detail,
    };
    r.entries[r.head] = rec;
    r.head = (r.head + 1) % crate::ckm::AUDIT_CAPACITY;
    if r.len < crate::ckm::AUDIT_CAPACITY {
        r.len += 1;
    }
    r.total += 1;
    r.next_seq += 1;
    rec
}

/// `ckm: audit seq=1 op=fs_write result=deny pid=3 uid=2000`。
fn print_record(rec: &Record) {
    serial::print("ckm: audit seq=");
    serial::print_dec(rec.seq);
    serial::print(" op=");
    serial::print(Op::from_code(rec.op).map_or("unknown", |op| op.name()));
    serial::print(" result=");
    serial::print(if rec.result == RESULT_DENY { "deny" } else { "allow" });
    serial::print(" pid=");
    serial::print_dec(rec.pid);
    serial::print(" uid=");
    serial::print_dec(rec.uid as u64);
    serial::println("");
}

/// 把 `seq > since_seq` 的记录按**最旧→最新**拷进 `out`，返回写入条数。
///
/// `out.len()` 必须至少是 [`AUDIT_RECORD_SIZE`]（调用方已校验）；
/// 装不下的部分**不截断单条记录**，只少写几条。
pub fn read(since_seq: u64, out: &mut [u8]) -> usize {
    let r = ring();
    let capacity = out.len() / AUDIT_RECORD_SIZE;
    let mut written = 0;
    for i in 0..r.len {
        if written == capacity {
            break;
        }
        let idx = (r.head + crate::ckm::AUDIT_CAPACITY - r.len + i) % crate::ckm::AUDIT_CAPACITY;
        let rec = r.entries[idx];
        if rec.seq <= since_seq {
            continue;
        }
        let start = written * AUDIT_RECORD_SIZE;
        rec.write(&mut out[start..start + AUDIT_RECORD_SIZE]);
        written += 1;
    }
    written
}
