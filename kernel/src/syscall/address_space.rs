//! 用户地址空间（v0.6 建立，v0.7 扩展为「按段权限逐页映射」）
//!
//! 目标：让用户程序运行在一棵**独立页表**下，且内核页面对 ring3 不可见。
//!
//! 构造方式（全部在**内核** CR3 下完成，最后一步才切 CR3）：
//!
//! 1. 新建一张 PML4，把当前（内核）PML4 的 512 项**逐项拷贝**过来 ——
//!    内核代码/数据/栈/页表因此在新地址空间里照样可访问，切 CR3 不会崩；
//! 2. 递归清理拷贝过来的内核子树中**被置位**的 `USER` 位（[`scrub_user_bits`]）
//!    —— 固件若给内核页置了 `USER=1`，不清理则 ring3 可读写内核内存，
//!    隔离形同虚设（ring3 要求**每一级**都置 `USER` 才能到达叶子页）；
//! 3. 在**空闲的 PML4 槽位**（下标 1，即 0x0000_0080_0000_0000）新建
//!    PDPT/PD/PT，并在**每一级**都置 `USER`。由于映像窗口、栈都在该 PT 覆盖的
//!    2 MiB 内，本模块只需要一张 PT；
//! 4. 用户栈固定映射（4 页，`RW` 且 `NX`）；用户映像页由 ELF Loader 按段权限
//!    逐页映射（[`UserSpace::map_image_page`]）。
//!
//! 为什么不复用低地址空间（例如 0x4000_0000）：内核运行在固件建立的恒等
//! 映射上（PML4 下标 0 覆盖低 512 GiB），该子树与内核共享；在其中插用户页
//! 会同时改动内核页表。改用一个空槽位后，用户子树完全归本模块所有，
//! `Drop` 时能整棵释放而不误伤内核。
//!
//! 一个**实测**得到的注意点：固件（OVMF/EDK2）把自己建立的页表页标记为**只读**
//! （EDK2 的 EnablePageTableProtection）。因此“把整棵内核子树无条件重写一遍”
//! 会立即 #PF（v0.6 实测：写 0x1fc03000 处的 PDPT 项，error=P|W）。
//! 所以 [`scrub_user_bits`] 是**惰性**的：只在 `USER` 确实为 1 时才写回。
//!
//! ## 用户虚拟地址布局（固定，不随程序大小漂移）
//!
//! ```text
//! 0x0000_0080_0000_0000 + 0x0000  页 0..15   用户映像窗口（64 KiB）
//!                                  └── ELF PT_LOAD 落在这里，按段权限逐页映射
//! 0x0000_0080_0000_0000 + 0x10000 页 16..19  用户栈（16 KiB，RW、NX）
//! 0x0000_0080_0000_0000 + 0x20000 页 32..95  brk 堆窗口（256 KiB，按需映射，RW、NX）
//! 0x0000_0080_0000_0000 + 0x60000 页 96..159 匿名 mmap 窗口（256 KiB，first-fit，RW 或 RX）
//! 0x0000_0080_0000_0000 + 0x100000 页 256..287 共享库窗口（128 KiB，v0.9 动态链接）
//! ```
//!
//! 映像窗口、栈、堆、mmap 窗口、共享库窗口都落在同一张 PT 覆盖的 2 MiB 内（共 288 页，占 512 项里的
//! 一小部分），因此本模块仍然只需要一张 PT。**中间没被映射的页（例如页 20..31）摸到就 #PF**
//! —— 这是隔离的正常表现，v0.6 的探针测试仍然有效。
//!
//! ## 用户堆（v0.9 运行时前置条件第一步）
//!
//! 两个窗口都是**按需映射**（首次触碰前不存在页帧，`page_flags` 返回 `None` ⇒
//! syscall 层的缓冲校验会拒绝，不会让内核去写一段不存在的内存）：
//!
//! - **brk 堆窗口**：`brk(addr)` 增长/收缩堆顶，`addr == 0` 时只查询当前堆顶
//!   （Linux 语义：**不返回错误**，失败时返回原值 ⇒ 调用方用 `ret < addr` 判断）；
//! - **匿名 mmap 窗口**：`mmap` 在窗口内做 first-fit 连续分配、`munmap` 整段归还；
//!   只支持 `MAP_PRIVATE|MAP_ANONYMOUS` 的匿名页，且拒绝 `PROT_WRITE|PROT_EXEC`（W^X）。
//!
//! 三条只有实测才能确认的性质，验收脚本就断言它们：
//!
//! 1. 新映射的页**一定是零**（`mmap` 语义：匿名页不得泄漏上一任内容）；
//! 2. `munmap`/收缩之后访问该页会 #PF（页表项被清掉 **且** `invlpg` 刷新了 TLB ——
//!    少了刷新就会出现「页帧已归还、TLB 里还是旧映射」的幽灵访问）；
//! 3. 用户态结束后**帧全部归还**（`Drop` 释放堆/mmap 页；`user.rs` 用帧分配器的
//!    `free_frames` 前后对比做泄漏检查）。
//!
//! 窗口大小是**刻意写死**的（各 256 KiB）：v0.9 只需要支撑「有堆可用」这件事，
//! 更大的地址空间留给后续「按需新建 PT」的扩展（那时窗口大小会变成运行期参数）。
//!
//! ## `mprotect`（v1.0 第一步：运行时改保护属性）
//!
//! `mprotect(addr, len, prot)` 改写**已经映射**的页的权限位（[`UserSpace::mprotect`]）：
//!
//! - 范围必须**页对齐**、非空、且完整落在**同一个窗口**里（映像 / brk 堆 / 匿名 mmap /
//!   共享库）；跨窗口或越界 ⇒ `EINVAL`，不做「部分成功」；
//! - `W^X` 仍然在 syscall 层校验：`PROT_WRITE|PROT_EXEC` 一律 `EINVAL`；
//! - 每一页都必须已映射（堆/mmap 里没碰过的页不存在）⇒ 缺一即 `EINVAL`
//!   （与 `munmap` 的「整段已映射」同一约定，而不是 Linux 的 `ENOMEM`）；
//! - `PROT_NONE` 的表达方式是**清 `USER` 位而保留 `PRESENT`**：ring3 访问立刻 #PF
//!   （`error=P|U`），页帧与内容都还在，`mprotect` 回来即可用；内核侧仍然可访问，
//!   但 [`UserSpace::page_flags`] 把没有 `USER` 的页当作「不是用户缓冲」拒掉
//!   （所以拿一段 `PROT_NONE` 内存去 `write` 是 `-EFAULT`）；
//! - 用户栈（固定 4 页 `RW` + `NX`）**不在支持范围**：它的保护属性由内核在 `new()`
//!   里一次性定死，`mprotect` 碰它返回 `EINVAL`（串口有记录，不静默忽略）。
//!
//! 已知限制（写在文档里而不是藏着）：拷贝的是 PML4 项的**快照**。若内核在用户态
//! 运行期间新增整个 PML4 槽位级的映射（如堆跨到新 PDPT），该映射不会出现在用户
//! 页表中。用户态路径只做串口输出与 tick 等待，不触发此类增长。

