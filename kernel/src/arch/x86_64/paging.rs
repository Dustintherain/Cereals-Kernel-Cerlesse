//! x86_64 四级页表操作（v0.3 实现）——页表遍历、映射/解除映射、TLB 刷新
//!
//! 约定（ADR-009）：v0.3 内核运行在 bootloader/固件建立的**恒等映射**上，
//! 因此页表本身用物理地址直接访问（phys == virt）。
//!
//! 结构：PML4 → PDPT → PD → PT，每级 512 项，8 字节/项。
//! 支持识别固件留下的 1 GiB（PDPT 层）与 2 MiB（PD 层）大页；新建映射一律 4 KiB。

use crate::arch::x86_64::cpu;
use crate::driver::serial;
use crate::memory::frame::{self, PhysFrame};
use crate::memory::page::{PAGE_SIZE, PAGE_SIZE_1G, PAGE_SIZE_2M};

/// 每级页表项数
pub const ENTRY_COUNT: usize = 512;

/// 页表项标志位
pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
/// v0.6 用户页映射使用；v0.3 内核映射不置位
#[allow(dead_code)]
pub const USER: u64 = 1 << 2;
pub const HUGE: u64 = 1 << 7;
pub const NO_EXEC: u64 = 1 << 63;

/// 物理地址掩码（4KiB 表基址 / 2MiB 与 1GiB 大页基址共用低位清零）
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// 映射错误
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    /// 已存在映射（不覆盖，调用方先 unmap）
    AlreadyMapped,
    /// 该地址未映射
    NotMapped,
    /// 地址未对齐到 4KiB
    Unaligned,
    /// 物理帧分配失败
    NoFrame,
}

/// 页表项
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Entry(u64);

impl Entry {
    pub const fn is_present(self) -> bool {
        self.0 & PRESENT != 0
    }

    /// 是否为大页（1 GiB / 2 MiB）
    pub const fn is_huge(self) -> bool {
        self.0 & HUGE != 0
    }

    /// 项内指向的物理地址（4KiB 对齐）
    pub const fn address(self) -> u64 {
        self.0 & ADDR_MASK
    }

    /// 标志位（去掉地址）
    pub const fn flags(self) -> u64 {
        self.0 & !ADDR_MASK
    }

    fn set(&mut self, phys: u64, flags: u64) {
        self.0 = (phys & ADDR_MASK) | flags;
    }

    fn clear(&mut self) {
        self.0 = 0;
    }
}

/// 一张 4KiB 页表（512 项）
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [Entry; ENTRY_COUNT],
}

/// 以恒等映射直接访问某张页表
///
/// # Safety
/// `phys` 必须是有效、4KiB 对齐且当前可写访问的页表帧地址。
unsafe fn table_at(phys: u64) -> &'static mut PageTable {
    unsafe { &mut *(phys as *mut PageTable) }
}

/// 第 `level` 级页表索引：0=PT，1=PD，2=PDPT，3=PML4
const fn index(virt: u64, level: usize) -> usize {
    ((virt >> (12 + 9 * level)) & 0x1FF) as usize
}

/// 翻译结果
#[derive(Clone, Copy)]
pub struct Translation {
    /// 映射到的物理地址（含页内偏移）
    pub phys: u64,
    /// 页表项标志
    pub flags: u64,
    /// 页大小（4KiB / 2MiB / 1GiB）
    pub page_size: u64,
}

/// 在 `root` 页表中翻译虚拟地址（支持固件大页）。
pub fn translate(root: u64, virt: u64) -> Option<Translation> {
    // SAFETY: 调用方保证 root 是有效页表根
    let l4 = unsafe { table_at(root) };
    let e4 = l4.entries[index(virt, 3)];
    if !e4.is_present() {
        return None;
    }
    let l3 = unsafe { table_at(e4.address()) };
    let e3 = l3.entries[index(virt, 2)];
    if !e3.is_present() {
        return None;
    }
    if e3.is_huge() {
        return Some(Translation {
            phys: e3.address() + (virt & (PAGE_SIZE_1G - 1)),
            flags: e3.flags(),
            page_size: PAGE_SIZE_1G,
        });
    }
    let l2 = unsafe { table_at(e3.address()) };
    let e2 = l2.entries[index(virt, 1)];
    if !e2.is_present() {
        return None;
    }
    if e2.is_huge() {
        return Some(Translation {
            phys: e2.address() + (virt & (PAGE_SIZE_2M - 1)),
            flags: e2.flags(),
            page_size: PAGE_SIZE_2M,
        });
    }
    let l1 = unsafe { table_at(e2.address()) };
    let e1 = l1.entries[index(virt, 0)];
    if !e1.is_present() {
        return None;
    }
    Some(Translation {
        phys: e1.address() + (virt & (PAGE_SIZE - 1)),
        flags: e1.flags(),
        page_size: PAGE_SIZE,
    })
}

