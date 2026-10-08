//! 时间子系统（v0.4 起，优先 PIT tick 路径）
//!
//! 目标：建立最小可验证的时钟语义。
//! 1. PIT（8253/8254）通道 0 产生周期性 IRQ0 中断（100Hz = 10ms/tick）。
//! 2. 内核维护物理 tick 计数与简单 monotonic 语义（`clock`）。
//! 3. IRQ0 回调负责递增 tick，并按节流打印心跳（见 `interrupt::irq`）。
//! 4. 启动侧与集成测试基于 `tick_count` 观测心跳，验证 PIT 是否走通。
//!
//! 约定：
//! - PIT 通道 0 数据端口：0x40
//! - PIT 命令端口：0x43
//! - IRQ0 经 PIC 映射为向量 `interrupt::controller::IRQ_BASE_VECTOR`


#![allow(dead_code)]

pub mod clock;
pub mod timer;

use crate::interrupt::controller;

/// 时间子系统使用的中断控制器视图（带向量的 EOI）。
///
/// 中断上下文里没有 `&mut Controller` 可用，因此实际 EOI 由
/// `interrupt::controller::eoi_with_vector_static` 承担；
/// 这个视图保留给 v0.5 之后的“全局控制器句柄”使用。
pub struct TimeCtrl<'a> {
    pub controller: &'a mut controller::Controller,
}

impl TimeCtrl<'_> {
    /// 从某一向量执行带向量的 EOI 语义。
    ///
    /// # Safety
    ///
    /// 仅可在中断上下文或关中断的临界区内调用。
    pub unsafe fn handle_irq(&mut self, vector: u8) {
        self.controller.eoi_with_vector(vector);
    }
}
