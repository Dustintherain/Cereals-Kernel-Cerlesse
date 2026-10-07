//! x86_64 架构层

pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod interrupt;
pub mod paging;

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
/// 注意：本函数目前仅作为 v0.4 时间/IRQ 路径的预留入口，
/// 尚未在启动链中实际调用（IRQ0 虽已在 PIC 上使能，但 IF 仍关闭）。
#[allow(dead_code)]
pub unsafe fn enable_irqs() {
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
}
