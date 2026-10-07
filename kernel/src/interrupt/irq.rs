//! IRQ 分发（v0.4 实现）
// TODO(v0.4): 向量号 → handler 注册表

// v0.4 延伸2：准备接收 IRQ0（PIT）中断的最小分发语义。
// 当前阶段不构建完整的 IRQ 表，只在概念上表明：
// - IRQ0 对应向量 `interrupt::controller::IRQ_BASE_VECTOR`
// - 递交时应调用控制器的 EOI（主片 + 必要时从片）
// - 并在时钟子系统侧递增 tick

/// IRQ 分发预留入口（将来由汇编 IRQ stub 跳入）。
///
/// # Safety
///
/// 仅由中断上下文调用。当前还未真正接通；此函数主要用于表达
/// 后续实现意图与载体。
#[no_mangle]
pub unsafe extern "C" fn irq_dispatch(vector: u8) {
    // 预留语义：
    // 1. 根据 vector 决定是 master 还是 slave IRQ
    // 2. 调用控制器的 EOI 语义
    // 3. 若是 IRQ0（PIT），通知时钟子系统递增 tick
    //
    // 当前实现为占位，不实际分发任何已启用的 IRQ。
    let _vector = vector;

    // ---- 占位语义 ----
    // 当前尚未真正构建 IRQ 表；此函数仅表达后续分发意图。

    // 若将来这里真的接收到 PIT IRQ（IRQ0），则应当：
    // 1. 取得控制器实例（通常为全局/单例引用）
    // 2. 对 IRQ0 调用 EOI（主片命令端口写 0x20）
    // 3. 通知时钟子系统递增 tick
    //
    // 为避免依赖串口或其他子系统导致过早耦合，
    // 这里暂不打印/通知，只保留占位。

        let _ = crate::interrupt::controller::irq0_vector();
}

// ---- 演示辅助：标记本函数是否曾被启动链中的占位调用触发过 ----

/// 标记 IRQ 分发预留入口已被启动链中的占位调用触发过。
pub fn mark_irq_dispatch_placeholder_called() {
    unsafe {
        crate::arch::x86_64::IRQ0_DISPATCH_PLACEHOLDER_CALLED = true;
    }
}

/// 查询 IRQ 分发预留入口是否已被启动链中的占位调用触发过。
pub fn irq_dispatch_placeholder_called() -> bool {
    unsafe { crate::arch::x86_64::IRQ0_DISPATCH_PLACEHOLDER_CALLED }
}
