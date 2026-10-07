//! IDT 中断描述符表（v0.2 实现；v0.4 扩展 IRQ 向量范围占位）
//!
//! 覆盖 CPU 保留的 0..32 异常向量；处理函数地址来自 `arch::x86_64::interrupt::handler_table`。
//! #DF 使用 IST1（gdt.rs 中的双错专用栈），避免栈损坏时 triple fault。
//!
//! v0.4 目标：在现有 0..32 异常向量之上，预留 IRQ（典型 32..47）入口扩展点。
//! 当前实现不直接安装 IRQ handler，仅提供常量 `IRQ_FIRST` / `IDT_SIZE` 供后续扩展引用。

use crate::arch::x86_64::gdt::KERNEL_CODE;
use crate::arch::x86_64::interrupt::handler_table;
use core::arch::asm;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut};

/// CPU 保留异常向量数（0..31）
pub const NUM_VECTORS: usize = 32;

/// #DF 向量号
const VECTOR_DOUBLE_FAULT: usize = 8;

/// 第一批 IRQ 向量起始值（PIC 传统映射下 IRQ0 常放在向量 32）。
/// 当前阶段对齐 `arch::x86_64::interrupt::IRQ_FIRST`，供上层一致引用。
pub const IRQ_FIRST: usize = NUM_VECTORS;

/// IDT 表当前容量（槽位数）。v0.4 起扩展到 IRQ 范围（32..47）。
pub const IDT_SIZE: usize = NUM_VECTORS + 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    /// bit0-2: IST；bit8-11: 类型(0xE=中断门)；bit15: P
    options: u16,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn empty() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            options: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn set(&mut self, handler: u64, selector: u16, ist: u8) {
        self.offset_low = handler as u16;
        self.selector = selector;
        self.options = (ist as u16 & 0x7) | (0x0E << 8) | (1 << 15);
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.reserved = 0;
    }
}

#[repr(C, align(16))]
struct Idt([IdtEntry; IDT_SIZE]);

static mut IDT: Idt = Idt([IdtEntry::empty(); IDT_SIZE]);

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

/// 构建 IDT 并 lidt。调用前提：gdt::init() 已完成。
pub fn init() {
    unsafe {
        let handlers = handler_table();
        let idt = &mut *addr_of_mut!(IDT);
        for vec in 0..NUM_VECTORS {
            let entry = &mut idt.0[vec];
            let ist = if vec == VECTOR_DOUBLE_FAULT { 1 } else { 0 };
            match handlers.get(vec) {
                Some(&handler) => {
                    entry.set(handler as u64, KERNEL_CODE, ist);
                }
                None => {
                    entry.set(0, KERNEL_CODE, ist);
                }
            }
        }
        // v0.4：IRQ 范围（32..47）也安装对应向量 stub，供后续 IRQ 分发使用。
        for vec in NUM_VECTORS..IDT_SIZE {
            let handler = crate::arch::x86_64::interrupt::handler_table()[vec];
            idt.0[vec].set(handler as u64, KERNEL_CODE, 0);
        }

        let ptr = IdtPointer {
            limit: (IDT_SIZE * size_of::<IdtEntry>() - 1) as u16,
            base: addr_of!(IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(nostack, preserves_flags));
    }
}
