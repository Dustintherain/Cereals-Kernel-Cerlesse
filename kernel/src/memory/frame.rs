//! 物理帧分配：MemoryMap 解析 + 位图帧分配器（v0.3 实现）
//!
//! 策略：1 bit = 1 帧（4KiB），`1` = 已占用，`0` = 空闲。
//! 初始化时位图全部标记占用，仅把 UEFI `EfiConventionalMemory`（type 7）区域
//! 释放为可用；随后显式保留：
//!
//! - 低 1 MiB（IVT/BDA/EBDA 等遗留结构）
//! - 内核映像 `[__kernel_start, __kernel_end)`（链接脚本导出）
//! - BootInfo 本身与内存映射缓冲（bootloader `.bss`，内核仍在读）
//! - 内核堆预留区（由 [`crate::memory::heap::init`] 调用 [`reserve_range`]）
//!
//! 保守策略（ADR-010）：ExitBootServices 之后 BootServicesCode/Data 实际已可回收，
//! 但 v0.3 只回收 ConventionalMemory，其余类型保持保留，先把正确性做实。

use core::ptr::addr_of_mut;
use core::slice;
use shared::BootInfo;

use crate::memory::page::{align_down, align_up, PAGE_SIZE};

/// 物理帧大小 = 页大小（4KiB）
pub const FRAME_SIZE: usize = PAGE_SIZE as usize;
/// 管理上限：4 GiB（QEMU `-m 512M` 远小于此；位图 = 128 KiB 静态数组）
pub const MAX_PHYS_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 最多管理的帧数
pub const MAX_FRAMES: usize = (MAX_PHYS_BYTES / FRAME_SIZE as u64) as usize;

/// 位图字节数（1 bit / 帧）
const BITMAP_BYTES: usize = MAX_FRAMES / 8;

/// UEFI 内存描述符类型：EfiConventionalMemory
const EFI_CONVENTIONAL_MEMORY: u32 = 7;
/// UEFI 内存描述符最小长度（Type/pad/PhysicalStart/VirtualStart/NumberOfPages）
const MIN_DESCRIPTOR_SIZE: usize = 40;

/// 低 1 MiB 保留（BIOS 数据区等）
const LOW_MEMORY_RESERVE: u64 = 0x10_0000;

// 链接脚本（kernel/link.ld）导出
extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

/// 物理帧（4KiB）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PhysFrame {
    number: usize,
}

impl PhysFrame {
    pub const fn from_number(number: usize) -> Self {
        Self { number }
    }

    /// 包含该地址的帧（地址向下对齐到 4KiB）
    pub const fn containing_address(addr: u64) -> Self {
        Self {
            number: (addr / FRAME_SIZE as u64) as usize,
        }
    }

    pub const fn number(&self) -> usize {
        self.number
    }

    pub const fn start_address(&self) -> u64 {
        self.number as u64 * FRAME_SIZE as u64
    }
}

/// 内存统计
#[derive(Clone, Copy)]
pub struct Stats {
    /// 管理器覆盖到的帧总数（最高常规内存帧 + 1）
    pub total_frames: usize,
    /// 当前空闲帧数
    pub free_frames: usize,
}

impl Stats {
    pub fn free_bytes(&self) -> u64 {
        self.free_frames as u64 * FRAME_SIZE as u64
    }
}

// ---- 位图（静态，.bss）：0xFF 表示初始全部占用 ----
static mut BITMAP: [u8; BITMAP_BYTES] = [0xFF; BITMAP_BYTES];
static mut TOTAL_FRAMES: usize = 0;
static mut FREE_FRAMES: usize = 0;
static mut NEXT_FREE: usize = 0;
static mut INITIALIZED: bool = false;

fn bitmap_ptr() -> *mut u8 {
    // 仅取地址，不创建引用
    addr_of_mut!(BITMAP) as *mut u8
}

fn bit_used(frame: usize) -> bool {
    debug_assert!(frame < MAX_FRAMES);
    unsafe { *bitmap_ptr().add(frame / 8) & (1 << (frame % 8)) != 0 }
}

fn set_bit(frame: usize, used: bool) {
    debug_assert!(frame < MAX_FRAMES);
    unsafe {
        let byte = bitmap_ptr().add(frame / 8);
        if used {
            *byte |= 1 << (frame % 8);
        } else {
            *byte &= !(1 << (frame % 8));
        }
    }
}