use crate::arch::x86_64::paging::{
    self, entry_address, table_index, write_entry, HUGE, NO_EXEC, PRESENT, USER, WRITABLE,
};
use crate::fs::vfs::{EINVAL, ENOMEM};
use crate::memory::frame::{self, PhysFrame};
use crate::memory::page::PAGE_SIZE;

/// 用户虚拟地址基址：PML4 下标 1（内核恒等映射只用到下标 0）。
///
/// 0x0000_0080_0000_0000 = 512 GiB，位于规范低半区（< 0x0000_8000_0000_0000）。
pub const USER_VIRT_BASE: u64 = 0x0000_0080_0000_0000;

/// 用户映像窗口页数上限（16 页 = 64 KiB）。
///
/// `user/hello`（libc + `core::fmt` + 页对齐的 4 个段）实测 4 页；
/// 超过上限的 ELF 会在装载时被 [`UserSpace::load_elf`] 拒绝（有明确 errno）。
pub const USER_IMAGE_MAX_PAGES: usize = 16;

/// 用户映像窗口字节上限。
pub const USER_IMAGE_MAX: u64 = USER_IMAGE_MAX_PAGES as u64 * PAGE_SIZE;

/// 用户栈起始虚拟地址：固定排在映像窗口之后（与程序实际大小无关）。
pub const USER_STACK_VIRT: u64 = USER_VIRT_BASE + USER_IMAGE_MAX;
/// 用户栈页数（16 KiB，向下增长）。
pub const USER_STACK_PAGES: usize = 4;
/// 用户栈顶（从最高地址开始向下增长）。
pub const USER_STACK_TOP: u64 = USER_STACK_VIRT + (USER_STACK_PAGES as u64) * PAGE_SIZE;

/// brk 堆窗口起始虚拟地址：固定排在用户栈之后（页号 32）。
pub const USER_HEAP_VIRT: u64 = USER_VIRT_BASE + 0x0002_0000;
/// brk 堆窗口页数（64 页 = 256 KiB）。
pub const USER_HEAP_PAGES: usize = 64;
/// brk 堆窗口字节上限。
pub const USER_HEAP_MAX: u64 = USER_HEAP_PAGES as u64 * PAGE_SIZE;

/// 匿名 mmap 窗口起始虚拟地址（堆窗口正上方，页号 96）。
pub const USER_MMAP_VIRT: u64 = USER_HEAP_VIRT + USER_HEAP_MAX;
/// 匿名 mmap 窗口页数（64 页 = 256 KiB）。
pub const USER_MMAP_PAGES: usize = 64;
/// 匿名 mmap 窗口字节上限。
pub const USER_MMAP_MAX: u64 = USER_MMAP_PAGES as u64 * PAGE_SIZE;

/// 共享库窗口起始虚拟地址（v0.9 动态链接；页号 256，= 基址 + 1 MiB）。
///
/// `PT_INTERP` 指向的解释器与每个 `DT_NEEDED` 共享库都装在这个窗口里，
/// 而不是映像窗口：主程序（ET_DYN PIE）仍固定落在 [`USER_VIRT_BASE`]，
/// 共享对象则依次串排在窗口里（见 `syscall/dynamic.rs`）。
pub const USER_DYN_BASE: u64 = USER_VIRT_BASE + 0x0010_0000;
/// 共享库窗口页数（32 页 = 128 KiB）。最小闭环里 = 解释器 + 一个库，绰绰有余。
pub const USER_DYN_PAGES: usize = 32;
/// 共享库窗口字节上限。
pub const USER_DYN_MAX: u64 = USER_DYN_PAGES as u64 * PAGE_SIZE;

/// `PROT_*`：用户内存保护位（与 Linux 一致）—— **再导出**，位值定义在 `abi::flags`
/// （阶段三的单一事实源：内核与 libc 引用同一处，不再“各写一份 + 注释提醒”）。
///
/// 本模块只负责把位翻译成页表标志（[`prot_to_flags`]），ABI 层的合法性校验
/// （未知位、`W^X`）在 `syscall/memory.rs`。
pub use abi::flags::{PROT_EXEC, PROT_MASK, PROT_READ, PROT_WRITE};

/// 本地址空间在 `new()` 期间拥有的帧数量：PML4 + PDPT + PD + PT + 栈页。
///
/// 映像页不在其中：它们在 `map_image_page` 里按 ELF 段按需分配。
const OWNED_TABLES: usize = 4;
const OWNED_FRAMES: usize = OWNED_TABLES + USER_STACK_PAGES;

/// ELF 装载结果（串口日志与自测的断言来源）。
#[derive(Clone, Copy, Debug)]
pub struct LoadStats {
    /// 程序入口虚拟地址（来自 ELF 头）
    pub entry: u64,
    /// 已装载的 PT_LOAD 段数
    pub segments: usize,
    /// 映像字节数（各段 `p_memsz` 之和，含 `.bss`）
    pub bytes: u64,
    /// 占用页数
    pub pages: usize,
    /// 其中可写页数（ELF 的 `RW` 段；0 说明程序没有可写数据）
    pub writable_pages: usize,
}

/// 用户地址空间：一整套可由 CR3 切换的页表 + 用户页帧的所有权。
pub struct UserSpace {
    /// PML4（本模块拥有）
    root: u64,
    /// 用户子树：PDPT / PD / PT（本模块拥有）
    pdpt: u64,
    pd: u64,
    pt: u64,
    /// 已装入的用户映像页帧（由 `map_image_page` 填充前 `image_pages` 个槽位）
    image: [Option<PhysFrame>; USER_IMAGE_MAX_PAGES],
    /// 每页的映射标志（`PRESENT` 位为 0 表示该页未映射），用于 syscall 缓冲校验
    image_flags: [u64; USER_IMAGE_MAX_PAGES],
    /// 已映射的映像页数（= 最高已用页 + 1）
    image_pages: usize,
    /// 用户栈页帧
    stack: [PhysFrame; USER_STACK_PAGES],
    /// brk 堆窗口页帧（`None` = 未映射；下标 = 页序号）
    heap: [Option<PhysFrame>; USER_HEAP_PAGES],
    /// 每个堆页的映射标志（`mprotect` 之后堆页未必还是 `RW`；`USER` 位为 0 = ring3 不可访问）
    heap_flags: [u64; USER_HEAP_PAGES],
    /// 当前 break（页对齐，恒在 `[USER_HEAP_VIRT, USER_HEAP_VIRT + USER_HEAP_MAX]` 内）
    brk: u64,
    /// 匿名 mmap 窗口页帧（first-fit 分配，`None` = 空闲）
    mmap: [Option<PhysFrame>; USER_MMAP_PAGES],
    /// 每个 mmap 页的映射标志（`PRESENT` 位为 0 表示该页空闲），用于缓冲校验
    mmap_flags: [u64; USER_MMAP_PAGES],
    /// 共享库窗口页帧（v0.9 动态链接；`None` = 未映射）
    dyn_pages: [Option<PhysFrame>; USER_DYN_PAGES],
    /// 每个共享库页的映射标志（`PRESENT` 位为 0 表示未映射），用于缓冲校验
    dyn_flags: [u64; USER_DYN_PAGES],
}

