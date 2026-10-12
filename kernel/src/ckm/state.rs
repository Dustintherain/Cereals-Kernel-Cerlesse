//! 组件/扩展状态表（固定 32 槽，`docs/ckm.md` 5.3/5.4）
//!
//! 这张表是内核里唯一认识「扩展」的地方，而且**只认识状态**：没有清单解析、
//! 没有包、没有依赖求解（第 0 节边界 1）。内核也**不持久化**它 —— 重启后由
//! `ckmd` 从回执/状态目录重建并重新登记（阶段 3 起，`docs/ckm.md` 5.4）。
//!
//! 表里有两类列：
//!
//! - **ABI 列**：进 64 字节记录、被 `ckm_component_state`（259）导出
//!   （`id`/`kind`/`state`/`version_code`/`abi_kernel`/`priority`/`rev`）；
//! - **内核侧列**：`path`/`uid`/`gid`/`caps` —— 它们是「组件程序」这个身份
//!   （`docs/ckm.md` 3.1 的凭证改变路径）的输入，**不进记录**（记录布局里没有 uid 字段）。
//!
//! 枚举顺序**必须确定性**（`kind` 升序 → `priority` 降序 → `id` 字节序升序）：
//! 与 `fs: ls` 条目顺序、`getprocs` 槽位顺序同一约定 —— 相同输入两次查询得到
//! 逐字节相同的输出，这是可以脚本断言的前提（`docs/ckm.md` 10 补充条款）。
//!
//! 状态转换按 5.3 的**合法转换表**校验，其它一律 `EINVAL` 并留证据：
//! 状态机的意义是「谁在什么时候把哪个组件从什么状态推到了什么状态」，
//! 允许任意跳动等于没有状态机。

use crate::driver::serial;
use crate::fs::vfs::{EEXIST, EINVAL, ENOSPC};

/// 组件状态记录长度（字节）—— **再导出**，定义在 `abi::ckm`。
pub use abi::ckm::COMPONENT_RECORD_SIZE;
/// `id` 字段宽度（NUL 填充；`[A-Za-z0-9._-]{1,15}`）—— 由 ABI 的字符上限推导。
pub const COMPONENT_ID_LEN: usize = abi::ckm::COMPONENT_ID_MAX + 1;
/// 内核引导组件的版本号（没有清单，给一个确定的整数而不是 0）。
pub const BOOTSTRAP_VERSION_CODE: u32 = 1;

/// 扩展类型（`docs/ckm.md` 5.1 的 `kind`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// 1 用户态扩展（Shell、命令、应用、`libc`）
    Userland = 1,
    /// 2 系统组件扩展（服务、系统工具、配置组件）
    SystemComponent = 2,
    /// 3 系统修改模块（覆盖层，阶段 5）
    SystemOverride = 3,
    /// 4 内核模块（阶段 7 之前一律拒绝安装）
    KernelModule = 4,
}

impl Kind {
    /// 编号（进记录）。
    pub const fn code(self) -> u16 {
        self as u16
    }

}

/// 生命周期状态（`docs/ckm.md` 5.3 的状态机）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// 0 不存在
    Absent = 0,
    /// 1 已暂存（校验通过、事务未提交）
    Staged = 1,
    /// 2 已安装（事务提交成功）
    Installed = 2,
    /// 3 已激活
    Active = 3,
    /// 4 已停用
    Disabled = 4,
    /// 5 失败（阶段 3 起由 ckmd / 内核的失败路径写入；CKM-1 还没有写入者，
    /// 但它是 5.3 状态机的合法状态，先占号 —— 号一旦发布不再回收）
    #[allow(dead_code)]
    Failed = 5,
    /// 6 待移除（下次启动清理）
    RemovedPending = 6,
}

impl State {
    /// 编号（进记录）。
    pub const fn code(self) -> u8 {
        self as u8
    }

}

