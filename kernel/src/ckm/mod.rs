//! CKM —— Cerlesse Kernel Manager（内核侧最小接口，阶段 1 / CKM-1）
//!
//! 权威文档：[`docs/ckm.md`](../../../docs/ckm.md)。这一层**不是包管理器**：
//! 它不认识「包」、不解析清单、不做依赖求解（第 0 节边界 1）。它只做三件事：
//!
//! 1. **主体与凭证**：每个进程有一份 [`security::Cred`]，能力位判定（[`cred`]）；
//! 2. **判定点**：把「谁能做什么」变成内核里的**强制点**（[`policy`]）；
//! 3. **可核查**：每次判定与拒绝都留下**审计记录 + 串口证据**（[`audit`]），
//!    组件状态按**确定性顺序**导出（[`state`]）。
//!
//! 模块依赖（必须，`docs/ckm.md` 4.1）：
//!
//! ```text
//! syscall/* ──▶ ckm::api ──▶ {policy, cred, state, audit}
//! fs/        ──▶ ckm::policy（只取前缀常量与判定）
//! fs/        ──▶ ckm::overlay（路径改写钩子；覆盖层的**文件动作**在 fs 侧发起）
//! ckm/       ──▶ security/（凭证与 mode）、driver/{serial, block}（状态区扇区 I/O）
//! ckm/       ──▶ **不**反向依赖 syscall/ 与 fs/
//! ```
//!
//! `ckm/` 是叶子被依赖者：它能被 `syscall/`、`fs/` 依赖，但自己不调用任何 syscall。
//! 这样判定点的位置由调用方决定（`docs/ckm.md` 3.4 的「落点」列），
//! 「内核真的判断过」则统一由 `policy::check` 在串口上留痕。

pub mod api;
pub mod audit;
pub mod cred;
pub mod overlay;
pub mod policy;
pub mod state;
pub mod store;

/// CKM ABI 版本（第一版 = 1）。
///
/// 扩展清单必须声明 `abi.kernel`，阶段 1 的判定规则是**相等**
/// （`ext.abi_kernel == CKM_ABI_VERSION`，`docs/ckm.md` 4.2）。
/// 它与 syscall 号段边界（`SYS_MAX`）**是两个独立的版本号**：前者约束管理接口 / 记录布局，
/// 后者约束「哪个调用号存在」。
pub use abi::ckm::ABI_VERSION as CKM_ABI_VERSION;

/// 管理员 uid（阶段 1 = 内核常量；阶段 4 起由签名配置决定，见开放问题 3）。
pub const CKM_ADMIN_UID: u32 = crate::security::credential::ADMIN_UID;

/// 审计环形缓冲容量（`docs/ckm.md` 4.1：**必须** ≥ 64 条）。
pub const AUDIT_CAPACITY: usize = 64;

/// 组件/扩展状态表容量（固定容量 + 明确拒绝，与 `driver` 注册表、进程表同风格）。
/// **再导出**，定义在 `abi::ckm::MAX_COMPONENTS`（阶段三的单一事实源）。
pub use abi::ckm::MAX_COMPONENTS as STATE_SLOTS;

/// 已初始化标志（[`api::init`] 幂等）。
static mut INITIALIZED: bool = false;

/// 初始化 CKM：建立**空**状态表与审计环，并登记内核内置的引导组件表。
///
/// **幂等**：重复调用不改变任何状态（`docs/ckm.md` 4.1 的 `init()`）。
/// 必须在任何用户进程运行之前调用（`kernel_main` 里紧跟 `fs::init` 之后），
/// 否则第一批 `open`/`execve` 会在凭证还没建立时被判定 —— 那时 `current()` 只能
/// 退回内核凭证，等于「启动早期一段无判定窗口」。
pub fn init() {
    unsafe {
        if INITIALIZED {
            return;
        }
        INITIALIZED = true;
    }
    audit::reset();
    state::reset();
    // 阶段 1 没有 `ckmd`：内核内置**引导组件表**（`docs/ckm.md` 3.1 的主体划分）。
    // 这三条同时是 `docs/ckm.md` 5.4 状态表的初始内容 —— `ckm_component_state`
    // 导出它们，`execve` 按路径查它们换 uid/gid/能力。
    state::register_bootstrap(
        "init",
        state::Kind::SystemComponent,
        0,
        0,
        crate::security::capability::INIT,
        "/init",
    );
    state::register_bootstrap(
        "shell",
        state::Kind::Userland,
        crate::security::credential::ADMIN_UID,
        crate::security::credential::ADMIN_UID,
        crate::security::capability::ADMIN,
        "/bin/sh",
    );
    state::register_bootstrap(
        "hello",
        state::Kind::Userland,
        crate::security::credential::PLAIN_UID,
        crate::security::credential::PLAIN_UID,
        0,
        "/bin/hello",
    );

    crate::driver::serial::print("ckm: init ABI_VERSION=");
    crate::driver::serial::print_dec(CKM_ABI_VERSION as u64);
    crate::driver::serial::print(" admin_uid=");
    crate::driver::serial::print_dec(CKM_ADMIN_UID as u64);
    crate::driver::serial::print(" protected=");
    crate::driver::serial::print_dec(policy::PROTECTED_PREFIXES.len() as u64);
    crate::driver::serial::print(" audit_cap=");
    crate::driver::serial::print_dec(AUDIT_CAPACITY as u64);
    crate::driver::serial::println("");
}
