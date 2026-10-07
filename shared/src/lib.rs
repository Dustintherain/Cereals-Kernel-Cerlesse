//! shared — boot 与 kernel 之间共享的数据结构（第一版：BootInfo）
//!
//! 被 `boot`（x86_64-unknown-uefi）与 `kernel`（x86_64-unknown-none）同时编译，
//! 必须保持 `#![no_std]` 且只包含纯数据定义。

#![no_std]

/// BootInfo 魔数："CERLESSE"（小端）
pub const BOOT_MAGIC: u64 = u64::from_le_bytes(*b"CERLESSE");

/// 由 Bootloader 在 ExitBootServices 之后传给内核入口（RDI）。
///
/// `memory_map` 指向 UEFI 内存描述符数组（v0.3 解析）。
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub memory_map: *const u8,
    pub memory_map_size: usize,
    pub memory_descriptor_size: usize,
    pub memory_descriptor_version: u32,
    pub _reserved: u32,
}
