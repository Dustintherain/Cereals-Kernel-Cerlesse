//! 内核堆（v0.3 实现）
//!
//! 布局（ADR-011）：从内核映像末尾 `__kernel_end`（链接脚本导出）向后预留
//! [`HEAP_SIZE`] 字节，这些物理帧由帧分配器保留（不再对外分配）；
//! v0.3 内核为恒等映射（ADR-009），故堆地址 = 物理地址。
//!
//! 算法：首次适配（first-fit）空闲链表分配器
//! - 块头 16 字节：`size`（整块字节数，含块头）+ `next_free`（空闲时有效）
//! - 分配时按请求对齐切分，剩余部分够一个最小块（32B）就切出来还回链表
//! - 释放时按地址有序插入空闲链表，并与相邻块合并（避免碎片）
//!
//! 单核 + 关中断（v0.4 之前）下不引入锁；v0.4 起中断处理需要分配时再加自旋锁。

use alloc::alloc::{Layout};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::{addr_of, addr_of_mut};

use crate::arch::x86_64::paging;
use crate::driver::serial;

/// 内核堆大小：1 MiB
pub const HEAP_SIZE: usize = 1024 * 1024;
/// 块头：size + next_free
const HEADER_SIZE: usize = core::mem::size_of::<usize>() * 2;
/// 对齐粒度：块起点与块大小都是 16 字节的倍数，因此任意 ≤16 的对齐请求都不会产生不可表示的前导空洞
const MIN_ALIGN: usize = 16;
/// 最小有效载荷（切分阈值：剩余 ≥ 块头 + 该值 才切成独立块）
const MIN_PAYLOAD: usize = 16;

/// 堆区域
#[derive(Clone, Copy)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

/// 空闲/已用块
#[repr(C)]
struct Block {
    /// 整块大小（含块头），16 字节对齐
    size: usize,
    /// 空闲链表指针（仅空闲块有效）
    next_free: *mut Block,
}

/// 空闲链表堆
pub struct Heap {
    head: *mut Block,
    start: usize,
    end: usize,
    initialized: bool,
    /// 当前已分配字节（含块头）
    allocated: usize,
    /// 当前已分配块数
    live_blocks: usize,
}

impl Heap {
    pub const fn empty() -> Self {
        Self {
            head: core::ptr::null_mut(),
            start: 0,
            end: 0,
            initialized: false,
            allocated: 0,
            live_blocks: 0,
        }
    }

    /// 用 `[start, end)` 初始化：整段作为一个空闲块。
    ///
    /// # Safety
    /// 该区间必须是本内核独占、可读写的内存。
    pub unsafe fn init(&mut self, start: usize, end: usize) -> bool {
        let start = round_up(start, MIN_ALIGN);
        let end = end & !(MIN_ALIGN - 1);
        if end <= start + HEADER_SIZE + MIN_PAYLOAD {
            return false;
        }
        self.start = start;
        self.end = end;
        self.allocated = 0;
        self.live_blocks = 0;
        let block = start as *mut Block;
        // SAFETY: 调用方保证区间有效
        unsafe {
            (*block).size = end - start;
            (*block).next_free = core::ptr::null_mut();
        }
        self.head = block;
        self.initialized = true;
        true
    }

    pub fn allocated_bytes(&self) -> usize {
        self.allocated
    }

    pub fn live_blocks(&self) -> usize {
        self.live_blocks
    }

    pub fn region(&self) -> Region {
        Region {
            start: self.start,
            end: self.end,
        }
    }