/// 合法转换（`docs/ckm.md` 5.3 的表；其余一律 `EINVAL`）。
///
/// 与规格表逐条对应：`absent→staged`、`staged→installed|failed`、
/// `installed→active|failed`、`active→disabled|failed|removed-pending`、
/// `disabled→active|failed|removed-pending`、`failed→removed-pending`、
/// `removed-pending→absent`，以及 `any→removed-pending`（标记删除）。
pub fn transition_allowed(from: State, to: State) -> bool {
    if to == State::RemovedPending {
        return true; // 任何状态都可标记删除（文件清理在下次启动）
    }
    matches!(
        (from, to),
        (State::Absent, State::Staged)
            | (State::Staged, State::Installed)
            | (State::Staged, State::Failed)
            | (State::Installed, State::Active)
            | (State::Installed, State::Failed)
            | (State::Active, State::Disabled)
            | (State::Active, State::Failed)
            | (State::Disabled, State::Active)
            | (State::Disabled, State::Failed)
            | (State::RemovedPending, State::Absent)
    )
}

/// 一个槽位：ABI 列 + 内核侧列。
#[derive(Clone, Copy)]
pub struct Slot {
    /// 扩展标识（16 字节，NUL 填充）
    pub id: [u8; COMPONENT_ID_LEN],
    /// 类型
    pub kind: Kind,
    /// 当前状态
    pub state: State,
    /// 整数版本（比较规则同 H-06）
    pub version_code: u32,
    /// 声明兼容的 CKM ABI 版本
    pub abi_kernel: u32,
    /// 覆盖层优先级（非覆盖层恒 0）
    pub priority: u32,
    /// 内容修订（payload SHA-256 前 24 字节；内核引导组件无 payload → 全 0）
    pub rev: [u8; 24],
    /// **内核侧**：组件程序在 VFS 上的路径（`execve` 按它换凭证）
    pub path: &'static str,
    /// **内核侧**：组件程序的运行 uid
    pub uid: u32,
    /// **内核侧**：组件程序的运行主组
    pub gid: u32,
    /// **内核侧**：组件程序的能力位
    pub caps: u32,
}

impl Slot {
    /// `id` 的文本视图（NUL 之前的字节）。
    pub fn id_str(&self) -> &str {
        let end = self
            .id
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(COMPONENT_ID_LEN);
        core::str::from_utf8(&self.id[..end]).unwrap_or("?")
    }

    /// 序列化进 64 字节记录（小端；与 libc 解析器逐字段对应）。
    pub fn write_record(&self, out: &mut [u8]) {
        out[0..COMPONENT_ID_LEN].copy_from_slice(&self.id);
        out[16..18].copy_from_slice(&self.kind.code().to_le_bytes());
        out[18] = self.state.code();
        out[19] = 0; // reserved
        out[20..24].copy_from_slice(&self.version_code.to_le_bytes());
        out[24..28].copy_from_slice(&self.abi_kernel.to_le_bytes());
        out[28..32].copy_from_slice(&self.priority.to_le_bytes());
        out[32..56].copy_from_slice(&self.rev);
        out[56..64].copy_from_slice(&[0u8; 8]); // reserved
    }
}

/// 固定容量表（`None` = 空闲槽位）。
static mut SLOTS: [Option<Slot>; crate::ckm::STATE_SLOTS] = [const { None }; crate::ckm::STATE_SLOTS];

/// 槽位基址（`*const Option<Slot>`）。
#[inline]
fn base() -> *const Option<Slot> {
    (&raw const SLOTS).cast::<Option<Slot>>()
}

/// 读槽位（越界或空闲 → `None`）。
pub fn read_slot(index: usize) -> Option<Slot> {
    if index >= crate::ckm::STATE_SLOTS {
        return None;
    }
    unsafe { (*base().add(index)).clone() }
}

/// 写槽位。
fn write_slot(index: usize, value: Option<Slot>) {
    unsafe {
        (&raw mut SLOTS)
            .cast::<Option<Slot>>()
            .add(index)
            .write(value);
    }
}

/// 清空状态表（[`crate::ckm::init`] 调用；幂等）。
pub fn reset() {
    for i in 0..crate::ckm::STATE_SLOTS {
        write_slot(i, None);
    }
}

/// 已登记槽位数。
pub fn count() -> usize {
    let mut n = 0;
    for i in 0..crate::ckm::STATE_SLOTS {
        if read_slot(i).is_some() {
            n += 1;
        }
    }
    n
}