impl UserSpace {
    /// 新建用户地址空间（尚未装入程序）。
    ///
    /// 任何一步失败都会回收已分配的全部帧，不留泄漏（帧分配器统计因此可还原）。
    pub fn new() -> Result<Self, i64> {
        // 1. 一次性把需要的帧全部取出来，失败即整体回滚
        let mut owned: [Option<PhysFrame>; OWNED_FRAMES] = [None; OWNED_FRAMES];
        for slot in owned.iter_mut() {
            match frame::allocate() {
                Some(frame) => *slot = Some(frame),
                None => {
                    for frame in owned.iter().flatten() {
                        frame::free(*frame);
                    }
                    return Err(ENOMEM);
                }
            }
        }
        let take = |i: usize| -> PhysFrame { owned[i].expect("frame slot filled") };
        let root = take(0).start_address();
        let pdpt = take(1).start_address();
        let pd = take(2).start_address();
        let pt = take(3).start_address();
        let mut stack = [PhysFrame::from_number(0); USER_STACK_PAGES];
        for (i, slot) in stack.iter_mut().enumerate() {
            *slot = take(OWNED_TABLES + i);
        }

        // 2. 清零本模块拥有的全部页表帧
        for table in [root, pdpt, pd, pt] {
            unsafe { core::ptr::write_bytes(table as *mut u8, 0, PAGE_SIZE as usize) };
        }

        // 3. 拷贝内核 PML4（内核在新地址空间中依然可见），再清内核子树的 USER 位
        let kernel_root = paging::current_root();
        unsafe {
            for i in 0..paging::ENTRY_COUNT {
                write_entry(root, i, paging::read_entry(kernel_root, i));
            }
        }
        let user_slot = table_index(USER_VIRT_BASE, 3);
        let mut cleared = 0u64;
        unsafe {
            for i in 0..paging::ENTRY_COUNT {
                if i == user_slot {
                    continue;
                }
                let entry = paging::read_entry(root, i);
                if entry & PRESENT == 0 {
                    continue;
                }
                if entry & HUGE == 0 {
                    scrub_user_bits(entry_address(entry), 2, &mut cleared);
                }
                if entry & USER != 0 {
                    write_entry(root, i, entry & !USER);
                    cleared += 1;
                }
            }
        }
        if cleared > 0 {
            crate::driver::serial::print("user: kernel map USER bits cleared=");
            crate::driver::serial::print_dec(cleared);
            crate::driver::serial::println("");
        }

        // 4. 构建用户子树（每一级都要 USER，任一缺失 ring3 就访问不到叶子页）
        unsafe {
            write_entry(root, user_slot, pdpt | PRESENT | WRITABLE | USER);
            write_entry(pdpt, table_index(USER_VIRT_BASE, 2), pd | PRESENT | WRITABLE | USER);
            write_entry(pd, table_index(USER_VIRT_BASE, 1), pt | PRESENT | WRITABLE | USER);
        }

        // 5. 叶子映射：栈页可写（不可执行）。
        //    映像页由 `map_image_page` 按 ELF 段权限逐页映射。
        unsafe {
            for (i, frame) in stack.iter().enumerate() {
                let virt = USER_STACK_VIRT + i as u64 * PAGE_SIZE;
                write_entry(
                    pt,
                    table_index(virt, 0),
                    frame.start_address() | PRESENT | WRITABLE | USER | NO_EXEC,
                );
            }
        }

        Ok(Self {
            root,
            pdpt,
            pd,
            pt,
            image: [None; USER_IMAGE_MAX_PAGES],
            image_flags: [0; USER_IMAGE_MAX_PAGES],
            image_pages: 0,
            stack,
            heap: [None; USER_HEAP_PAGES],
            heap_flags: [0; USER_HEAP_PAGES],
            brk: USER_HEAP_VIRT,
            mmap: [None; USER_MMAP_PAGES],
            mmap_flags: [0; USER_MMAP_PAGES],
            dyn_pages: [None; USER_DYN_PAGES],
            dyn_flags: [0; USER_DYN_PAGES],
        })
    }

    /// 页表根（写入 CR3）。
    pub fn root(&self) -> u64 {
        self.root
    }

    /// 已映射的映像页数。
    pub fn image_pages(&self) -> usize {
        self.image_pages
    }

    /// 装入 ELF 程序（解析 + 按段权限映射 + 拷贝段数据）。
    ///
    /// 实现在 [`crate::syscall::elf`]：本函数只做转发，让「页表/帧的所有权」
    /// 留在本模块，「ELF 格式解析」独立成模块（便于以后加 `elf::validate` 之类
    /// 的纯解析接口）。
    pub fn load_elf(&mut self, elf: &[u8]) -> Result<LoadStats, i64> {
        crate::syscall::elf::load(self, elf)
    }

    /// 映射映像窗口的第 `index` 页并返回其物理地址（幂等）。
    ///
    /// - 首次调用：分配帧、清零、写入 PT（`PRESENT|USER|flags`）；
    /// - 再次调用（同一页被多个 ELF 段覆盖，例如 `R` 段与 `RW` 段落在同一页）：
    ///   **只合并权限位**，不重新分配、不清零（否则会擦掉先拷进去的段数据）。
    pub fn map_image_page(&mut self, index: usize, flags: u64) -> Result<u64, i64> {
        if index >= USER_IMAGE_MAX_PAGES {
            return Err(EINVAL);
        }
        let merged = flags | PRESENT | USER;
        if let Some(frame) = self.image[index] {
            self.image_flags[index] |= merged;
            let entry = frame.start_address() | self.image_flags[index];
            unsafe { write_entry(self.pt, table_index(USER_VIRT_BASE + index as u64 * PAGE_SIZE, 0), entry) };
            return Ok(frame.start_address());
        }

        let frame = frame::allocate().ok_or(ENOMEM)?;
        // SAFETY: 该帧由本空间拥有，且内核 CR3 下其物理地址恒等可写
        unsafe { core::ptr::write_bytes(frame.start_address() as *mut u8, 0, PAGE_SIZE as usize) };
        self.image[index] = Some(frame);
        self.image_flags[index] = merged;
        self.image_pages = self.image_pages.max(index + 1);
        unsafe {
            write_entry(
                self.pt,
                table_index(USER_VIRT_BASE + index as u64 * PAGE_SIZE, 0),
                frame.start_address() | merged,
            )
        };
        Ok(frame.start_address())
    }