/// 逐级取下一级表；若不存在则从帧分配器申请一张并清零。
fn ensure_next(entry: &mut Entry) -> Result<u64, MapError> {
    if entry.is_present() {
        if entry.is_huge() {
            // 已存在大页覆盖该区域，v0.3 不做拆分
            return Err(MapError::AlreadyMapped);
        }
        return Ok(entry.address());
    }
    let frame = frame::allocate().ok_or(MapError::NoFrame)?;
    let phys = frame.start_address();
    // SAFETY: 刚分配的帧，恒等映射可写
    unsafe { core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize) };
    entry.set(phys, PRESENT | WRITABLE);
    Ok(phys)
}

/// 建立一页 4 KiB 映射（中间级缺失的页表自动分配）。
pub fn map_4k(root: u64, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    if virt % PAGE_SIZE != 0 || phys % PAGE_SIZE != 0 {
        return Err(MapError::Unaligned);
    }
    // SAFETY: 见模块约定（恒等映射）
    let l4 = unsafe { table_at(root) };
    let pdpt = ensure_next(&mut l4.entries[index(virt, 3)])?;
    let l3 = unsafe { table_at(pdpt) };
    let pd = ensure_next(&mut l3.entries[index(virt, 2)])?;
    let l2 = unsafe { table_at(pd) };
    let pt = ensure_next(&mut l2.entries[index(virt, 1)])?;
    let l1 = unsafe { table_at(pt) };
    let entry = &mut l1.entries[index(virt, 0)];
    if entry.is_present() {
        return Err(MapError::AlreadyMapped);
    }
    entry.set(phys, PRESENT | (flags & !ADDR_MASK));
    Ok(())
}

/// 解除一页 4 KiB 映射，返回原物理地址。
pub fn unmap_4k(root: u64, virt: u64) -> Result<u64, MapError> {
    if virt % PAGE_SIZE != 0 {
        return Err(MapError::Unaligned);
    }
    let e4 = unsafe { table_at(root) }.entries[index(virt, 3)];
    if !e4.is_present() {
        return Err(MapError::NotMapped);
    }
    let e3 = unsafe { table_at(e4.address()) }.entries[index(virt, 2)];
    if !e3.is_present() {
        return Err(MapError::NotMapped);
    }
    if e3.is_huge() {
        return Err(MapError::AlreadyMapped); // 大页不拆分，v0.3 不支持
    }
    let e2 = unsafe { table_at(e3.address()) }.entries[index(virt, 1)];
    if !e2.is_present() {
        return Err(MapError::NotMapped);
    }
    if e2.is_huge() {
        return Err(MapError::AlreadyMapped);
    }
    let entry = &mut unsafe { table_at(e2.address()) }.entries[index(virt, 0)];
    if !entry.is_present() {
        return Err(MapError::NotMapped);
    }
    let phys = entry.address();
    entry.clear();
    Ok(phys)
}

/// 刷新单个虚拟地址的 TLB 项
pub fn flush_tlb(virt: u64) {
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    }
}

/// 当前页表根（CR3）
pub fn current_root() -> u64 {
    cpu::read_cr3()
}

/// 确保 `[start, end)` 在当前地址空间中存在恒等映射（缺失则按 4 KiB 补齐）。
/// 内核堆使用：堆帧以物理地址直接访问，必须保证映射存在且为恒等。
pub fn ensure_identity_range(start: u64, end: u64) -> Result<(), &'static str> {
    use crate::memory::page::{align_down, align_up, Page};

    let root = current_root();
    let first = align_down(start, PAGE_SIZE);
    let pages = (align_up(end, PAGE_SIZE) - first) / PAGE_SIZE;
    let mut mapped = 0;
    for page in Page::containing_address(first).range(pages) {
        let addr = page.start_address();
        match translate(root, addr) {
            Some(t) if t.phys == addr => {}
            Some(_) => return Err("range already mapped to a different physical address"),
            None => {
                map_4k(root, addr, addr, PRESENT | WRITABLE | NO_EXEC)
                    .map_err(|_| "identity map failed")?;
                flush_tlb(addr);
                mapped += 1;
            }
        }
    }
    if mapped > 0 {
        serial::print("mem: heap pages mapped=");
        serial::print_dec(mapped as u64);
        serial::println("");
    }
    Ok(())
}

