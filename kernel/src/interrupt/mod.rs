//! 中断分层入口（v0.2 异常骨架；v0.4 补齐 IRQ 控制器绑定）
//!
//! 本 crate 内内核中断相关代码组织如下：
//! - `exception`：异常名称表与诊断辅助（v0.2，不再直接持有汇编 stub）。
//! - `irq`：IRQ 分发入口与最小可验证的 IRQ0（PIT）注册表骨架（v0.4）。
//! - `controller`：中断控制器概念接口与 PIC/APIC 切分（v0.4 起）。

pub mod controller;
pub mod exception;
pub mod irq;

// 说明：kernel crate 当前是 `no_std` 二进制目标（x86_64-unknown-none），
// 因此 `cargo test` 会引入 std 并与自定义 panic_handler 冲突（E0152）。
// 后续单元测试倾向放到独立的可测试宿主 crate（或 tools-go/ 等主机侧），
// 并通过本模块说明该限制，而非直接对 kernel 本身做 `cargo test`。

// 已验证结论（本环境实测，2026-10-08）：
// - `cargo check --workspace` ✅（无错误、无告警）
// - `make test` / `make test-exception` / `make test-memory` / `make test-pagefault` ✅
// - `make test-keyboard` ✅（QEMU monitor 注入真实按键 → IRQ1 回显 + PIT 心跳）
// - `kernel/Cargo.toml` feature `exception-test` / `pagefault-test` 可用
// - PIC 初始化已接入启动链；中断未注册的 IRQ 会被无害化 EOI
// - PIT 通道 0 = 100Hz（10ms/tick）；IRQ0 回调递增 tick 并按节流打印心跳
// - IRQ 向量 32..47 全部走 `irq_common`（保存/恢复通用寄存器后 iretq）
// - 当前状态快照见 `kernel/src/interrupt/known_state.md`