    /// 在共享库窗口映射第 `index` 页并返回其物理地址（幂等，语义同
    /// [`UserSpace::map_image_page`]）。动态链接器用它把 `PT_INTERP` 解释器与
    /// `DT_NEEDED` 共享库一页页放进来。
    pub fn map_dyn_page(&mut self, index: usize, flags: u64) -> Result<u64, i64> {
        if index >= USER_DYN_PAGES {
            return Err(EINVAL);
        }
        let merged = flags | PRESENT | USER;
        if let Some(frame) = self.dyn_pages[index] {
            self.dyn_flags[index] |= merged;
            let entry = frame.start_address() | self.dyn_flags[index];
            unsafe {
                write_entry(
                    self.pt,
                    table_index(USER_DYN_BASE + index as u64 * PAGE_SIZE, 0),
                    entry,
                )
            };
            return Ok(frame.start_address());
        }
        let frame = frame::allocate().ok_or(ENOMEM)?;
        // SAFETY: 该帧由本空间拥有，且内核 CR3 下其物理地址恒等可写
        unsafe { core::ptr::write_bytes(frame.start_address() as *mut u8, 0, PAGE_SIZE as usize) };
        self.dyn_pages[index] = Some(frame);
        self.dyn_flags[index] = merged;
        unsafe {
            write_entry(
                self.pt,
                table_index(USER_DYN_BASE + index as u64 * PAGE_SIZE, 0),
                frame.start_address() | merged,
            )
        };
        Ok(frame.start_address())
    }

    /// 已映射的共享库窗口页数（统计/日志用）。
    pub fn dyn_page_count(&self) -> usize {
        self.dyn_pages.iter().filter(|slot| slot.is_some()).count()
    }

    /// 用户虚拟地址 → 物理地址（内核 CR3 下写重定位/拷贝段数据用）。
    ///
    /// 只覆盖映像窗口与共享库窗口：v0.7/v0.9 动态链接需要写的就是这两处
    /// （段数据、`.rela` 重定位目标）。未映射返回 `None`（绝不返回一个假地址）。
    pub fn phys_of(&self, virt: u64) -> Option<u64> {
        let in_page = virt & (PAGE_SIZE - 1);
        if (USER_VIRT_BASE..USER_VIRT_BASE + USER_IMAGE_MAX).contains(&virt) {
            let index = ((virt - USER_VIRT_BASE) / PAGE_SIZE) as usize;
            return self.image[index].map(|frame| frame.start_address() + in_page);
        }
        if (USER_DYN_BASE..USER_DYN_BASE + USER_DYN_MAX).contains(&virt) {
            let index = ((virt - USER_DYN_BASE) / PAGE_SIZE) as usize;
            return self.dyn_pages[index].map(|frame| frame.start_address() + in_page);
        }
        None
    }

