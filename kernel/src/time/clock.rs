//! 内核时间追踪（v0.4 起，简单 monotonic tick 语义）
//!
//! 当前阶段仅提供基于 PIT tick 的最小计数，由 IRQ0 回调递增（100Hz → 10ms/tick）。
//! 后续可扩展为纳秒粒度时钟源、休眠/超时支持。
//!
//! 注意：当前阶段为单核 + 中断上下文更新，先用普通静态变量；
//! 若后续引入原子/屏障/多核需求，再迁移到 `core::sync::atomic`。

#![allow(dead_code)]

/// 内核已观测到的 PIT tick 数（由 IRQ0 回调更新）。
pub static mut KERNEL_TICK: u64 = 0;

/// 返回当前已记录的内核 tick 数。
pub unsafe fn tick_count() -> u64 {
    unsafe { KERNEL_TICK }
}

/// 增加内核 tick（供 IRQ0 回调使用）。
pub unsafe fn inc_tick() {
    unsafe {
        KERNEL_TICK = KERNEL_TICK.wrapping_add(1);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

