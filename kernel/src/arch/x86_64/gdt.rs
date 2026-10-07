//! GDT 全局描述符表 + TSS（v0.2 实现）
//!
//! 布局：0x00 null | 0x08 内核代码 | 0x10 内核数据 | 0x18 TSS（16 字节项，占 3-4）。
//! TSS 提供 IST1 专用双错栈与 RSP0（v0.4/v0.6 用户态陷入时使用）。

use core::arch::asm;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut};

/// 内核代码段选择子（index 1）
pub const KERNEL_CODE: u16 = 0x08;
/// 内核数据段选择子（index 2）
pub const KERNEL_DATA: u16 = 0x10;
/// TSS 选择子（index 3）
pub const TSS_SEL: u16 = 0x18;

// 内核栈顶（crate 根 `main.rs` 的 `_start` 汇编中定义，`.global` 导出）
extern "C" {
    #[link_name = "stack_top"]
    static STACK_TOP: u8;
}

/// 64 位任务状态段
#[repr(C, packed)]
struct TaskStateSegment {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

/// 双错专用栈（IST1）：栈损坏时 #DF 仍能进入 handler
const DF_STACK_SIZE: usize = 16 * 1024;
#[repr(align(16))]
struct Stack([u8; DF_STACK_SIZE]);
static mut DOUBLE_FAULT_STACK: Stack = Stack([0; DF_STACK_SIZE]);

static mut TSS: TaskStateSegment = TaskStateSegment {
    reserved0: 0,
    rsp: [0; 3],
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<TaskStateSegment>() as u16, // 无 I/O 位图
};

#[repr(C, align(16))]
struct Gdt {
    null: u64,
    code: u64,
    data: u64,
    tss_low: u64,
    tss_high: u64,
}

static mut GDT: Gdt = Gdt {
    null: 0,
    code: 0,
    data: 0,
    tss_low: 0,
    tss_high: 0,
};

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// 构造 64 位可用 TSS 描述符（16 字节 = 低 8 + 高 8）
fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let low = (limit as u64 & 0xFFFF)
        | ((base & 0xFFFF) << 16)
        | (((base >> 16) & 0xFF) << 32)
        | (0x89 << 40) // type=0x9（可用 64 位 TSS）, P=1, DPL=0
        | (((limit as u64 >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = (base >> 32) & 0xFFFF_FFFF;
    (low, high)
}

/// 加载自有 GDT，重载 CS/DS/SS/ES，加载 TSS。
pub fn init() {
    unsafe {
        // 1. 填充 TSS：IST1 = 双错栈顶；RSP0 = 内核栈顶
        let tss = &mut *addr_of_mut!(TSS);
        tss.ist[0] = core::ptr::addr_of!(DOUBLE_FAULT_STACK.0) as u64 + DF_STACK_SIZE as u64;
        tss.rsp[0] = addr_of!(STACK_TOP) as u64;

        // 2. 填充 GDT（含 TSS 描述符）
        let (tss_low, tss_high) = tss_descriptor(
            addr_of!(TSS) as u64,
            (size_of::<TaskStateSegment>() - 1) as u32,
        );
        let gdt = &mut *addr_of_mut!(GDT);
        gdt.code = 0x00AF_9A00_0000_FFFF; // 64 位代码段：P, DPL0, 可执行/可读, L=1
        gdt.data = 0x00CF_9200_0000_FFFF; // 数据段：P, DPL0, 可写, G=1
        gdt.tss_low = tss_low;
        gdt.tss_high = tss_high;

        // 3. lgdt
        let ptr = GdtPointer {
            limit: (size_of::<Gdt>() - 1) as u16,
            base: addr_of!(GDT) as u64,
        };
        asm!("lgdt [{}]", in(reg) &ptr, options(nostack, preserves_flags));

        // 4. 远返回重载 CS = 0x08
        asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );

        // 5. 重载数据段
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            in(reg) KERNEL_DATA,
            options(nostack, preserves_flags),
        );

        // 6. ltr 加载 TSS（CPU 会置位忙标志）
        asm!("ltr {0:x}", in(reg) TSS_SEL, options(nostack, preserves_flags));
    }
}