    /// 映射一段**原始机器码**（内嵌隔离探针用；位置无关、单页即可）。
    ///
    /// 这不是「装载程序」的常规路径：v0.7 的用户程序一律经 ELF Loader
    /// （[`UserSpace::load_elf`]）。保留它是因为隔离自测需要一段**必然越权**的
    /// 代码，而把它写成 ELF 只会让探针依赖被测的 ELF Loader。
    pub fn map_raw(&mut self, code: &[u8]) -> Result<u64, i64> {
        if code.is_empty() || code.len() as u64 > USER_IMAGE_MAX {
            return Err(EINVAL);
        }
        for (i, chunk) in code.chunks(PAGE_SIZE as usize).enumerate() {
            let phys = self.map_image_page(i, 0)?; // 可读可执行
            unsafe {
                core::ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    phys as *mut u8,
                    chunk.len(),
                );
            }
        }
        Ok(USER_VIRT_BASE)
    }

    /// `fork` 的地址空间复制：新建一棵**同布局**的页表，并把已映射的用户页逐页复制。
    ///
    /// 复制的是**内容**而不是页表项：父子进程此后各有自己的页帧（写时复制属后续版本
    /// 的优化，当前语义是「立即复制」，正确性上更强）。任何一步失败时返回 `Err`，
    /// 此时新地址空间作为一个局部量被 `Drop`，已分配的帧全部归还（`fork` 失败不留泄漏）。
    ///
    /// 复制的对象与「用户态能观察到什么」一一对应：
    /// - 映像页（代码/`RW` 段 + 每页的权限位，`W^X` 因此保持）；
    /// - 用户栈（子进程恢复执行时用的还是同一段栈内容）；
    /// - brk 堆窗口里已映射的页 + 当前 break；
    /// - 匿名 mmap 窗口里已映射的页 + 每页权限位。
    pub fn clone_for_fork(&self) -> Result<Self, i64> {
        let mut child = Self::new()?;

        // 1. 映像页：`map_image_page` 会分配 + 清零 + 写 PT，然后按段权限写入权限位
        for i in 0..self.image_pages {
            let Some(src) = self.image[i] else {
                continue;
            };
            let flags = self.image_flags[i];
            let dst = child.map_image_page(i, flags)?;
            unsafe { copy_page(src.start_address(), dst) };
            // `map_image_page` 会把 `PRESENT|USER` 并进调用方给的标志位。父进程若用
            // `mprotect(PROT_NONE)` 把这一页关掉（`USER` 已清），子进程必须保持同样的
            // 不可访问状态 —— 否则 fork 会把保护属性悄悄「升权」。
            if flags & USER == 0 {
                child.set_page_flags(UserWindow::Image, i, flags)?;
            }
        }

        // 2. 用户栈（`new()` 已经映射好，且父子页帧一一对应）
        for i in 0..USER_STACK_PAGES {
            unsafe { copy_page(self.stack[i].start_address(), child.stack[i].start_address()) };
        }

        // 3. brk 堆：只复制**已映射**的页，并沿用当前 break
        let pt = child.pt;
        for i in 0..USER_HEAP_PAGES {
            let Some(src) = self.heap[i] else {
                continue;
            };
            let virt = USER_HEAP_VIRT + i as u64 * PAGE_SIZE;
            // 标志位沿用父进程的（`mprotect` 可能把某个堆页改成非 `RW`）
            let flags = self.heap_flags[i];
            map_new_page(pt, &mut child.heap[i], virt, flags)?;
            child.heap_flags[i] = flags;
            let dst = child.heap[i].expect("map_new_page 已填槽位").start_address();
            unsafe { copy_page(src.start_address(), dst) };
        }
        child.brk = self.brk;

        // 4. 匿名 mmap 窗口：同样只复制已映射的页 + 每页权限
        for i in 0..USER_MMAP_PAGES {
            let Some(src) = self.mmap[i] else {
                continue;
            };
            let virt = USER_MMAP_VIRT + i as u64 * PAGE_SIZE;
            let flags = self.mmap_flags[i];
            map_new_page(pt, &mut child.mmap[i], virt, flags)?;
            child.mmap_flags[i] = flags;
            let dst = child.mmap[i].expect("map_new_page 已填槽位").start_address();
            unsafe { copy_page(src.start_address(), dst) };
        }

        // 5. 共享库窗口（动态链接的程序被 fork 时，子进程也要有同布局的库页）
        for i in 0..USER_DYN_PAGES {
            let Some(src) = self.dyn_pages[i] else {
                continue;
            };
            let virt = USER_DYN_BASE + i as u64 * PAGE_SIZE;
            let flags = self.dyn_flags[i];
            map_new_page(pt, &mut child.dyn_pages[i], virt, flags)?;
            child.dyn_flags[i] = flags;
            let dst = child.dyn_pages[i].expect("map_new_page 已填槽位").start_address();
            unsafe { copy_page(src.start_address(), dst) };
        }

        Ok(child)
    }

    /// 把 `data` 写进用户栈的虚拟地址 `virt`（必须整段落在这 4 个栈页内）。
    ///
    /// 用途：内核在**切换 CR3 之前**构建 `execve` 的 `argv`（字符串 + 指针数组）。
    /// 因为此时内核还跑在内核 CR3 下，所以这里按栈页帧的**物理地址**写入
    /// （内核恒等映射物理内存），而不需要先切页表。
    ///
    /// # Panics
    /// `virt..virt+data.len()` 越出栈窗口会 panic：调用方（syscall 层）在调用前
    /// 已按自己的上限（[`crate::syscall::process::ARG_MAX_BYTES`]）算过布局。
    pub fn stack_write(&self, virt: u64, data: &[u8]) {
        let mut done = 0usize;
        while done < data.len() {
            let v = virt + done as u64;
            let offset = v - USER_STACK_VIRT;
            let index = (offset / PAGE_SIZE) as usize;
            let in_page = (offset % PAGE_SIZE) as usize;
            assert!(index < USER_STACK_PAGES, "stack_write 越出用户栈窗口");
            let frame = self.stack[index];
            let room = PAGE_SIZE as usize - in_page;
            let n = room.min(data.len() - done);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(done),
                    (frame.start_address() + in_page as u64) as *mut u8,
                    n,
                )
            };
            done += n;
        }
    }

    /// 把 `data` 写进 **mmap 窗口**的虚拟地址 `virt`（文件映射装载时用）。
    ///
    /// 与 [`UserSpace::stack_write`] 同一套做法：此时内核还跑在自己的 CR3 下，
    /// 因此按页帧的**物理地址**写入（内核恒等映射物理内存），不需要先切页表。
    /// `virt` 必须落在已映射的 mmap 页里（调用方先 `mmap_anon` 再调用）。
    ///
    /// # Panics
    /// 目标页未映射时 panic：这是内核内部契约，调用顺序错了应当立刻显形。
    pub fn mmap_write(&self, virt: u64, data: &[u8]) {
        let mut done = 0usize;
        while done < data.len() {
            let v = virt + done as u64;
            let offset = v - USER_MMAP_VIRT;
            let index = (offset / PAGE_SIZE) as usize;
            let in_page = (offset % PAGE_SIZE) as usize;
            assert!(index < USER_MMAP_PAGES, "mmap_write 越出 mmap 窗口");
            let frame = self.mmap[index].expect("mmap_write 目标页未映射");
            let room = PAGE_SIZE as usize - in_page;
            let n = room.min(data.len() - done);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(done),
                    (frame.start_address() + in_page as u64) as *mut u8,
                    n,
                )
            };
            done += n;
        }
    }

    /// 当前 break（页对齐）。`brk(0)` 的返回值。
    pub fn brk_value(&self) -> u64 {
        self.brk
    }

    /// 系统调用 `brk(addr)`：增长/收缩用户堆。
    ///
    /// **Linux 语义：不返回错误。** 成功返回页对齐后的新 break（`>= addr`），
    /// 失败（越出堆窗口 / 帧分配失败）返回**原值**，调用方用 `ret < addr` 判断。
    /// 增长是「全成或全不成」：任一页失败就回滚本次新增的页，不留半个堆。
    pub fn brk(&mut self, addr: u64) -> u64 {
        if addr == 0 {
            return self.brk;
        }
        // 向上取整到页；溢出（addr 已经贴着 u64::MAX）直接当作越界
        let want = match addr.checked_add(PAGE_SIZE - 1) {
            Some(v) => v & !(PAGE_SIZE - 1),
            None => return self.brk,
        };
        if want < USER_HEAP_VIRT || want > USER_HEAP_VIRT + USER_HEAP_MAX {
            return self.brk;
        }
        let want_pages = ((want - USER_HEAP_VIRT) / PAGE_SIZE) as usize;
        let have_pages = ((self.brk - USER_HEAP_VIRT) / PAGE_SIZE) as usize;
        if want_pages > have_pages {
            let mut mapped = 0usize;
            for i in have_pages..want_pages {
                let virt = USER_HEAP_VIRT + i as u64 * PAGE_SIZE;
                match map_new_page(self.pt, &mut self.heap[i], virt, USER_RW_FLAGS) {
                    Ok(()) => {
                        self.heap_flags[i] = USER_RW_FLAGS;
                        mapped += 1;
                    }
                    Err(_) => {
                        for j in have_pages..have_pages + mapped {
                            unmap_page(self.pt, &mut self.heap[j], USER_HEAP_VIRT + j as u64 * PAGE_SIZE);
                            self.heap_flags[j] = 0;
                        }
                        return self.brk;
                    }
                }
            }
        } else if want_pages < have_pages {
            for i in want_pages..have_pages {
                unmap_page(self.pt, &mut self.heap[i], USER_HEAP_VIRT + i as u64 * PAGE_SIZE);
                self.heap_flags[i] = 0;
            }
        }
        self.brk = want;
        want
    }

    /// 匿名映射（`mmap`）：在 mmap 窗口里 first-fit 找连续 `len` 字节。
    ///
    /// 只接受 `MAP_PRIVATE|MAP_ANONYMOUS`（调用方在 syscall 层校验 flags，并已把
    /// `prot` 的合法性、`W^X` 检查做完）。新页**必须清零**：匿名映射不得把上一任
    /// 使用者的内容送给新调用方。`PROT_NONE`（`prot == 0`）是合法的：映射存在、
    /// 帧已分配，但对 ring3 不可访问（清 `USER`），可由 `mprotect` 再启用。
    pub fn mmap_anon(&mut self, len: u64, prot: u64) -> Result<u64, i64> {
        if len == 0 {
            return Err(EINVAL);
        }
        let bytes = match len.checked_add(PAGE_SIZE - 1) {
            Some(v) => v & !(PAGE_SIZE - 1),
            None => return Err(ENOMEM),
        };
        if bytes > USER_MMAP_MAX {
            return Err(ENOMEM);
        }
        let pages = (bytes / PAGE_SIZE) as usize;
        let start = match self.find_free_run(pages) {
            Some(start) => start,
            None => return Err(ENOMEM),
        };
        let flags = prot_to_flags(prot);
        let mut mapped = 0usize;
        for i in start..start + pages {
            let virt = USER_MMAP_VIRT + i as u64 * PAGE_SIZE;
            match map_new_page(self.pt, &mut self.mmap[i], virt, flags) {
                Ok(()) => {
                    self.mmap_flags[i] = flags;
                    mapped += 1;
                }
                Err(err) => {
                    for j in start..start + mapped {
                        unmap_page(self.pt, &mut self.mmap[j], USER_MMAP_VIRT + j as u64 * PAGE_SIZE);
                        self.mmap_flags[j] = 0;
                    }
                    return Err(err);
                }
            }
        }
        Ok(USER_MMAP_VIRT + start as u64 * PAGE_SIZE)
    }

    /// 取消一段匿名映射（`munmap`）。
    ///
    /// 要求整段页对齐、落在 mmap 窗口内、且**每一页都已映射** ——
    /// 不做「部分取消」这种半成品语义（要么整段归还，要么 `EINVAL`）。
    pub fn munmap_anon(&mut self, addr: u64, len: u64) -> Result<(), i64> {
        if len == 0 || addr % PAGE_SIZE != 0 {
            return Err(EINVAL);
        }
        let bytes = match len.checked_add(PAGE_SIZE - 1) {
            Some(v) => v & !(PAGE_SIZE - 1),
            None => return Err(EINVAL),
        };
        let end = match addr.checked_add(bytes) {
            Some(end) => end,
            None => return Err(EINVAL),
        };
        if addr < USER_MMAP_VIRT || end > USER_MMAP_VIRT + USER_MMAP_MAX {
            return Err(EINVAL);
        }
        let start = ((addr - USER_MMAP_VIRT) / PAGE_SIZE) as usize;
        let pages = (bytes / PAGE_SIZE) as usize;
        for i in start..start + pages {
            if self.mmap[i].is_none() {
                return Err(EINVAL);
            }
        }
        for i in start..start + pages {
            unmap_page(self.pt, &mut self.mmap[i], USER_MMAP_VIRT + i as u64 * PAGE_SIZE);
            self.mmap_flags[i] = 0;
        }
        Ok(())
    }

    /// 系统调用 `mprotect(addr, len, prot)`：改写**已映射**页的权限位，返回页数。
    ///
    /// 全部校验通过才动手（**没有部分成功**）：页对齐、非空、`addr` 落在支持的窗口里、
    /// 范围不跨窗口、每一页都已映射。缺一即 `EINVAL`（与 [`UserSpace::munmap_anon`]
    /// 的「整段已映射」同一约定；Linux 这里用 `ENOMEM`）。
    ///
    /// 页帧不动：`mprotect` 不搬家也不清零，`PROT_NONE` → `RW` 回来内容原样还在。
    pub fn mprotect(&mut self, addr: u64, len: u64, prot: u64) -> Result<usize, i64> {
        if len == 0 || addr % PAGE_SIZE != 0 {
            return Err(EINVAL);
        }
        let bytes = match len.checked_add(PAGE_SIZE - 1) {
            Some(v) => v & !(PAGE_SIZE - 1),
            None => return Err(EINVAL),
        };
        let end = match addr.checked_add(bytes) {
            Some(end) => end,
            None => return Err(EINVAL),
        };
        let window = match UserWindow::of(addr) {
            Some(window) => window,
            None => return Err(EINVAL),
        };
        if end > window.base() + window.pages() as u64 * PAGE_SIZE {
            return Err(EINVAL); // 跨窗口/越界：不做半段
        }
        let first = ((addr - window.base()) / PAGE_SIZE) as usize;
        let pages = (bytes / PAGE_SIZE) as usize;
        for i in first..first + pages {
            // 「已映射」= `PRESENT` 已置（`PROT_NONE` 只清 `USER`，因此仍算已映射，
            // 可以从 `PROT_NONE` 改回 `RW`/`RX`）
            if !self.window_mapped(window, i) {
                return Err(EINVAL);
            }
        }
        let flags = prot_to_flags(prot);
        for i in first..first + pages {
            self.set_page_flags(window, i, flags)?;
        }
        Ok(pages)
    }

    /// 窗口里第 `index` 页是否已映射（以 `PRESENT` 位为准；`PROT_NONE` 的页仍算已映射）。
    fn window_mapped(&self, window: UserWindow, index: usize) -> bool {
        self.window_flags(window, index) & PRESENT != 0
    }

    /// 取窗口里第 `index` 页当前记录的权限位。
    fn window_flags(&self, window: UserWindow, index: usize) -> u64 {
        match window {
            UserWindow::Image => self.image_flags[index],
            UserWindow::Heap => self.heap_flags[index],
            UserWindow::Mmap => self.mmap_flags[index],
            UserWindow::Dyn => self.dyn_flags[index],
        }
    }

    /// 把窗口里第 `index` 页的权限位整页改写，并刷新 TLB。
    ///
    /// 页帧不变（只重写页表项与标志位记录）。调用方保证该页已映射；
    /// 槽位为空时返回 `EINVAL`（不静默当作成功）。
    fn set_page_flags(&mut self, window: UserWindow, index: usize, flags: u64) -> Result<(), i64> {
        let virt = window.base() + index as u64 * PAGE_SIZE;
        let frame = match window {
            UserWindow::Image => {
                self.image_flags[index] = flags;
                self.image[index]
            }
            UserWindow::Heap => {
                self.heap_flags[index] = flags;
                self.heap[index]
            }
            UserWindow::Mmap => {
                self.mmap_flags[index] = flags;
                self.mmap[index]
            }
            UserWindow::Dyn => {
                self.dyn_flags[index] = flags;
                self.dyn_pages[index]
            }
        };
        let Some(frame) = frame else {
            return Err(EINVAL);
        };
        unsafe { write_entry(self.pt, table_index(virt, 0), frame.start_address() | flags) };
        // 改权限位必须刷 TLB：否则 ring3 可能继续用「旧的、更宽的权限」访问同一页
        // （与 `munmap`/`brk` 收缩的幽灵映射是同一个坑）。
        paging::flush_tlb(virt);
        Ok(())
    }

    /// 在 mmap 窗口里找连续 `pages` 个空闲槽位（first-fit）。
    fn find_free_run(&self, pages: usize) -> Option<usize> {
        if pages == 0 || pages > USER_MMAP_PAGES {
            return None;
        }
        let mut run = 0usize;
        for (i, slot) in self.mmap.iter().enumerate() {
            if slot.is_none() {
                run += 1;
                if run == pages {
                    return Some(i + 1 - pages);
                }
            } else {
                run = 0;
            }
        }
        None
    }

    /// 校验用户区间；`need_write` 为真时要求每一页都可写。
    ///
    /// syscall handler 用它把「用户传了越界指针」变成 `-EFAULT`，而不是让内核去
    /// 读写一段不属于该用户的名字空间。检查按**页**进行，因此「前半在映射里、
    /// 后半越界」的缓冲区也会被拒绝。
    pub fn check_range(&self, addr: u64, len: u64, need_write: bool) -> bool {
        if len == 0 {
            return true;
        }
        let end = match addr.checked_add(len) {
            Some(end) => end,
            None => return false,
        };
        let mut page = addr & !(PAGE_SIZE - 1);
        while page < end {
            let flags = match self.page_flags(page) {
                Some(flags) => flags,
                None => return false,
            };
            if need_write && flags & WRITABLE == 0 {
                return false;
            }
            page += PAGE_SIZE;
        }
        true
    }

    /// 校验用户区间（只读访问：`write`、`open` 的路径参数）。
    pub fn valid_user_range(&self, addr: u64, len: u64) -> bool {
        self.check_range(addr, len, false)
    }

    /// 校验用户区间（写访问：`read` 的接收缓冲）。
    pub fn valid_user_write_range(&self, addr: u64, len: u64) -> bool {
        self.check_range(addr, len, true)
    }

    /// 取某虚拟地址所在页的映射标志（ring3 不可访问或未映射时返回 `None`）。
    ///
    /// 这是 syscall 层缓冲校验的唯一依据：返回 `None` 的页会被当成越界指针拒掉
    /// （`-EFAULT`），而不是让内核去写不存在/不该碰的内容。三类页都是 `None`：
    ///
    /// 1. 从未映射的页（堆/mmap 窗口里**还没碰到**的页）；
    /// 2. `munmap`/`brk` 收缩之后归还了的页；
    /// 3. `mprotect(PROT_NONE)` 关掉的页（`PRESENT` 还在，但 `USER` 已清 ——
    ///    ring3 摸它会 #PF，内核也不把它当用户缓冲，见 [`accessible`]）。
    fn page_flags(&self, page: u64) -> Option<u64> {
        let stack_end = USER_STACK_TOP;
        if page >= USER_STACK_VIRT && page < stack_end {
            return accessible(PRESENT | WRITABLE | USER | NO_EXEC);
        }
        if page >= USER_HEAP_VIRT && page < USER_HEAP_VIRT + USER_HEAP_MAX {
            let index = ((page - USER_HEAP_VIRT) / PAGE_SIZE) as usize;
            return accessible(self.heap_flags[index]);
        }
        if page >= USER_MMAP_VIRT && page < USER_MMAP_VIRT + USER_MMAP_MAX {
            let index = ((page - USER_MMAP_VIRT) / PAGE_SIZE) as usize;
            return accessible(self.mmap_flags[index]);
        }
        // 共享库窗口（v0.9 动态链接）：库里的 .rodata/.text 必须能被 syscall 的
        // 缓冲校验认可（例如共享库自己发起的 `write`，缓冲区在库的只读段里）。
        if page >= USER_DYN_BASE && page < USER_DYN_BASE + USER_DYN_MAX {
            let index = ((page - USER_DYN_BASE) / PAGE_SIZE) as usize;
            return accessible(self.dyn_flags[index]);
        }
        if page < USER_VIRT_BASE {
            return None;
        }
        let index = (page - USER_VIRT_BASE) / PAGE_SIZE;
        if index as usize >= USER_IMAGE_MAX_PAGES {
            return None;
        }
        accessible(self.image_flags[index as usize])
    }
}

