//! IDT 中断描述符表（v0.2 实现）
//!
//! 覆盖 CPU 保留的 0..32 异常向量；处理函数地址来自 `interrupt::ISR_TABLE`。
//! #DF 使用 IST1（gdt.rs 中的双错专用栈），避免栈损坏时 triple fault。

use crate::arch::x86_64::gdt::KERNEL_CODE;
use crate::arch::x86_64::interrupt::handler_table;
use core::arch::asm;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut};

/// CPU 保留异常向量数
pub const NUM_VECTORS: usize = 32;

/// #DF 向量号
const VECTOR_DOUBLE_FAULT: usize = 8;

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
struct Idt([IdtEntry; NUM_VECTORS]);

static mut IDT: Idt = Idt([IdtEntry::empty(); NUM_VECTORS]);

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
        for (vec, entry) in idt.0.iter_mut().enumerate() {
            let ist = if vec == VECTOR_DOUBLE_FAULT { 1 } else { 0 };
            entry.set(handlers[vec] as u64, KERNEL_CODE, ist);
        }

        let ptr = IdtPointer {
            limit: (NUM_VECTORS * size_of::<IdtEntry>() - 1) as u16,
            base: addr_of!(IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(nostack, preserves_flags));
    }
}
