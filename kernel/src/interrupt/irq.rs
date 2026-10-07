//! IRQ 分发（v0.4 实现）
// TODO(v0.4): 向量号 → handler 注册表

// v0.4：IRQ 分发预留入口（将来由汇编 IRQ stub 跳入）。
//
// 当前阶段实现最小可验证的 IRQ0（PIT）分发语义：
// - IRQ0 对应向量 `interrupt::controller::IRQ_BASE_VECTOR`
// - 调用控制器的 EOI（主片 + 必要时从片）
// - 在时钟子系统侧递增 tick 并打印心跳日志

/// IRQ 分发入口（将来由汇编 IRQ stub 跳入）。
///
/// # Safety
///
/// 仅由中断上下文调用。
#[no_mangle]
pub unsafe extern "C" fn irq_dispatch(vector: u8) {
    let vector = vector;

    // 当前阶段仅处理 IRQ0（PIT）。其他 IRQ 在接通后再在此扩展。
    //
    // 注意：在串口由同一 UART 提供且未屏蔽串口中断时，`serial::print*` 可能在
    // 中断上下文中重入。当前阶段优先保证不崩溃：仅通过安全端口语义做 EOI，
    // tick 递增使用原子感知静态变量，并尽量避免在时钟向量打印期间复用串口。
    if vector == crate::interrupt::controller::irq0_vector() {
        // EOI：IRQ0 是主片 IRQ0，属于 slave 范围之外，因此仅需主片 EOI。
        // `TimeCtrl::handle_irq0` 语义与此一致，等后续再合并为控制器单例。
        unsafe {
            crate::interrupt::controller::eoi_with_vector_static(vector);
        }

        // 通知时钟子系统递增 tick。
        unsafe {
            crate::time::clock::inc_tick();
        }

        // heartbeat 打印：当前阶段直接在 IRQ0 中打印，但前提是串口未被其他中断打断。
        // 若以后引入串口 IRQ 或更多外设中断，此处应改为线程/定时器任务汇聚打印。
        // 此处使用最小串口打印，避免与其他串口输出交错。
        unsafe {
            crate::driver::serial::print("T");
            crate::driver::serial::print_hex(crate::time::clock::tick_count());
            crate::driver::serial::print("\r\n");
        }
    } else {
        // 其他 IRQ 尚未接通处理；仅做 EOI 占位，避免未处理向量导致后续中断屏蔽。
        unsafe {
            crate::interrupt::controller::eoi_with_vector_static(vector);
        }
    }

    let _ = vector;
}

/// IRQ 分发预留占位标记查询（旧辅助，不影响主流程）
#[allow(dead_code)]
pub fn placeholder_called() -> bool {
    false
}

// 模块结尾占位，不改变模块语义
