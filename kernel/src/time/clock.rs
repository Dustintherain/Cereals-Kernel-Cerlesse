//! 内核时间追踪（v0.4 起，简单 monotonic tick 语义）
//!
//! 当前阶段不依赖高精度时钟源，只提供基于 PIT tick 的最小计数。
//!
//! TODO(v0.4):
//! - 从 IRQ0 handler 更新 `KernelTick`
//! - 提供简单的 tick 读取接口
//! - 后续扩展为 nanosecond-maybe 与休眠/超时支持

#![allow(dead_code)]

/// 内核已观测到的 PIT tick 数（仅作示例语义，尚未由 IRQ0 handler 实际更新）。
///
/// 真实实现中，这一变量应由中断上下文安全地更新，并考虑读/写可见性。
pub static mut KERNEL_TICK: u64 = 0;

/// 返回当前已记录的内核 tick 数。
///
/// # Safety
///
/// 当前实现依赖非原子的静态可变变量；调用者必须自行确保读取时
/// 不会与中断上下文更新发生竞争。后续应替换为原子或受锁保护的实现。
pub unsafe fn tick_count() -> u64 {
    KERNEL_TICK
}

/// 增加内核 tick（预留给 IRQ0 handler 使用）。
///
/// # Safety
///
/// 只能从适合的中断/时钟上下文调用。当前实现为简单自增，不保证
/// 多核或并发安全。
pub unsafe fn inc_tick() {
    KERNEL_TICK = KERNEL_TICK.wrapping_add(1);
}
