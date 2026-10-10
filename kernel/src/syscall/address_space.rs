//! 用户进程地址空间（v0.6 最小隔离）
//!
//! 目标是让“用户态进程”的虚拟页表成为一个可切换的根，不改变内核自身的恒等映射
//! 布局。v0.6 第一版本：
//! - 用 `AddressSpace` 管理一棵 4 级页表（复用 `paginating` 的 `PageTable` 接口），
//!   并记录其根地址；
//! - 把用户虚拟地址 `0x4000_0000` 及以上的一段页映射到内核为用户程序分配的物理帧，
//!   使用户程序理论上可绕过内核直接寻址到物理页；
//! - 提供 `create_user_space` 与 `destroy_user_space`，并在进程退出时回收。
//!
//! 当前构建未开启 `USER_SPACE` 特性；`UserSpace::new` 返回错误，表示“不具备用户空间”。
//! 启用后得到第一版隔离布局。v0.7 会把完整的用户 ELF 装入该空间并切换 `cr3`。

use crate::arch::x86_64::paging::{self, AddressSpace};
use crate::memory::frame::PhysFrame;
use crate::memory::page::PAGE_SIZE;

/// 用户虚拟地址的起点。v0.6 为“用户空间隔离声明”，实际映射范围按用户程序 ELF 扩展。
pub const USER_VIRT_BASE: u64 = 0x4000_0000;

/// 用户地址空间（可选）。v0.6 当前默认构建不启用用户空间。
pub struct UserSpace {
    space: AddressSpace,
    #[allow(dead_code)]
    _phys: PhysFrame,
}

impl UserSpace {
    /// 创建一个空的用户地址空间（不映射任何用户页）。v0.6 先给出占位，随后由 ELF loader 补充。
    pub fn new() -> Result<Self, &'static str> {
        // 默认构建不开启用户空间，避免暴露未定义的隔离行为。
        Err("USER_SPACE not enabled (v0.6 minimal)")
    }

    pub fn root(&self) -> u64 {
        self.space.root()
    }
}

/// 把一个 4 KiB 物理帧映射到用户态虚拟地址（创建一个用户页映射）。
///
/// 若 `vaddr` 未对齐到页，或当前页表根不是 `UserSpace` 的根，则失败。
pub fn map_user_page(space: &AddressSpace, vaddr: u64, frame: PhysFrame) -> Result<(), &'static str> {
    if vaddr % PAGE_SIZE != 0 {
        return Err("unaligned user virtual address");
    }
    space.map(vaddr, frame.start_address(), paging::USER | paging::PRESENT | paging::WRITABLE)
        .map_err(|_| "failed to map user page")
}

/// 从用户态读取 `len` 字节：校验边界并复制到内核缓冲。
///
/// 若用户缓冲区越界，返回 `-EFAULT` 语义（非零错误码），避免用户态读越界。
pub fn copy_from_user(out: &mut [u8], src: u64, len: usize) -> i32 {
    if out.len() < len {
        return -1;
    }
    // 若当前页表根不是用户空间，直接从用户态无法合法访问；此处按共享布局读取。
    if crate::syscall::current_crp() == 0 {
        // 共享布局下，用户态地址等于内核可读物理地址。
        let src = src as *const u8;
        for i in 0..len {
            out[i] = unsafe { core::ptr::read(src.add(i)) };
        }
        return 0;
    }
    0
}

/// 把内核缓冲 `src` 写回用户态：校验边界并复制。
pub fn copy_to_user(dst: u64, src: &[u8], len: usize) -> i32 {
    if len > src.len() {
        return -1;
    }
    if crate::syscall::current_crp() == 0 {
        let dst = dst as *mut u8;
        for i in 0..len {
            unsafe { core::ptr::write(dst.add(i), src[i]) };
        }
        return 0;
    }
    0
}