/// 排序键比较：`kind` 升序 → `priority` 降序 → `id` 字节序升序（确定性）。
fn less(a: &Slot, b: &Slot) -> bool {
    if a.kind.code() != b.kind.code() {
        return a.kind.code() < b.kind.code();
    }
    if a.priority != b.priority {
        return a.priority > b.priority;
    }
    a.id.cmp(&b.id) == core::cmp::Ordering::Less
}

/// 把**确定性顺序**的槽位下标写进 `out`（空闲槽位补 `u8::MAX`），返回槽位数。
///
/// 插入排序、无堆分配：表最多 32 项，而且这是冷路径（快照查询）。
pub fn ordered_indices(out: &mut [u8; crate::ckm::STATE_SLOTS]) -> usize {
    let mut n = 0usize;
    for i in 0..crate::ckm::STATE_SLOTS {
        let Some(slot) = read_slot(i) else {
            continue;
        };
        let mut pos = n;
        while pos > 0 {
            let prev = read_slot(out[pos - 1] as usize).expect("顺序表里的下标必然有效");
            if !less(&slot, &prev) {
                break;
            }
            out[pos] = out[pos - 1];
            pos -= 1;
        }
        out[pos] = i as u8;
        n += 1;
    }
    for i in n..crate::ckm::STATE_SLOTS {
        out[i] = u8::MAX;
    }
    n
}

/// 按确定性顺序取第 `index` 个槽位。
pub fn ordered(index: usize) -> Option<Slot> {
    let mut idx = [u8::MAX; crate::ckm::STATE_SLOTS];
    let n = ordered_indices(&mut idx);
    if index >= n {
        return None;
    }
    read_slot(idx[index] as usize)
}

/// 按路径查组件（`execve` 的凭证改变路径；`docs/ckm.md` 3.1）。
///
/// 返回**槽位下标**（证据行里的 `detail`）与槽位；`kind=4` 不进表，因此这里
/// 不会返回它。
pub fn component_by_path(path: &str) -> Option<(usize, Slot)> {
    for i in 0..crate::ckm::STATE_SLOTS {
        if let Some(slot) = read_slot(i) {
            if slot.path == path {
                return Some((i, slot));
            }
        }
    }
    None
}

/// 内部：只做校验与写入，不打证据（[`register`] 的引导路径要一次打印）。
fn apply(slot: usize, state: State) {
    let mut s = read_slot(slot).expect("槽位已登记");
    s.state = state;
    write_slot(slot, Some(s));
}

/// 登记一个组件：校验 → 占槽位 → 走状态机 `absent→staged→installed→active` → 打证据。
///
/// 校验项（`docs/ckm.md` 5.2 的「必须拒绝」在记录层面的那部分；签名/哈希/路径白名单
/// 属于 CKM-3 的安装校验，不是本函数的事）：
///
/// - `id` 必须是 `[A-Za-z0-9._-]{1,15}`（否则 `EINVAL`）；
/// - `kind=4`（内核模块）在阶段 7 之前一律拒绝 `EINVAL`；
/// - `kind=3` 必须带 `priority ≥ 1`，其余必须 `priority == 0`；
/// - 同一 `path` 不允许重复登记（`EEXIST` —— 重复 target 是 H-06 也拒绝的形态）；
/// - 表满返回 `ENOSPC`（固定容量 + 明确拒绝）。
pub fn register(
    id: &str,
    kind: Kind,
    priority: u32,
    version_code: u32,
    abi_kernel: u32,
    uid: u32,
    gid: u32,
    caps: u32,
    path: &'static str,
) -> Result<usize, i64> {
    if !valid_id(id) {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=id");
        return Err(EINVAL);
    }
    if kind == Kind::KernelModule {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=kind");
        return Err(EINVAL);
    }
    let priority_ok = if kind == Kind::SystemOverride {
        priority >= 1
    } else {
        priority == 0
    };
    if !priority_ok {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=priority");
        return Err(EINVAL);
    }
    if abi_kernel != crate::ckm::CKM_ABI_VERSION {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=abi");
        return Err(EINVAL);
    }
    if let Some((_, _)) = component_by_path(path) {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=target");
        return Err(EEXIST);
    }

    let mut free = None;
    for i in 0..crate::ckm::STATE_SLOTS {
        if read_slot(i).is_none() {
            free = Some(i);
            break;
        }
    }
    let Some(slot) = free else {
        serial::print("ckm: verify id=");
        serial::print(id);
        serial::println(" result=deny reason=full");
        return Err(ENOSPC);
    };

    let mut id_bytes = [0u8; COMPONENT_ID_LEN];
    id_bytes[..id.len()].copy_from_slice(id.as_bytes());
    write_slot(
        slot,
        Some(Slot {
            id: id_bytes,
            kind,
            state: State::Absent,
            version_code,
            abi_kernel,
            priority,
            rev: [0u8; 24],
            path,
            uid,
            gid,
            caps,
        }),
    );
    // 引导路径把状态机完整走一遍（每一步都过 5.3 的合法转换校验），
    // 但只打**一行**最终状态：逐转移明细是 `ckmd` 事务路径（CKM-3/4）的证据。
    apply(slot, State::Staged);
    apply(slot, State::Installed);
    apply(slot, State::Active);
    print_component_line(&read_slot(slot).expect("刚写入"));
    Ok(slot)
}

