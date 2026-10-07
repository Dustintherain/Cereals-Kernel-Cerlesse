//! 内核时间追踪（v0.4 起，简单 monotonic tick 语义）
//!
//! 当前阶段不依赖高精度时钟源，只提供基于 PIT tick 的最小计数。
//!
//! TODO(v0.4):
//! - 从 IRQ0 handler 更新 `KernelTick`
//! - 提供简单的 tick 读取接口
//! - 后续扩展为 nanosecond-maybe 与休眠/超时支持

#![allow(dead_code)]

/// 内核已观测到的 PIT tick 数（由 IRQ0 handler 更新）。
///
/// 当前阶段为单核 + 中断上下文更新，先用普通静态变量；
/// 若后续引入原子/屏障需求，再迁移到 `core::sync::atomic`。
pub static mut KERNEL_TICK: u64 = 0;

/// 返回当前已记录的内核 tick 数。
pub unsafe fn tick_count() -> u64 {
    unsafe { KERNEL_TICK }
}

/// 增加内核 tick（供 IRQ0 handler 使用）。
pub unsafe fn inc_tick() {
    unsafe {
        KERNEL_TICK = KERNEL_TICK.wrapping_add(1);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