    /// 空闲字节总数（遍历空闲链表）
    pub fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut cur = self.head;
        while !cur.is_null() {
            unsafe {
                total += (*cur).size;
                cur = (*cur).next_free;
            }
        }
        total
    }

    /// 空闲块数
    pub fn free_blocks(&self) -> usize {
        let mut n = 0;
        let mut cur = self.head;
        while !cur.is_null() {
            n += 1;
            unsafe { cur = (*cur).next_free };
        }
        n
    }

    /// 首次适配分配。
    ///
    /// # Safety
    /// 与 [`GlobalAlloc`] 语义一致；返回指针按 `layout.align()` 对齐。
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialized {
            return core::ptr::null_mut();
        }
        let align = layout.align().max(MIN_ALIGN);
        let size = round_up(layout.size().max(1), MIN_ALIGN);

        let mut link: *mut *mut Block = &mut self.head;
        while !(*link).is_null() {
            let cur = *link;
            // SAFETY: cur 来自空闲链表，指向本堆内的有效块
            let (base, block_size, next) = unsafe { (cur as usize, (*cur).size, (*cur).next_free) };
            let payload = round_up(base + HEADER_SIZE, align);
            // 前导 padding 恒为 16 的倍数：0 或 ≥ HEADER_SIZE，可表示为一个独立空闲块
            let pad = payload - (base + HEADER_SIZE);
            let want = pad + HEADER_SIZE + size;
            if block_size < want {
                link = unsafe { &mut (*cur).next_free };
                continue;
            }
            let remainder = block_size - want;
            let block = (payload - HEADER_SIZE) as *mut Block;
            // 剩余部分 ≥ 一个最小块就切出来，否则并入本次分配（避免碎片）
            // 注意：块从 base+pad 开始，块大小 = HEADER + size（= want - pad），
            // 前导 pad 字节归属 front 空闲块，绝不能算进已分配块。
            let split = remainder >= HEADER_SIZE + MIN_PAYLOAD;
            let alloc_size = if split { want - pad } else { block_size - pad };
            // SAFETY: block 落在本空闲块内部，是本次分配的新块头
            unsafe {
                (*block).size = alloc_size;
                if split {
                    let rest = (base + want) as *mut Block;
                    (*rest).size = remainder;
                    if pad > 0 {
                        let front = base as *mut Block;
                        (*front).size = pad;
                        (*front).next_free = rest;
                        (*rest).next_free = next;
                        *link = front;
                    } else {
                        (*rest).next_free = next;
                        *link = rest;
                    }
                } else if pad > 0 {
                    let front = base as *mut Block;
                    (*front).size = pad;
                    (*front).next_free = next;
                    *link = front;
                } else {
                    *link = next;
                }
            }
            self.allocated += alloc_size;
            self.live_blocks += 1;
            return payload as *mut u8;
        }
        core::ptr::null_mut()
    }

    /// 释放并合并相邻空闲块。
    ///
    /// # Safety
    /// `ptr` 必须来自本堆的 [`Heap::alloc`]。
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() || !self.initialized {
            return;
        }
        let base = ptr as usize - HEADER_SIZE;
        let block = base as *mut Block;
        // SAFETY: 块头就在返回指针之前
        let size = unsafe { (*block).size };
        self.allocated = self.allocated.saturating_sub(size);
        self.live_blocks = self.live_blocks.saturating_sub(1);

        // 找到插入点（按地址有序），同时记录前驱
        let mut prev: *mut Block = core::ptr::null_mut();
        let mut link: *mut *mut Block = &mut self.head;
        while !(*link).is_null() && ((*link) as usize) < base {
            prev = *link;
            link = unsafe { &mut (*prev).next_free };
        }

        unsafe {
            // 先与后继合并
            let next = *link;
            if !next.is_null() && base + size == next as usize {
                (*block).size = size + (*next).size;
                (*block).next_free = (*next).next_free;
            } else {
                (*block).next_free = next;
            }
            // 再与前驱合并
            if !prev.is_null() && prev as usize + (*prev).size == base {
                (*prev).size += (*block).size;
                (*prev).next_free = (*block).next_free;
            } else {
                *link = block;
            }
        }
    }
}

/// 把 `value` 向上对齐到 `align`（align 为 2 的幂）
const fn round_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

// ---- 全局堆实例 ----
static mut KERNEL_HEAP: Heap = Heap::empty();

/// 预留并初始化内核堆；返回堆区域。
pub fn init() -> Result<Region, &'static str> {
    let start = round_up(kernel_end(), crate::memory::frame::FRAME_SIZE);
    let end = start + HEAP_SIZE;
    let expected = (end - start) / crate::memory::frame::FRAME_SIZE;
    let reserved = crate::memory::frame::reserve_range(start as u64, end as u64);
    if reserved != expected {
        serial::print("heap: region 0x");
        serial::print_hex(start as u64);
        serial::print("-0x");
        serial::print_hex(end as u64);
        serial::print(" reserved=");
        serial::print_dec(reserved as u64);
        serial::print(" expected=");
        serial::print_dec(expected as u64);
        serial::println("");
        return Err("heap region overlaps reserved memory");
    }
    paging::ensure_identity_range(start as u64, end as u64)?;
    // SAFETY: 上述区间已确认独占且恒等映射
    unsafe {
        if !(*addr_of_mut!(KERNEL_HEAP)).init(start, end) {
            return Err("heap region too small");
        }
    }
    // SAFETY: 只读访问已初始化的堆
    Ok(unsafe { (*addr_of!(KERNEL_HEAP)).region() })
}

/// 内核映像末尾（链接脚本 `__kernel_end`）
fn kernel_end() -> usize {
    extern "C" {
        static __kernel_end: u8;
    }
    // 链接脚本符号，仅取地址
    core::ptr::addr_of!(__kernel_end) as usize
}

// ---- 供全局分配器使用的入口 ----