impl Drop for UserSpace {
    fn drop(&mut self) {
        // 用户映像页（只在映射过的槽位里有帧）
        for slot in self.image.iter() {
            if let Some(frame) = slot {
                frame::free(*frame);
            }
        }
        for frame in self.stack.iter() {
            frame::free(*frame);
        }
        // 堆与匿名 mmap 的页帧（未被 munmap 归还的部分必须在这里释放）
        for slot in self.heap.iter() {
            if let Some(frame) = slot {
                frame::free(*frame);
            }
        }
        for slot in self.mmap.iter() {
            if let Some(frame) = slot {
                frame::free(*frame);
            }
        }
        // 共享库窗口（v0.9 动态链接装载的解释器与 .so）
        for slot in self.dyn_pages.iter() {
            if let Some(frame) = slot {
                frame::free(*frame);
            }
        }
        // 本模块拥有的页表帧（内核子树是共享的，绝不能碰）
        paging::free_table(self.pt);
        paging::free_table(self.pd);
        paging::free_table(self.pdpt);
        paging::free_table(self.root);
    }
}

/// 复制一个页帧的内容（源与目标都由调用方保证是完整映射的 4 KiB 页）。
///
/// # Safety
/// `src`/`dst` 必须是有效且互不重叠的物理页地址；内核 CR3 下两者都可读写。
unsafe fn copy_page(src: u64, dst: u64) {
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, PAGE_SIZE as usize)
    };
}

