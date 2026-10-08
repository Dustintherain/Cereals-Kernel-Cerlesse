//! 异常 stub 与统一分发（v0.2 实现；v0.4 补充 IRQ stub 预留位）
//!
//! 每个向量一个汇编 stub：有错误码的异常（8/10/11/12/13/14/16）由 CPU 压栈，
//! 其余补 0；随后压向量号、统一保存 15 个通用寄存器，调用 `exception_dispatch`。
//! 栈帧布局与 `ExceptionFrame` 严格一一对应。
//!
//! v0.4 目标：IRQ（32..47）汇编 stub 已实装（IRQ0 单独展开，余者走 isr_common）。
//! IRQ 分发由 `irq_dispatch` 负责，注册表见 `interrupt::irq`。

use crate::arch::x86_64::idt::IDT_SIZE;
use crate::driver::serial;
use core::arch::{asm, global_asm};

/// 异常时的完整栈帧（自栈底向栈顶）。
/// 布局顺序 = isr_common 的 push 逆序，低地址在前。
#[repr(C)]
pub struct ExceptionFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

global_asm!(
    // ---- 无错误码向量：补 0 ----
    ".global isr_0", "isr_0:", "push 0", "push 0", "jmp isr_common",
    ".global isr_1", "isr_1:", "push 0", "push 1", "jmp isr_common",
    ".global isr_2", "isr_2:", "push 0", "push 2", "jmp isr_common",
    ".global isr_3", "isr_3:", "push 0", "push 3", "jmp isr_common",
    ".global isr_4", "isr_4:", "push 0", "push 4", "jmp isr_common",
    ".global isr_5", "isr_5:", "push 0", "push 5", "jmp isr_common",
    ".global isr_6", "isr_6:", "push 0", "push 6", "jmp isr_common",
    ".global isr_7", "isr_7:", "push 0", "push 7", "jmp isr_common",
    // ---- 有错误码向量：CPU 已压栈，仅压向量号 ----
    ".global isr_8", "isr_8:", "push 8", "jmp isr_common",
    ".global isr_9", "isr_9:", "push 0", "push 9", "jmp isr_common",
    ".global isr_10", "isr_10:", "push 10", "jmp isr_common",
    ".global isr_11", "isr_11:", "push 11", "jmp isr_common",
    ".global isr_12", "isr_12:", "push 12", "jmp isr_common",
    ".global isr_13", "isr_13:", "push 13", "jmp isr_common",
    ".global isr_14", "isr_14:", "push 14", "jmp isr_common",
    ".global isr_15", "isr_15:", "push 0", "push 15", "jmp isr_common",
    ".global isr_16", "isr_16:", "push 16", "jmp isr_common",
    ".global isr_17", "isr_17:", "push 0", "push 17", "jmp isr_common",
    ".global isr_18", "isr_18:", "push 0", "push 18", "jmp isr_common",
    ".global isr_19", "isr_19:", "push 0", "push 19", "jmp isr_common",
    ".global isr_20", "isr_20:", "push 0", "push 20", "jmp isr_common",
    ".global isr_21", "isr_21:", "push 0", "push 21", "jmp isr_common",
    ".global isr_22", "isr_22:", "push 0", "push 22", "jmp isr_common",
    ".global isr_23", "isr_23:", "push 0", "push 23", "jmp isr_common",
    ".global isr_24", "isr_24:", "push 0", "push 24", "jmp isr_common",
    ".global isr_25", "isr_25:", "push 0", "push 25", "jmp isr_common",
    ".global isr_26", "isr_26:", "push 0", "push 26", "jmp isr_common",
    ".global isr_27", "isr_27:", "push 0", "push 27", "jmp isr_common",
    ".global isr_28", "isr_28:", "push 0", "push 28", "jmp isr_common",
    ".global isr_29", "isr_29:", "push 0", "push 29", "jmp isr_common",
    ".global isr_30", "isr_30:", "push 0", "push 30", "jmp isr_common",
    ".global isr_31", "isr_31:", "push 0", "push 31", "jmp isr_common",

    // ---- IRQ stub（v0.4：32..47 全部走 irq_common） ----
    // 每个 stub 压入 (error_code=0, vector) 两个 qword，然后跳入 irq_common。
    // 注意：IRQ 绝不能落到 isr_common —— 那会走 exception_dispatch，
    // 把正常的外设中断当成未处理异常而 panic（v0.4 曾因此崩溃）。
    ".global isr_32", "isr_32:", "push 0", "push 32", "jmp irq_common",
    ".global isr_33", "isr_33:", "push 0", "push 33", "jmp irq_common",
    ".global isr_34", "isr_34:", "push 0", "push 34", "jmp irq_common",
    ".global isr_35", "isr_35:", "push 0", "push 35", "jmp irq_common",
    ".global isr_36", "isr_36:", "push 0", "push 36", "jmp irq_common",
    ".global isr_37", "isr_37:", "push 0", "push 37", "jmp irq_common",
    ".global isr_38", "isr_38:", "push 0", "push 38", "jmp irq_common",
    ".global isr_39", "isr_39:", "push 0", "push 39", "jmp irq_common",
    ".global isr_40", "isr_40:", "push 0", "push 40", "jmp irq_common",
    ".global isr_41", "isr_41:", "push 0", "push 41", "jmp irq_common",
    ".global isr_42", "isr_42:", "push 0", "push 42", "jmp irq_common",
    ".global isr_43", "isr_43:", "push 0", "push 43", "jmp irq_common",
    ".global isr_44", "isr_44:", "push 0", "push 44", "jmp irq_common",
    ".global isr_45", "isr_45:", "push 0", "push 45", "jmp irq_common",
    ".global isr_46", "isr_46:", "push 0", "push 46", "jmp irq_common",
    ".global isr_47", "isr_47:", "push 0", "push 47", "jmp irq_common",

    // ---- IRQ 统一入口 ----
    // 栈布局（低地址在前）：[GPRs][vector][error_code][rip][cs][rflags][rsp][ss]
    // 必须像 isr_common 一样保存/恢复全部通用寄存器：IRQ 会打断任意内核代码，
    // 若直接调用 irq_dispatch 而不保存 caller-saved 寄存器，iretq 返回后
    // 被打断的代码会带着被破坏的寄存器继续跑（v0.4 曾因此出现随机 #GP）。
    "irq_common:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov rbx, rsp",       // rbx = &IrqFrame（rbx 已入栈，稍后由 pop 恢复）
    "mov rdi, rbx",       // 第一参数 = &IrqFrame
    "and rsp, -16",       // SysV：call 前 rsp 必须 16 字节对齐
    "call irq_dispatch",
    "mov rsp, rbx",       // 丢弃对齐偏移，回到帧顶
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "add rsp, 16",        // 弹出 vector + error_code
    "iretq",
    "\n",

    // ---- 统一入口：保存寄存器 → 对齐栈 → 调 Rust 分发 → 恢复 → iretq ----
    "isr_common:",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov rbx, rsp",      // rbx = 帧指针（rbx 已入栈，会被恢复）
    "mov rdi, rsp",      // 第一参数 = &ExceptionFrame
    "call exception_dispatch",
    "mov rsp, rbx",       // 恢复 call 前的栈帧（rbx 保存于 push rbx 之后、调用前）
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "add rsp, 16",        // 弹出 vector + error_code（所有向量 stub 均压入二者）
    "iretq",
);

