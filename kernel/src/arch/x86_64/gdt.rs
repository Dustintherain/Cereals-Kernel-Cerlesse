//! GDT 全局描述符表 + TSS（v0.2 实现；v0.6 追加：用户コード/データ段）
//!
//! 布局：0x00 null | 0x08 内核コード | 0x10 内核データ | 0x18 TSS（16 字节项，占 3-4）
//!        | 0x28 用户コード | 0x30 用户データ
//! TSS 提供 IST1 专用双错栈与 RSP0（v0.4/v0.6 用户态陷入时使用）。

use core::arch::asm;
use core::mem::size_of;
use core::ptr::{addr_of, addr_of_mut};

/// 内核コード段選択子（index 1）
pub const KERNEL_CODE: u16 = 0x08;
/// 内核データ段選択子（index 2）
pub const KERNEL_DATA: u16 = 0x10;
/// TSS 選択子（index 3）
pub const TSS_SEL: u16 = 0x18;

/// 用户コード段選択子（index 5、RPL=3）
pub const USER_CODE: u16 = 0x28;
/// 用户データ段選択子（index 6、RPL=3）
pub const USER_DATA: u16 = 0x30;

/// 内核栈顶（crate 根 `main.rs` の `_start` 汇编 で定義、`.global` 导出）
extern "C" {
    #[link_name = "stack_top"]
    static STACK_TOP: u8;
}

/// 64 位タスク状態段
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
    user_code: u64,
    user_data: u64,
}

static mut GDT: Gdt = Gdt {
    null: 0,
    code: 0,
    data: 0,
    tss_low: 0,
    tss_high: 0,
    user_code: 0,
    user_data: 0,
};

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// 64 位タスク状態段を構築するヘルパー（TSS 描述子 本体 16 バイト）
fn tss_desc_low(base: u64, limit: u32) -> u64 {
    (limit as u64 & 0xFFFF)
        | ((base & 0xFFFF) << 16)
        | (((base >> 16) & 0xFF) << 32)
        | (0x89 << 40)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56)
}

/// 加载自有 GDT，重载 CS/DS/SS/ES，加载 TSS。
pub fn init() {
    unsafe {
        // 1. 填充 TSS：IST1 = 双错栈顶；RSP0 = 用户态陷入用固定カーネルスタック
        let tss = &mut *addr_of_mut!(TSS);
        tss.ist[0] = core::ptr::addr_of!(DOUBLE_FAULT_STACK.0) as u64 + DF_STACK_SIZE as u64;
        tss.rsp[0] = addr_of!(STACK_TOP) as u64;

        // 2. 填充 GDT（含 TSS 描述符）
        let tss_low = tss_desc_low(addr_of!(TSS) as u64, (size_of::<TaskStateSegment>() - 1) as u32);
        let tss_high = (addr_of!(TSS) as u64 >> 32) & 0xFFFF_FFFF;
        let gdt = &mut *addr_of_mut!(GDT);
        gdt.null = 0;
        gdt.code = 0x00AF_9A00_0000_FFFF; // 64 位コード段：P, DPL0, 可执行/可读, L=1
        gdt.data = 0x00CF_9200_0000_FFFF; // データ段：P, DPL0, 可写, G=1
        gdt.tss_low = tss_low;
        gdt.tss_high = tss_high;
        gdt.user_code = 0x00AF_FA00_0000_FFFF; // 用户コード 64-bit：P, DPL3, L=1
        gdt.user_data = 0x00CF_F200_0000_FFFF; // 用户データ：P, DPL3, G=1

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

        // 5. 重载データ段
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