fn total_frames() -> usize {
    unsafe { *core::ptr::addr_of!(TOTAL_FRAMES) }
}

fn free_frames() -> usize {
    unsafe { *core::ptr::addr_of!(FREE_FRAMES) }
}

fn set_free_frames(value: usize) {
    unsafe { *addr_of_mut!(FREE_FRAMES) = value }
}

/// 解析 BootInfo 内存映射并初始化分配器。
pub fn init(boot_info: &BootInfo) -> Result<Stats, &'static str> {
    if unsafe { *core::ptr::addr_of!(INITIALIZED) } {
        return Ok(stats());
    }
    if boot_info.memory_map.is_null() || boot_info.memory_map_size == 0 {
        return Err("memory map is empty");
    }
    let desc_size = boot_info.memory_descriptor_size;
    if desc_size < MIN_DESCRIPTOR_SIZE || boot_info.memory_map_size % desc_size != 0 {
        return Err("bad memory descriptor size");
    }

    // SAFETY: bootloader 保证 [memory_map, memory_map + memory_map_size) 为有效只读缓冲
    let map = unsafe { slice::from_raw_parts(boot_info.memory_map, boot_info.memory_map_size) };
    let count = map.len() / desc_size;

    // 1. 释放 ConventionalMemory 区域
    let mut max_frame = 0usize;
    for i in 0..count {
        let desc = &map[i * desc_size..(i + 1) * desc_size];
        let typ = u32::from_le_bytes([desc[0], desc[1], desc[2], desc[3]]);
        if typ != EFI_CONVENTIONAL_MEMORY {
            continue;
        }
        let start = u64::from_le_bytes(desc[8..16].try_into().unwrap());
        let pages = u64::from_le_bytes(desc[24..32].try_into().unwrap());
        let end = start.saturating_add(pages.saturating_mul(FRAME_SIZE as u64));
        let first = (start / FRAME_SIZE as u64) as usize;
        let last = ((end.min(MAX_PHYS_BYTES) + FRAME_SIZE as u64 - 1) / FRAME_SIZE as u64) as usize;
        let last = last.min(MAX_FRAMES);
        for f in first..last {
            set_bit(f, false);
        }
        max_frame = max_frame.max(last);
    }
    if max_frame == 0 {
        return Err("no usable memory in map");
    }
    unsafe { *addr_of_mut!(TOTAL_FRAMES) = max_frame; }
    set_free_frames(count_free());

    // 2. 保留关键区域
    reserve_range(0, LOW_MEMORY_RESERVE);
    let (kstart, kend) = kernel_image();
    reserve_range(kstart, kend);
    reserve_range(
        boot_info as *const BootInfo as u64,
        boot_info as *const BootInfo as u64 + core::mem::size_of::<BootInfo>() as u64,
    );
    reserve_range(
        boot_info.memory_map as u64,
        boot_info.memory_map as u64 + boot_info.memory_map_size as u64,
    );

    unsafe {
        *addr_of_mut!(NEXT_FREE) = 0;
        *addr_of_mut!(INITIALIZED) = true;
    }
    Ok(stats())
}

fn count_free() -> usize {
    let total = total_frames();
    let mut n = 0;
    let mut f = 0;
    while f < total {
        if !bit_used(f) {
            n += 1;
        }
        f += 1;
    }
    n
}

fn kernel_image() -> (u64, u64) {
    // 链接脚本符号，仅取地址
    (
        core::ptr::addr_of!(__kernel_start) as u64,
        core::ptr::addr_of!(__kernel_end) as u64,
    )
}

/// 保留 `[start, end)` 覆盖的全部帧；返回新保留的帧数。
pub fn reserve_range(start: u64, end: u64) -> usize {
    if end <= start {
        return 0;
    }
    let first = (align_down(start, FRAME_SIZE as u64) / FRAME_SIZE as u64) as usize;
    let last = (align_up(end, FRAME_SIZE as u64) / FRAME_SIZE as u64) as usize;
    let last = last.min(MAX_FRAMES);
    let mut newly = 0;
    for f in first..last {
        if !bit_used(f) {
            set_bit(f, true);
            set_free_frames(free_frames().saturating_sub(1));
            newly += 1;
        }
    }
    newly
}

