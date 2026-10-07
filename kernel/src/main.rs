//! Cerlesse OS 内核入口（v0.1 实现，v0.2 增加 GDT/IDT）
//!
//! 启动链：UEFI Bootloader 加载本内核 ELF → ExitBootServices → 跳转 `_start`，
//! RDI 指向 `shared::BootInfo`。本文件建立自己的栈并进入 `kernel_main`。

#![no_std]
#![no_main]

mod arch;
mod driver;

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
    driver::serial::println("Cerlesse kernel v0.2");

    // 自建 GDT（含 TSS）+ IDT，接管异常处理
    arch::x86_64::init();
    driver::serial::println("GDT/TSS + IDT loaded");

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