/// 分配（`Layout` 语义同 `GlobalAlloc`）
///
/// # Safety
/// 见 [`core::alloc::GlobalAlloc::alloc`]
pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    unsafe { (*addr_of_mut!(KERNEL_HEAP)).alloc(layout) }
}

/// 释放
///
/// # Safety
/// `ptr` 必须来自 [`alloc`]，且 `layout` 与分配时一致
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    unsafe { (*addr_of_mut!(KERNEL_HEAP)).dealloc(ptr, layout) }
}

/// 当前已分配字节数（含块头）
pub fn allocated_bytes() -> usize {
    // SAFETY: 只读访问
    unsafe { (*addr_of!(KERNEL_HEAP)).allocated_bytes() }
}

/// 空闲块数
pub fn free_blocks() -> usize {
    // SAFETY: 只读访问
    unsafe { (*addr_of!(KERNEL_HEAP)).free_blocks() }
}

/// 空闲字节总数
pub fn free_bytes() -> usize {
    // SAFETY: 只读访问
    unsafe { (*addr_of!(KERNEL_HEAP)).free_bytes() }
}

/// 当前存活（已分配）块数
pub fn live_blocks() -> usize {
    // SAFETY: 只读访问
    unsafe { (*addr_of!(KERNEL_HEAP)).live_blocks() }
}

/// 堆区域
pub fn region() -> Region {
    // SAFETY: 只读访问
    unsafe { (*addr_of!(KERNEL_HEAP)).region() }
}

/// 堆自测（验收项）：
/// 1) Rust `alloc` 集成：Box / Vec / String
/// 2) 空闲链表压力：混合大小与对齐、交错释放、填充模式校验（抓重叠/越界）
/// 3) 全部释放后：分配字节归零、空闲链表完全合并为 1 块
pub fn selftest() -> Result<(), &'static str> {
    use alloc::boxed::Box;

    // 1) Box / Vec / String
    {
        let boxed = Box::new([0xABu8; 1024]);
        if boxed[0] != 0xAB || boxed[1023] != 0xAB {
            return Err("Box payload mismatch");
        }
        let mut v: Vec<u64> = Vec::new();
        for i in 0..10_000u64 {
            v.push(i);
        }
        if v.iter().sum::<u64>() != 49_995_000 || v.len() != 10_000 {
            return Err("Vec payload mismatch");
        }
        let mut s = String::new();
        for _ in 0..100 {
            s.push_str("cerlesse");
        }
        if s.len() != 800 || !s.starts_with("cerlesse") || !s.ends_with("cerlesse") {
            return Err("String payload mismatch");
        }
        // 大块分配（跨多个堆块）
        let big: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
        if big[64 * 1024 - 1] != ((64 * 1024 - 1) % 251) as u8 {
            return Err("large Vec payload mismatch");
        }
    }
    if allocated_bytes() != 0 || live_blocks() != 0 {
        return Err("heap leak after Rust alloc test");
    }

    // 2) 空闲链表压力测试
    const N: usize = 256;
    let mut live: Vec<(*mut u8, Layout, u8)> = Vec::with_capacity(N);
    for i in 0..N {
        let size = 8 + (i * 37) % 512;
        let align = 1usize << (3 + (i % 4)); // 8 / 16 / 32 / 64
        let layout = Layout::from_size_align(size, align).map_err(|_| "bad layout")?;
        // SAFETY: 全局分配器已初始化
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if ptr.is_null() {
            return Err("heap allocation failed (out of heap)");
        }
        if (ptr as usize) % align != 0 {
            return Err("allocation not aligned");
        }
        let tag = ((i * 7 + 3) % 251) as u8;
        unsafe {
            for j in 0..size {
                *ptr.add(j) = tag ^ 0x5A;
            }
        }
        live.push((ptr, layout, tag));
        // 交错释放：释放一半，人为制造碎片
        if i % 2 == 1 {
            let (p, l, _) = live.pop().unwrap();
            unsafe { alloc::alloc::dealloc(p, l) };
        }
    }
    // 校验所有存活块内容未被覆盖（重叠/越界的直接证据）
    for &(ptr, layout, tag) in live.iter() {
        for j in 0..layout.size() {
            if unsafe { *ptr.add(j) } != tag ^ 0x5A {
                return Err("heap block corrupted (overlap bug)");
            }
        }
    }
    for &(ptr, layout, _) in live.iter() {
        unsafe { alloc::alloc::dealloc(ptr, layout) };
    }
    drop(live);

    if allocated_bytes() != 0 || live_blocks() != 0 {
        return Err("heap leak after stress test");
    }
    if free_blocks() != 1 {
        return Err("free list did not coalesce back to one block");
    }
    let region = region();
    if free_bytes() != region.end - region.start {
        return Err("heap accounting mismatch");
    }
    Ok(())
}