/// 堆页与匿名 mmap 页的默认权限：可读可写、**不可执行**（NX）。
///
/// W^X 在这里是硬约束：只有 `mmap(PROT_EXEC)` 显式要求时才去掉 `NO_EXEC`，
/// 而 `PROT_WRITE|PROT_EXEC` 在 syscall 层就被拒（不存在既可写又可执行的自分配内存）。
/// 它同时是「`PROT_READ|PROT_WRITE` 的翻译结果」（[`prot_to_flags`]）。
const USER_RW_FLAGS: u64 = PRESENT | WRITABLE | USER | NO_EXEC;

/// 用户页属于哪个窗口。`mprotect` 只支持这 4 个窗口，且要求范围**不跨窗口**。
#[derive(Clone, Copy, PartialEq, Eq)]
enum UserWindow {
    /// 映像窗口（ELF 的代码/数据段）
    Image,
    /// brk 堆窗口
    Heap,
    /// 匿名 mmap 窗口
    Mmap,
    /// 共享库窗口（v0.9 动态链接的解释器与 `.so`）
    Dyn,
}

impl UserWindow {
    /// 窗口的起始虚拟地址。
    fn base(self) -> u64 {
        match self {
            Self::Image => USER_VIRT_BASE,
            Self::Heap => USER_HEAP_VIRT,
            Self::Mmap => USER_MMAP_VIRT,
            Self::Dyn => USER_DYN_BASE,
        }
    }

