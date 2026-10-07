//! x86_64 架构层

pub mod gdt;
pub mod idt;
pub mod interrupt;

// TODO(v0.3): pub mod paging;
// TODO(v0.5): pub mod context;
// boot.rs / cpu.rs 仍为占位（入口逻辑在 crate 根 main.rs）

/// 架构初始化：先 GDT（含 TSS），后 IDT。调用前提：已在 `_start` 中 cli、串口已初始化。
pub fn init() {
    gdt::init();
    idt::init();
}
