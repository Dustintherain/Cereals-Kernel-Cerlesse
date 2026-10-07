//! Cerlesse OS 内核入口（v0.1 实现；v0.2 GDT/IDT；v0.3 内存管理）
//!
//! 启动链：UEFI Bootloader 加载本内核 ELF → ExitBootServices → 跳转 `_start`，
//! RDI 指向 `shared::BootInfo`。本文件建立自己的栈并进入 `kernel_main`。

#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod driver;
mod interrupt;
mod memory;

use core::arch::global_asm;
use shared::BootInfo;

// 栈与入口：bootloader 以 RDI = &BootInfo 跳入 _start。
global_asm!(
    ".section .bss",
    ".align 16",
    ".global stack_bottom",
    "stack_bottom:",
    ".space 65536",
    ".global stack_top",
    "stack_top:",
    ".text",
    ".global _start",
    "_start:",
    "cli", // B-01修复：入口即关中断，关闭固件 IDT 下的中断窗口
    "lea rsp, [rip + stack_top]",
    "xor rbp, rbp",
    "call kernel_main",
    "hlt_loop:",
    "hlt",
    "jmp hlt_loop",
);

#[no_mangle]
pub extern "C" fn kernel_main(boot_info: *const BootInfo) -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    driver::serial::init();
    driver::serial::println("Cerlesse kernel v0.3");

    // 自建 GDT（含 TSS）+ IDT，接管异常处理
    arch::x86_64::init();
    driver::serial::println("GDT/TSS + IDT loaded");

    // v0.4：初始化中断控制器（PIC），随后再决定启用哪些 IRQ
    {
        let mut controller = interrupt::controller::Controller::new(
            interrupt::controller::ControllerKind::Pic,
        );
        controller.init_pic();
        // 先保持全部 IRQ 屏蔽；后续再按需 enable_irq(...)。
        driver::serial::println("PIC initialized");
    }

    if boot_info.is_null() {
        driver::serial::println("panic: null BootInfo");
        halt();
    }
    // SAFETY: 非空指针，由 bootloader 按 shared::BootInfo 布局构造。
    let bi = unsafe { &*boot_info };
    if bi.magic != shared::BOOT_MAGIC {
        driver::serial::println("panic: bad BootInfo magic");
        halt();
    }

    driver::serial::print("memory map: ");
    driver::serial::print_hex(bi.memory_map_size as u64);
    driver::serial::print(" bytes, desc size ");
    driver::serial::print_hex(bi.memory_descriptor_size as u64);
    driver::serial::println("");

    // v0.3：帧分配器 → 内核堆 → 自测（heap/frame/paging）
    if let Err(err) = memory::init(bi) {
        driver::serial::print("mem: FAIL: ");
        driver::serial::println(err);
        halt();
    }

    driver::serial::println("Kernel started!");

    // v0.2 验收：异常注入测试（feature 门控，默认构建不触发）
    #[cfg(feature = "exception-test")]
    {
        driver::serial::println("test: triggering divide error (#DE)");
        unsafe {
            core::arch::asm!(
                "xor eax, eax",
                "div eax",
                lateout("eax") _,
                lateout("edx") _,
                options(nostack),
            );
        }
    }

    // v0.3 验收：页错误诊断测试（feature 门控，默认构建不触发）
    #[cfg(feature = "pagefault-test")]
    {
        const UNMAPPED: u64 = 0x0000_5000_0000_0000;
        driver::serial::println("test: touching unmapped address");
        unsafe {
            core::ptr::write_volatile(UNMAPPED as *mut u8, 0xAA);
        }
    }

    halt()
}

pub(crate) fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    driver::serial::println("KERNEL PANIC");
    if let Some(loc) = info.location() {
        driver::serial::print(loc.file());
        driver::serial::print(":");
        driver::serial::print_hex(loc.line() as u64);
        driver::serial::println("");
    }
    halt()
}