extern "C" {
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();
    fn isr_32();
    fn isr_33();
    fn isr_34();
    fn isr_35();
    fn isr_36();
    fn isr_37();
    fn isr_38();
    fn isr_39();
    fn isr_40();
    fn isr_41();
    fn isr_42();
    fn isr_43();
    fn isr_44();
    fn isr_45();
    fn isr_46();
    fn isr_47();
}

/// 异常/IRQ 处理入口地址表（idt.rs 消费）。
/// 在 Rust 侧构建而非汇编 `.quad`：目标为 PIE，`.rodata` 绝对重定位会被链接器拒绝。
pub fn handler_table() -> [usize; IDT_SIZE] {
    [
        isr_0 as *const () as usize,
        isr_1 as *const () as usize,
        isr_2 as *const () as usize,
        isr_3 as *const () as usize,
        isr_4 as *const () as usize,
        isr_5 as *const () as usize,
        isr_6 as *const () as usize,
        isr_7 as *const () as usize,
        isr_8 as *const () as usize,
        isr_9 as *const () as usize,
        isr_10 as *const () as usize,
        isr_11 as *const () as usize,
        isr_12 as *const () as usize,
        isr_13 as *const () as usize,
        isr_14 as *const () as usize,
        isr_15 as *const () as usize,
        isr_16 as *const () as usize,
        isr_17 as *const () as usize,
        isr_18 as *const () as usize,
        isr_19 as *const () as usize,
        isr_20 as *const () as usize,
        isr_21 as *const () as usize,
        isr_22 as *const () as usize,
        isr_23 as *const () as usize,
        isr_24 as *const () as usize,
        isr_25 as *const () as usize,
        isr_26 as *const () as usize,
        isr_27 as *const () as usize,
        isr_28 as *const () as usize,
        isr_29 as *const () as usize,
        isr_30 as *const () as usize,
        isr_31 as *const () as usize,
        isr_32 as *const () as usize,
        isr_33 as *const () as usize,
        isr_34 as *const () as usize,
        isr_35 as *const () as usize,
        isr_36 as *const () as usize,
        isr_37 as *const () as usize,
        isr_38 as *const () as usize,
        isr_39 as *const () as usize,
        isr_40 as *const () as usize,
        isr_41 as *const () as usize,
        isr_42 as *const () as usize,
        isr_43 as *const () as usize,
        isr_44 as *const () as usize,
        isr_45 as *const () as usize,
        isr_46 as *const () as usize,
        isr_47 as *const () as usize,
    ]
}

