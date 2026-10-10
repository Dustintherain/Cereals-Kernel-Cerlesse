//! 用户地址空间（v0.6 最小隔离）
//!
//! 目标：让“用户态进程”的虚拟页表成为一个可切换的根，不改变内核自身的
//! 恒等映射布局。v0.6 第一版本：
//! - 用 `UserSpace` 管理一棵 4 级页表（复用 `paging` 的 `AddressSpace` 接口），
//!   并记录其根地址；
//! - 低半区用户起点 `USER_VIRT_BASE`（0x4000_0000）映射到内核为用户程序
//!   分配的物理帧；同时复制内核现有恒等映射到该空间，保证内核代码/数据
//!   可在该页表下运行（CR3 切换语义）；
//! - `USER_VIRT_BASE` 起的后两页标记 `USER` 标志，以供 ring0/ring3 使用；
//! - 提供 `create_user_space` 与 `destroy_user_space`，并在进程退出时回收。
//!
//! 当前构建不开启 `USER_SPACE` 特性；`UserSpace::new` 返回错误，表示“不具备用户空间”。
//! 启用后得到第一版隔离布局。v0.7 会把完整的用户 ELF 装入该空间并切换 `cr3`。

use crate::arch::x86_64::paging::AddressSpace;
use crate::memory::frame::PhysFrame;

/// 用户虚拟地址的起点（低半区，页表索引 0）。与内核恒等映射共用同一 PML4 树，
/// 且该区域的页表项中会设置 `USER` 标志，使 ring0/ring3 均可访问。
pub const USER_VIRT_BASE: u64 = 0x4000_0000;

/// 用户地址空间（可选）。v0.6 当前默认构建不启用用户空间。
pub struct UserSpace {
    #[allow(dead_code)]
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
