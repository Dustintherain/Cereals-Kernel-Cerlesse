//! 内存管理（v0.3 实现）
//!
//! 组成（见 docs/memory.md）：
//! - [`frame`]：解析 BootInfo 的 UEFI 内存映射 + 位图物理帧分配器
//! - [`page`]：Page 抽象与地址换算
//! - [`heap`]：内核堆（内核映像之后预留区 + 空闲链表分配器）
//! - [`allocator`]：`#[global_allocator]`，打通 Rust `alloc`（Box/Vec/String）
//!
//! 启动顺序：帧分配器 → 内核堆 → 自测（堆 / 帧 / 页表），全部通过后
//! `kernel_main` 才继续打印 "Kernel started!"。

pub mod allocator;
pub mod frame;
pub mod heap;
pub mod page;

use crate::arch::x86_64::paging;
use crate::driver::serial;
use shared::BootInfo;

/// v0.3 初始化 + 验收自测。任何一步失败都会让集成测试失败（不出 "Kernel started!"）。
pub fn init(boot_info: &BootInfo) -> Result<(), &'static str> {
    let stats = frame::init(boot_info)?;
    serial::print("mem: frames total=");
    serial::print_dec(stats.total_frames as u64);
    serial::print(" free=");
    serial::print_dec(stats.free_frames as u64);
    serial::print(" (");
    serial::print_dec(stats.free_bytes() / (1024 * 1024));
    serial::println(" MiB usable)");

    let region = heap::init()?;
    serial::print("mem: heap ");
    serial::print_hex(region.start as u64);
    serial::print("-");
    serial::print_hex(region.end as u64);
    serial::print(" (");
    serial::print_dec((region.end - region.start) as u64 / 1024);
    serial::println(" KiB)");

    // 自测顺序有依赖：堆自测先于帧压力测试（压力测试用 Vec 记录帧地址）。
    // 仅在堆自测区段短暂关闭中断，以保护空闲链表不被 IRQ 回调（同样会分配）打断。
    // v0.4 起中断已开启且 IRQ 回调可能使用堆，后续应改为 spinlock 等并发保护。
    unsafe {
        crate::arch::x86_64::disable_irqs();
    }
    heap::selftest()?;
    unsafe {
        crate::arch::x86_64::enable_irqs();
    }
    serial::println("heap: Box/Vec/String PASS");

    frame::stress_test()?;
    serial::println("frame: stress PASS (1024 frames alloc/unique/free)");

    paging::selftest()?;
    serial::println("paging: map/unmap PASS (fresh address space, 3 pages)");

    Ok(())
}