/// 校验 `id` 的字符集与长度（`[A-Za-z0-9._-]{1,15}`）。
fn valid_id(id: &str) -> bool {
    let n = id.len();
    if n == 0 || n >= COMPONENT_ID_LEN {
        return false;
    }
    id.bytes().all(|b| {
        b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'
    })
}

/// 状态转换（校验 + 证据）：非法转换 `EINVAL`，槽位不存在 `EINVAL`。
///
/// 这是给 `ckmd`（阶段 4）用的路径：每次合法转换打一行组件状态行，
/// 每次被拒也留一行 —— 「谁在什么时候动了什么」因此可断言。
pub fn set_state(slot: usize, to: State) -> Result<(), i64> {
    let Some(cur) = read_slot(slot) else {
        return Err(EINVAL);
    };
    if !transition_allowed(cur.state, to) {
        serial::print("ckm: component id=");
        serial::print(cur.id_str());
        serial::print(" transition=deny from=");
        serial::print_dec(cur.state.code() as u64);
        serial::print(" to=");
        serial::print_dec(to.code() as u64);
        serial::println("");
        return Err(EINVAL);
    }
    apply(slot, to);
    print_component_line(&read_slot(slot).expect("刚写入"));
    Ok(())
}

/// 引导组件登记（`ckm::init` 用）：`priority=0`、`version_code=1`、`abi=当前 ABI`。
pub fn register_bootstrap(
    id: &str,
    kind: Kind,
    uid: u32,
    gid: u32,
    caps: u32,
    path: &'static str,
) {
    let r = register(
        id,
        kind,
        0,
        BOOTSTRAP_VERSION_CODE,
        crate::ckm::CKM_ABI_VERSION,
        uid,
        gid,
        caps,
        path,
    );
    if r.is_err() {
        serial::print("ckm: bootstrap component registration FAILED id=");
        serial::println(id);
    }
}

/// `ckm: component id=hello kind=1 state=3 rev=<16 hex> priority=0`（`docs/ckm.md` 4.5）。
///
/// `rev` 打印前 8 字节的十六进制：引导组件没有 payload 哈希（全 0），
/// 完整 24 字节会写 48 个十六进制字符而没有任何额外信息量。
fn print_component_line(slot: &Slot) {
    serial::print("ckm: component id=");
    serial::print(slot.id_str());
    serial::print(" kind=");
    serial::print_dec(slot.kind.code() as u64);
    serial::print(" state=");
    serial::print_dec(slot.state.code() as u64);
    serial::print(" rev=");
    let mut head = [0u8; 8];
    head.copy_from_slice(&slot.rev[..8]);
    serial::print_hex(u64::from_be_bytes(head));
    serial::print(" priority=");
    serial::print_dec(slot.priority as u64);
    serial::println("");
}
