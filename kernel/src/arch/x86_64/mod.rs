//! x86_64 架构层

pub mod context;
pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod interrupt;
pub mod paging;

use crate::interrupt::controller;

/// 架构初始化：先 GDT（含 TSS），后 IDT。调用前提：已在 `_start` 中 cli、串口已初始化。
pub fn init() {
    gdt::init();
    idt::init();
}

/// 打开处理器中断标志（IF），允许可屏蔽中断递交。
///
/// 调用前提：
/// - GDT/IDT 已初始化；
/// - 中断控制器已初始化，且只启用了计划内的 IRQ；
/// - 对应 IRQ 的 handler 已注册（IRQ 分发依赖注册表）。
pub unsafe fn enable_irqs() {
    unsafe {
        core::arch::asm!("sti", options(nostack, preserves_flags));
    }
}

/// 关闭处理器中断标志（IF），用于简短的临界区。
///
/// 调用前提：仅用于不希望被可屏蔽中断打扰的短小代码段。
pub unsafe fn disable_irqs() {
    unsafe {
        core::arch::asm!("cli", options(nostack, preserves_flags));
    }
}

/// 返回当前处理器中断标志（IF）状态。
///
/// 仅用于开发期观测（启动链打印 `IF=1` 说明 sti 生效）。
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

/// 返回向量号是否属于 slave IRQ 范围（IRQ8..IRQ15）。
///
/// 当前 PIC 映射下，slave IRQ 对应向量 `IRQ_BASE_VECTOR+8 .. IRQ_BASE_VECTOR+15`。
#[allow(dead_code)]
pub fn is_pic_slave_irq(vector: u8) -> bool {
    vector >= controller::IRQ_BASE_VECTOR + 8
        && vector <= controller::IRQ_BASE_VECTOR + 15
}