/// 独立地址空间（新 PML4 根）。v0.3 仅用于页表自测；
/// v0.5/v0.6 将扩展为进程地址空间并支持 CR3 切换。
pub struct AddressSpace {
    root: u64,
}

impl AddressSpace {
    /// 新建空地址空间（分配 PML4 帧并清零）。
    pub fn new() -> Result<Self, MapError> {
        let frame = frame::allocate().ok_or(MapError::NoFrame)?;
        let root = frame.start_address();
        // SAFETY: 新分配的帧，恒等映射可写
        unsafe { core::ptr::write_bytes(root as *mut u8, 0, PAGE_SIZE as usize) };
        Ok(Self { root })
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn is_current(&self) -> bool {
        current_root() == self.root
    }

    pub fn map(&self, virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
        map_4k(self.root, virt, phys, flags)
    }

    pub fn unmap(&self, virt: u64) -> Result<u64, MapError> {
        unmap_4k(self.root, virt)
    }

    pub fn translate(&self, virt: u64) -> Option<Translation> {
        translate(self.root, virt)
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // SAFETY: 本空间由 new() 创建，页表与映射页均归其所有
        unsafe { free_level(self.root, 3) };
    }
}

/// 递归释放整棵页表树：叶子映射页 + 各级页表帧。
///
/// # Safety
/// 仅可用于完全由本内核创建、未安装为当前 CR3 的地址空间。
unsafe fn free_level(table_phys: u64, level: usize) {
    let table = unsafe { table_at(table_phys) };
    for i in 0..ENTRY_COUNT {
        let entry = table.entries[i];
        if !entry.is_present() {
            continue;
        }
        if level == 0 {
            frame::free(PhysFrame::containing_address(entry.address()));
        } else if !entry.is_huge() {
            unsafe { free_level(entry.address(), level - 1) };
        }
    }
    frame::free(PhysFrame::containing_address(table_phys));
}

/// 页表 Mapper 自测（验收项）：新建地址空间 → 映射 3 页 → 翻译校验 →
/// unmap/重复 unmap/重复 map 校验 → 回收，帧计数必须还原。
pub fn selftest() -> Result<(), &'static str> {
    const TEST_VIRT: u64 = 0x1000_0000; // 测试用虚拟地址（PML4 0，独立空间内无副作用）
    let before = frame::stats().free_frames;

    let space = AddressSpace::new().map_err(|_| "AddressSpace::new failed")?;
    if space.root() % PAGE_SIZE != 0 || space.root() == current_root() {
        return Err("bad address space root");
    }
    if space.is_current() {
        return Err("new address space equals current");
    }

    let mut frames = [0u64; 3];
    for i in 0..3 {
        let f = frame::allocate().ok_or("no frame for mapping")?;
        frames[i] = f.start_address();
        let pattern = 0xC0FF_EE00u64 ^ i as u64;
        unsafe {
            core::ptr::write_volatile(frames[i] as *mut u64, pattern);
            if core::ptr::read_volatile(frames[i] as *const u64) != pattern {
                return Err("mapped frame not writable");
            }
        }
        space
            .map(
                TEST_VIRT + i as u64 * PAGE_SIZE,
                frames[i],
                PRESENT | WRITABLE | NO_EXEC,
            )
            .map_err(|_| "map failed")?;
    }

    for i in 0..3 {
        let t = space
            .translate(TEST_VIRT + i as u64 * PAGE_SIZE)
            .ok_or("translate after map failed")?;
        if t.phys != frames[i] || t.page_size != PAGE_SIZE || t.flags & PRESENT == 0 {
            return Err("translation mismatch");
        }
    }
    if space.translate(TEST_VIRT + 0x2000_0000).is_some() {
        return Err("unmapped address translated");
    }
    if space
        .map(TEST_VIRT, frames[0], PRESENT | WRITABLE)
        .is_ok()
    {
        return Err("double map succeeded");
    }

    let phys = space
        .unmap(TEST_VIRT + PAGE_SIZE)
        .map_err(|_| "unmap failed")?;
    if phys != frames[1] {
        return Err("unmap returned wrong frame");
    }
    if space.translate(TEST_VIRT + PAGE_SIZE).is_some() {
        return Err("translate after unmap still maps");
    }
    if space.unmap(TEST_VIRT + PAGE_SIZE).is_ok() {
        return Err("double unmap succeeded");
    }

    space
        .map(TEST_VIRT + PAGE_SIZE, frames[1], PRESENT | WRITABLE)
        .map_err(|_| "remap failed")?;

    drop(space); // 释放全部页表与映射页

    if frame::stats().free_frames != before {
        return Err("frame accounting mismatch after drop");
    }
    Ok(())
}
