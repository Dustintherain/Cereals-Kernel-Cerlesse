//! IRQ 分发（v0.4 实现）
// TODO(v0.4): 向量号 → handler 注册表

// v0.4：IRQ 分发入口（将来由汇编 IRQ stub 跳入）。
//
// 当前阶段实现最小可验证的 IRQ0（PIT）分发语义：
// - IRQ0 对应向量 `interrupt::controller::IRQ_BASE_VECTOR`
// - 调用控制器的 EOI（主片 + 必要时从片）
// - 在时钟子系统侧递增 tick
//
// 当前阶段不再在中断上下文中复用串口打印，避免串口重入导致递归/混乱。
// 心跳观测改由启动线程/主循环侧基于 tick_count 轮询打印。

/// IRQ 分发入口（将来由汇编 IRQ stub 跳入）。
///
/// # Safety
///
/// 仅由中断上下文调用。
#[no_mangle]
pub unsafe extern "C" fn irq_dispatch(vector: u8) {
    let vector = vector;

    // 当前阶段仅处理 IRQ0（PIT）。其他 IRQ 在接通后再在此扩展。
    if vector == crate::interrupt::controller::irq0_vector() {
        // v0.4：IRQ0（PIT）在串口重入风险可控时直接处理完并返回。
        // 此处不打开中断（不 sti），避免在回人后立刻重入 IRQ0。
        // IF 的恢复留给中断返回路径（iretq）或后续显式使能逻辑。
        crate::interrupt::controller::eoi_with_vector_static(vector);
        crate::time::clock::inc_tick();
    } else {
        // 其他 IRQ 尚未接通处理；仅做 EOI 占位，避免未处理向量导致后续中断屏蔽。
        crate::interrupt::controller::eoi_with_vector_static(vector);
    }

    // 当前阶段 IRQ 分发不停止处理器；由中断返回路径（iretq）返回到被中断上下文。
    let _ = vector;
}

/// IRQ 分发预留占位标记查询（旧辅助，不影响主流程）
#[allow(dead_code)]
pub fn placeholder_called() -> bool {
    false
}

// 模块结尾占位，不改变模块语义