/// 向量名表（Intel SDM Vol.3 Table 6-1；B-06：v0.2 起此表曾从向量 9 起错位一位，
/// 导致 #PF 打印成 "reserved (#14)"，#GP 打印成 "page fault"；v0.3 已修正）
const NAMES: [&str; IDT_SIZE] = [
    "divide error",
    "debug",
    "non-maskable interrupt",
    "breakpoint",
    "overflow",
    "bound range exceeded",
    "invalid opcode",
    "device not available",
    "double fault",
    "coprocessor segment overrun",
    "invalid TSS",
    "segment not present",
    "stack-segment fault",
    "general protection fault",
    "page fault",
    "reserved (#15)",
    "x87 float exception",
    "alignment check",
    "machine check",
    "SIMD float exception",
    "virtualization exception",
    "control protection",
    "reserved (#22)",
    "reserved (#23)",
    "reserved (#24)",
    "reserved (#25)",
    "reserved (#26)",
    "reserved (#27)",
    "hypervisor injection",
    "VMM communication",
    "security exception",
    "reserved (#31)",
    // v0.4：IRQ0..IRQ15（向量 32..47）为外设中断，名称后续按设备补；
    // 目前先用“irq(N)”占位，便于串口定位 IRQ 触发来源。
    "irq(0)",
    "irq(1)",
    "irq(2)",
    "irq(3)",
    "irq(4)",
    "irq(5)",
    "irq(6)",
    "irq(7)",
    "irq(8)",
    "irq(9)",
    "irq(10)",
    "irq(11)",
    "irq(12)",
    "irq(13)",
    "irq(14)",
    "irq(15)",
];

fn print_reg(name: &str, val: u64) {
    serial::print(name);
    serial::print("=");
    serial::print_hex(val);
}

/// 统一异常分发：在串口输出受控的前提下打印完整诊断后停机。
///
/// 中断上下文中禁止复用串口输出导致递归，因此仅在 cli 保护下打印。
///
/// 由 `isr_common` 以 RDI = 栈帧指针调用；`rbx` 由 SysV 约定保留。
///
/// 注：IRQ0（向量 0x20）不再经过此函数，而是由向量 stub 直接调用 irq_dispatch。
#[no_mangle]
pub extern "C" fn exception_dispatch(frame: *mut ExceptionFrame) -> ! {
    // SAFETY: frame 由 isr_common 按 ExceptionFrame 布局构造
    let f = unsafe { &*frame };

    // 打印完整诊断前临时关闭中断，避免在打印期间再次触发同一向量。
    unsafe { asm!("cli", options(nostack, preserves_flags)); }

    let name = NAMES.get(f.vector as usize).copied().unwrap_or("unknown");
    serial::println("");
    serial::print("EXCEPTION: ");
    serial::println(name);
    serial::print("vector=");
    serial::print_hex(f.vector);
    serial::print(" error=");
    serial::print_hex(f.error_code);
    serial::println("");

    serial::print("RIP=");
    serial::print_hex(f.rip);
    serial::print(" CS=");
    serial::print_hex(f.cs);
    serial::print(" RFLAGS=");
    serial::print_hex(f.rflags);
    serial::println("");

    serial::print("RSP=");
    serial::print_hex(f.rsp);
    serial::print(" SS=");
    serial::print_hex(f.ss);
    serial::println("");

    print_reg("RAX", f.rax);
    serial::print(" ");
    print_reg("RBX", f.rbx);
    serial::print(" ");
    print_reg("RCX", f.rcx);
    serial::print(" ");
    print_reg("RDX", f.rdx);
    serial::println("");

    print_reg("RSI", f.rsi);
    serial::print(" ");
    print_reg("RDI", f.rdi);
    serial::print(" ");
    print_reg("RBP", f.rbp);
    serial::println("");

    print_reg("R8", f.r8);
    serial::print(" ");
    print_reg("R9", f.r9);
    serial::print(" ");
    print_reg("R10", f.r10);
    serial::print(" ");
    print_reg("R11", f.r11);
    serial::println("");

    print_reg("R12", f.r12);
    serial::print(" ");
    print_reg("R13", f.r13);
    serial::print(" ");
    print_reg("R14", f.r14);
    serial::print(" ");
    print_reg("R15", f.r15);
    serial::println("");

    if f.vector == 14 {
        let cr2: u64;
        unsafe {
            asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
        }
        serial::print("CR2=");
        serial::print_hex(cr2);
        serial::println("");
    }

    serial::println("KERNEL PANIC: unhandled exception");
    crate::halt()
}