/// 分配一帧：优先从上次位置向后扫描，尾部无空闲时回头扫描前部。
pub fn allocate() -> Option<PhysFrame> {
    if !unsafe { *core::ptr::addr_of!(INITIALIZED) } {
        return None;
    }
    let total = total_frames();
    let cursor = unsafe { *core::ptr::addr_of!(NEXT_FREE) };
    let found = find_free(cursor, total).or_else(|| find_free(0, cursor))?;
    set_bit(found, true);
    set_free_frames(free_frames().saturating_sub(1));
    unsafe { *addr_of_mut!(NEXT_FREE) = found + 1 }
    Some(PhysFrame::from_number(found))
}

/// 释放一帧（重复释放会被忽略）。
pub fn free(frame: PhysFrame) {
    let f = frame.number();
    if f >= total_frames() || f >= MAX_FRAMES {
        return;
    }
    if bit_used(f) {
        set_bit(f, false);
        set_free_frames(free_frames() + 1);
        let cursor = unsafe { *core::ptr::addr_of!(NEXT_FREE) };
        if f < cursor {
            unsafe { *addr_of_mut!(NEXT_FREE) = f }
        }
    }
}

fn find_free(start: usize, end: usize) -> Option<usize> {
    let mut f = start;
    while f < end {
        let byte = unsafe { *bitmap_ptr().add(f / 8) };
        if byte == 0xFF {
            f = (f / 8 + 1) * 8; // 整字节占用，跳到下一字节
            continue;
        }
        if byte & (1 << (f % 8)) == 0 {
            return Some(f);
        }
        f += 1;
    }
    None
}

/// 当前统计
pub fn stats() -> Stats {
    Stats {
        total_frames: total_frames(),
        free_frames: free_frames(),
    }
}

/// 帧分配器压力测试（验收项）：
/// 连续分配 1024 帧 → 校验计数/唯一性/对齐/范围/可读写 → 全部释放 → 校验计数还原。
pub fn stress_test() -> Result<(), &'static str> {
    const COUNT: usize = 1024;
    let before = stats();
    if before.free_frames < COUNT + 64 {
        return Err("not enough free frames");
    }

    let mut frames: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        match allocate() {
            Some(f) => frames.push(f.start_address()),
            None => return Err("allocate returned None early"),
        }
    }
    if stats().free_frames + COUNT != before.free_frames {
        return Err("free count mismatch after alloc");
    }

    frames.sort_unstable();
    for pair in frames.windows(2) {
        if pair[0] == pair[1] {
            return Err("duplicate frame returned");
        }
    }
    for &addr in frames.iter() {
        if addr % FRAME_SIZE as u64 != 0 {
            return Err("unaligned frame address");
        }
        if addr >= MAX_PHYS_BYTES {
            return Err("frame outside managed range");
        }
        // 真实可读写校验（首尾各写一个 u64）
        unsafe {
            let head = addr as *mut u64;
            let tail = (addr + FRAME_SIZE as u64 - 8) as *mut u64;
            core::ptr::write_volatile(head, addr ^ 0xA5A5_5A5A);
            core::ptr::write_volatile(tail, addr ^ 0x5A5A_A5A5);
            if core::ptr::read_volatile(head) != addr ^ 0xA5A5_5A5A
                || core::ptr::read_volatile(tail) != addr ^ 0x5A5A_A5A5
            {
                return Err("frame write/readback failed");
            }
        }
    }

    for &addr in frames.iter() {
        free(PhysFrame::containing_address(addr));
    }
    if stats().free_frames != before.free_frames {
        return Err("free count not restored");
    }

    // 复用检查：释放后的帧必须能被再次分配出来
    let mut again: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        match allocate() {
            Some(f) => again.push(f.start_address()),
            None => return Err("re-allocate failed after free"),
        }
    }
    again.sort_unstable();
    if again != frames {
        return Err("frame reuse mismatch");
    }
    for &addr in again.iter() {
        free(PhysFrame::containing_address(addr));
    }
    if stats().free_frames != before.free_frames {
        return Err("free count not restored after reuse");
    }
    Ok(())
}