    /// 窗口的页数。
    fn pages(self) -> usize {
        match self {
            Self::Image => USER_IMAGE_MAX_PAGES,
            Self::Heap => USER_HEAP_PAGES,
            Self::Mmap => USER_MMAP_PAGES,
            Self::Dyn => USER_DYN_PAGES,
        }
    }

    /// `addr` 落在哪个窗口里（每个窗口都是 `[base, base + pages*PAGE_SIZE)`）。
    ///
    /// 用户栈**不在**其中：它的保护属性固定，`mprotect` 碰它就 `EINVAL`（见模块文档）。
    fn of(addr: u64) -> Option<Self> {
        for window in [Self::Image, Self::Heap, Self::Mmap, Self::Dyn] {
            let base = window.base();
            if addr >= base && addr < base + window.pages() as u64 * PAGE_SIZE {
                return Some(window);
            }
        }
        None
    }
}

/// `PROT_*` → 页表标志。**`PRESENT` 恒置**：`PROT_NONE` 表达的是「ring3 不可访问」
/// （清 `USER` 位）而不是「没有映射」—— 页帧与内容都保留，`mprotect` 回来即可用。
///
/// 两个 x86_64 事实值得记下来：
///
/// - 硬件没有「不可读但可执行」这一档，`PROT_EXEC` 单独出现时实际是**可读可执行**
///   （Linux 在 x86 上同样如此，所以不算偏离 ABI）；
/// - 没有 `USER` 的页 ring3 访问会 #PF（`error=P|U`），而内核（ring0）照常可访问 ——
///   这正是「用户态不可访问、内核还能把帧留着」的实现方式。
fn prot_to_flags(prot: u64) -> u64 {
    if prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0 {
        return PRESENT | NO_EXEC; // PROT_NONE：不可读、不可写、不可执行
    }
    let mut flags = PRESENT | USER | NO_EXEC;
    if prot & PROT_WRITE != 0 {
        flags |= WRITABLE;
    }
    if prot & PROT_EXEC != 0 {
        flags &= !NO_EXEC;
    }
    flags
}

/// ring3 能访问的页必须同时有 `PRESENT` 与 `USER`（页表每一级的 `USER` 都已在
/// [`UserSpace::new`] 里置好）。少任何一个都返回 `None`：syscall 的缓冲校验据此把
/// `PROT_NONE` 的内存排除在「合法用户缓冲」之外。
fn accessible(flags: u64) -> Option<u64> {
    if flags & (PRESENT | USER) == PRESENT | USER {
        Some(flags)
    } else {
        None
    }
}

/// 在自拥有的 PT 上建立一页映射（分配 + 清零），并记录到 `slot`。
///
/// 已经映射过的槽位是幂等的：只重写权限位，不重新分配、不清零
/// （与 [`UserSpace::map_image_page`] 对「同页被多个段覆盖」的处理一致）。
///
/// `invlpg` 是必需的：`munmap`/收缩后同一地址可能又会被重新映射，
/// TLB 里的旧项必须失效，否则会出现「页帧已归还、访问还成功」的幽灵映射。
fn map_new_page(pt: u64, slot: &mut Option<PhysFrame>, virt: u64, flags: u64) -> Result<(), i64> {
    if let Some(frame) = *slot {
        unsafe { write_entry(pt, table_index(virt, 0), frame.start_address() | flags) };
        paging::flush_tlb(virt);
        return Ok(());
    }
    let frame = frame::allocate().ok_or(ENOMEM)?;
    // SAFETY: 该帧由本地址空间拥有；内核 CR3 下帧地址恒等可写
    unsafe { core::ptr::write_bytes(frame.start_address() as *mut u8, 0, PAGE_SIZE as usize) };
    unsafe { write_entry(pt, table_index(virt, 0), frame.start_address() | flags) };
    paging::flush_tlb(virt);
    *slot = Some(frame);
    Ok(())
}

/// 清掉一页映射并归还页帧（`slot` 为空时什么也不做）。
fn unmap_page(pt: u64, slot: &mut Option<PhysFrame>, virt: u64) {
    if let Some(frame) = slot.take() {
        unsafe { write_entry(pt, table_index(virt, 0), 0) };
        paging::flush_tlb(virt);
        frame::free(frame);
    }
}

/// 递归清除 `USER` 位：从 `level` 层往下（2=PDPT，1=PD，0=PT），
/// 返回累计清除的项数（通过 `cleared` 累加）。
///
/// **惰性写入**：只在 `USER` 确实为 1 时写回。固件页表页是只读的
/// （见模块头注释），无条件写回会在启动路径上直接 #PF；而 `USER` 本来就
/// 为 0 的表项“改成 0”没有任何意义。
///
/// 注意：内核子树与内核自身**共享**这些页表帧（PML4 项是按值拷贝），
/// 因此本函数会同时清掉内核页表中的 `USER` 位 —— 这正是期望结果
/// （内核页面本就应为 ring0 专属），对 ring0 运行没有影响。
///
/// # Safety
/// `table` 必须指向有效页表帧；调用期间不得有其它 CPU 并发修改同一页表。
unsafe fn scrub_user_bits(table: u64, level: usize, cleared: &mut u64) {
    unsafe {
        for i in 0..paging::ENTRY_COUNT {
            let entry = paging::read_entry(table, i);
            if entry & PRESENT == 0 {
                continue;
            }
            if entry & HUGE == 0 && level > 0 {
                scrub_user_bits(entry_address(entry), level - 1, cleared);
            }
            if entry & USER != 0 {
                write_entry(table, i, entry & !USER);
                *cleared += 1;
            }
        }
    }
}
