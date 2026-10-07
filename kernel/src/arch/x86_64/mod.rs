//! x86_64 架构层

pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod interrupt;
pub mod paging;

use crate::interrupt::controller;

// TODO(v0.5): pub mod context;
// boot.rs 仍为占位（入口逻辑在 crate 根 main.rs）

/// 架构初始化：先 GDT（含 TSS），后 IDT。调用前提：已在 `_start` 中 cli、串口已初始化。
pub fn init() {
    gdt::init();
    idt::init();
}

/// 启用外设中断使能的第一步：打开处理器中断标志（IF）。
/// 调用前提：
/// - GDT/IDT 已初始化
/// - 中断控制器已初始化且只启用了计划内的 IRQ
/// - 当前上下文允许开启中断（例如不在禁止中断的临界区内）
///
/// 当前实现为简单 `sti`；后续可结合态势控制与屏蔽语义包装。
///
/// 注意：v0.4 中本函数已在启动链中调用（PIT/IRQ0 配置后打开 IF），
/// 从而使 PIT 周期性中断得以递交。
pub unsafe fn enable_irqs() {
    // 打开处理器中断标志（IF），允许可屏蔽中断递交。
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
}

/// 关闭处理器中断标志（IF），用于简短的临界区。
/// 调用前提：适合在不希望受可屏蔽中断打扰的短小代码段中使用。
///
/// 注意：本函数目前仅作为 v0.4 时间/IRQ 路径的预留入口，
/// 尚未在启动链中实际调用。
#[allow(dead_code)]
pub unsafe fn disable_irqs() {
    unsafe {
        core::arch::asm!("cli", options(nostack, preserves_flags));
    }
}

/// 返回当前处理器中断标志（IF）的状态（0 或 1）。
///
/// 注意：这是一个简陋的查询辅助，仅用于观测当前是否允许可屏蔽中断。
pub fn irqs_enabled() -> bool {
    let flags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq; pop {0:r}",
            out(reg) flags,
            options(nomem, nostack, preserves_flags),
        );
    }
    (flags & 0x200) != 0
}

/// 返回当前向量号是否属于 slave IRQ 范围（IRQ8..IRQ15）。
///
/// 当前 PIC 映射下，slave IRQ 对应向量 `IRQ_BASE_VECTOR+8 .. IRQ_BASE_VECTOR+15`。
#[allow(dead_code)]
pub fn is_pic_slave_irq(vector: u8) -> bool {
    vector >= controller::IRQ_BASE_VECTOR + 8
        && vector <= controller::IRQ_BASE_VECTOR + 15
}

// ---- 下面是为后续 IRQ 分发/调试预留的小工具，不改变当前主流程 ----

/// 仅作演示用途的静态标记：指示 IRQ0（PIT）分发预留入口是否已在启动链中被调用过。
///
/// 当前实现为占位，不表示真实中断已被递交。
pub static mut IRQ0_DISPATCH_PLACEHOLDER_CALLED: bool = false;

/// 标记 IRQ0 分发预留入口已被调用（仅用于启动链中的占位演示）。
#[allow(dead_code)]
pub fn mark_irq0_dispatch_placeholder_called() {
    unsafe { IRQ0_DISPATCH_PLACEHOLDER_CALLED = true; }
}

/// 查询 IRQ0 分发预留入口是否已在启动链中被调用过。
#[allow(dead_code)]
pub fn irq0_dispatch_placeholder_called() -> bool {
    unsafe { IRQ0_DISPATCH_PLACEHOLDER_CALLED }
}
