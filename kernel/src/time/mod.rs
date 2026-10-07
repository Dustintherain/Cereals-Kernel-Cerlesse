//! 时间子系统（v0.4 起，优先 PIT tick 路径）
//!
//! 当前阶段的目标是建立最小可验证的时钟语义：
//! 1. PIT（8253/8254）产生周期性电脑计时中断（IRQ0）。
//! 2. 内核维护一个物理 tick 计数与简单 monotonic 语义。
//! 3. 后续再抽象为更高分辨率时钟源与休眠/超时支持。
//!
//! 约定：
//! - PIT 文件寄存器：0x40
//! - PIT 命令端口：0x43
//! - IRQ0 经 PIC 映射为向量 `interrupt::controller::IRQ_BASE_VECTOR`
//!
//! 当前阶段选用的观测方式：
//! - IRQ0 handler 仅递增 tick，不复用串口打印（避免串口重入）。
//! - 主循环/启动侧基于 tick_count 轮询打印心跳，用于验证 PIT 是否走通。

#![allow(dead_code)]

pub mod clock;
pub mod timer;

use crate::interrupt::controller;

/// 当前时间子系统使用的中断控制器视图（仅用于 IRQ0 处理中的 EOI 与可能的屏蔽操作）。
/// 真实实现中，这一句柄通常由全局控制器引用或单例提供；此处先保留接口意图。
pub struct TimeCtrl<'a> {
    pub controller: &'a mut controller::Controller,
}

impl TimeCtrl<'_> {
    /// 从 IRQ0 向量执行最小 EOI 语义。
    pub unsafe fn handle_irq0(&mut self, vector: u8) {
        // IRQ0 是 master IRQ0，属于 slave 范围之外，因此仅需主片 EOI。
        // 当前阶段通过控制器做带向量的 EOI，后续可直接合并为全局控制器视图。
        self.controller.eoi_with_vector(vector);
    }
}

/// IRQ 分发前瞻入口：将来可在此统一IRQ向量 → 子系统分发映射。
/// 当前阶段仅做占位，不处理实际中断。
pub fn dispatch_irq(_vector: u8) {
    let _ = _vector;
}
