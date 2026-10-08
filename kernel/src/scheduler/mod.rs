//! 调度器（v0.5）
//!
//! - [`scheduler`]：任务表、上下文切换编排、时间片记账入口；
//! - [`queue`]：定长环形就绪队列；
//! - [`round_robin`]：第一版唯一调度策略（Round Robin，200ms 时间片，ADR-005）。
//!
//! 驱动方式：PIT（100Hz）→ IRQ0 → `scheduler::on_tick()` → 时间片用尽则
//! `switch_to` 轮转到下一个就绪任务。切换发生在 IRQ0 中断上下文内，
//! 因此 IRQ0 的 EOI 必须在切换之前发出（见 `interrupt::irq::irq_dispatch`）。

pub mod queue;
pub mod round_robin;
pub mod scheduler;
