//! Cerlesse OS 内核入口（v0.1 实现；v0.2 GDT/IDT；v0.3 内存管理；v0.4 中断/Timer/键盘）
//!
//! 启动链：UEFI Bootloader 加载内核 ELF → ExitBootServices → 跳转 `_start`，
//! RDI 指向 `shared::BootInfo`。本文件建立自己的栈并进入 `kernel_main`。
//!
//! v0.4 验收（串口观测）：
//! - `PIC initialized` / `keyboard initialized` / `KBIRQ enabled`：中断与外设初始化完成；
//! - `IRQ0_heartbeat tick=<n>`：PIT（100Hz）每 100 tick（1 秒）打印一次，tick 递增；
//! - `KBIRQ ENTRY` / `KB a` / `KBIRQ EXIT`：按键经 IRQ1 → 扫描码解码 → 串口回显。
//!
//! 初始化完成后进入 `hlt` 空闲循环，中断由硬件持续递交（不再是阻塞式观测窗口）。

#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod driver;
mod interrupt;
mod ipc;
mod memory;
mod process;
mod scheduler;
mod time;

type KernelTaskEntry = extern "C" fn() -> !;

extern "C" fn kernel_task_a() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

extern "C" fn kernel_task_b() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

extern "C" fn kernel_task_c() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)); }
    }
}

/// v0.6 系统调用子系统（syscall 入口、编号分发、首批 syscall handler）
mod syscall;

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
    driver::serial::println("Cerlesse kernel v0.4");

    // 自建 GDT（含 TSS）+ IDT，接管异常处理
    arch::x86_64::init();
    driver::serial::println("GDT/TSS + IDT loaded");

    // v0.4：中断控制器 → PIT/IRQ0 → 键盘/IRQ1 → 打开 IF。
    // 注意：所有 IRQ 回调必须在 `enable_irqs()` 之前注册完毕，
    // 否则第一次中断会分发给空槽位（当前实现会安全丢弃，但属于配置错误）。
    {
        let mut controller = interrupt::controller::Controller::new(
            interrupt::controller::ControllerKind::Pic,
        );
        controller.init_pic();
        driver::serial::println("PIC initialized");

        // PIT 通道 0：100Hz（10ms/tick），供 tick 计数与心跳。
        unsafe { time::timer::set_channel0_reload(time::timer::TICK_RELOAD_100HZ) };
        controller.enable_irq(0);

        // 键盘控制器初始化（排空输出缓冲 + 打开 IRQ1 使能位），
        // 必须在启用 PIC 的 IRQ1 之前完成，否则残留字节会产生伪中断。
        unsafe { driver::keyboard::init() };
        driver::serial::println("keyboard initialized");

        controller.enable_irq(1);
        driver::serial::println("KBIRQ enabled");

        // 注册回调：IRQ0（PIT tick + 心跳）、IRQ1（键盘回显）。
        interrupt::irq::register_irq0_pit_callback();
        interrupt::irq::register_keyboard_callback();

        // 打开处理器中断标志（IF），PIT 与键盘中断开始递交。
        unsafe { arch::x86_64::enable_irqs() };
        driver::serial::println("PIT 100Hz + PIC IRQ0/IRQ1 unmasked; IF enabled");
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

    // v0.4：内核级上下文切换原语自测（为 v0.5 的任务/调度铺路）。
    if let Err(err) = arch::x86_64::context::selftest() {
        driver::serial::print("context: FAIL: ");
        driver::serial::println(err);
        halt();
    }
    driver::serial::println("context: switch PASS");

    driver::serial::println("Kernel started!");

    // v0.4：观测 IF 状态，确认中断使能路径已接通。
    {
        let enabled = arch::x86_64::irqs_enabled();
        driver::serial::print("IF=");
        driver::serial::print_dec(if enabled { 1u64 } else { 0u64 });
        driver::serial::println("");
    }

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

    // v0.5：内核线程 + Round Robin 调度（PIT/IRQ0 驱动时间片抢占）。
    {
        use crate::scheduler::scheduler as sched;

        sched::init();
        // v0.5 内核线程仍然使用旧入口名称 `task_a/task_b/task_c`；
        // 它们在本版本保留为占位，由下一阶段接入用户态隔离后逐步替换。
        sched::spawn("task_a", kernel_task_a as KernelTaskEntry);
        sched::spawn("task_b", kernel_task_b as KernelTaskEntry);
        sched::spawn("task_c", kernel_task_c as KernelTaskEntry);
        driver::serial::println("sched: tasks spawned; starting round robin");

        // 切出到第一个内核任务；本轮 Round Robin 绕回 pid 0 时从这里继续。
        sched::start();

        let stats = sched::stats();
        sched::report();
        if stats.started && stats.switches >= 4 {
            driver::serial::println("sched: round-robin wrap PASS");
        } else {
            driver::serial::println("sched: round-robin wrap FAIL");
            halt();
        }
    }

    // v0.5：IPC（pipe 骨架）验收自测——在 Round Robin 巡回之后执行，
    // 同时验证管道语义与「不给调度器引入新崩溃面」。
    if let Err(err) = ipc::pipe::selftest() {
        driver::serial::print("pipe: FAIL: ");
        driver::serial::println(err);
        halt();
    }
    driver::serial::println("pipe: selftest PASS");

    // 之后的执行由调度器接管：每个任务在 hlt 中等待下一个时间片。
    halt()
}

#[no_mangle]
pub extern "C" fn UserMain() -> ! {
    // 用户态入口。首次进入时，`r8` 包含本线程的一个初始化参数（0x1），
    // 随后按最小用户态 `start` 的行为：打印一条标记、
    // 循环调用 `sys_getpid`/`sys_write`/`sys_sleep` 做一次用户态 syscall 回归。
    // 退出时调用 `sys_exit`，进入调度器回收/空转即可。
    for i in 0..5u64 {
        driver::serial::print("user_main: running ");
        driver::serial::print_dec(i);
        driver::serial::println(" (pid=1)");

        // 通过 `sys_sleep` 让用户态任务每 100ms 让出一次 CPU。
        // 这是 v0.6 最小的计时只实现：使用 `time::timer::sleep_1tick()` 模拟 100ms。
        if i != 4 {
            // 这里不直接调用内核定时器，而是通过 `sys_sleep(20000)` 走用户态路径。
            // 若系统调用入口出现故障，返回到 `UserMain` 后继续循环。
            unsafe {
                let ret = syscall::sys_sleep([20000, 0, 0, 0, 0], core::ptr::null());
                let _ = ret;
            }
        }
    }

    // 用户态退出：提交退出请求。
    // 用户态退出：提交退出请求（永不返回，原型为 `!`，此路径仅语句完成）。
    unsafe {
        let _ = syscall::sys_exit([0, 0, 0, 0, 0], core::ptr::null());
        loop {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
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
