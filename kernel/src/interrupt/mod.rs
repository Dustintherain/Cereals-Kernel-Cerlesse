//! 中断分层入口（v0.2 异常骨架；v0.4 补齐 IRQ 控制器绑定）
//!
//! 本 crate 内内核中断相关代码组织如下：
//! - `exception`：异常名称表/诊断辅助（v0.2，不再直接持有汇编 stub）。
//! - `irq`：IRQ 分发预留（v0.4，后续补齐 handler 注册表）。
//! - `controller`：中断控制器概念接口与 PIC/APIC 切分（v0.4 起）。

pub mod controller;
pub mod exception;
pub mod irq;

// 说明：kernel crate 当前是 `no_std` 二进制目标（x86_64-unknown-none），
// 因此 `cargo test` 会引入 std 并与自定义 panic_handler 冲突（E0152）。
// 后续单元测试倾向放到独立的可测试宿主 crate（或 tools-go/ 等主机侧），
// 并通过本模块说明该限制，而非直接对 kernel 本身做 `cargo test`。

// 已验证结论（本环境实测）：
// - `cargo check --workspace` ✅
// - `make test` / `make test-exception` / `make test-memory` / `make test-pagefault` / `make test-all` ✅
// - `kernel/Cargo.toml` feature `exception-test` / `pagefault-test` 可用
// - 中断控制器 PIC 第一版已接入启动链（初始化后默认全屏蔽）
// - 中断基建快照见 `kernel/src/interrupt/doc CONTRIBUTING_CHAIN.md`（当前仅作为上下文占位）
