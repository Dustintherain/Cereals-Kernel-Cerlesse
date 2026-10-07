//! PIT 定时器配置（v0.4 起）
//!
//! 本模块提供对 8253/8254 PIT（端口 0x40/0x43）的最小直接操作接口。
//! 当前阶段目标是把 PIT 配置为周期性 IRQ0 源，供后续 tick 处理使用。
//!
//! 端口约定：
//! - PIT 命令端口：0x43
//! - PIT 通道 0 数据端口：0x40
//!
//! 命令字节约定（本模块当前仅使用通道 0、模式 3、遍历访问）：
//! - 0x36 = 0b00110110
//!   bit7..6 = 00：通道 0
//!   bit5..4 = 11：加载方式为先低字节后高字节（read-back/latch 之外的常规加载）
//!   bit3..1 = 011：模式 3（方波/终端可用）
//!   bit0 = 0：二进制计数
//!
//! 注意：
//! - 本模块暂不负责中断使能/屏蔽，只负责 PIT 本身的计数器配置。
//! - IRQ0 是否真正递交，取决于 PIC mask 与中断使能状态。

#![allow(dead_code)]

const PIT_CMD_PORT: u16 = 0x43;
const PIT_CH0_DATA_PORT: u16 = 0x40;

/// PIT 命令字节（通道 0、分两次加载、模式 3、二进制）。
pub const PIT_CMD_BYTE: u8 = 0x36;

/// 对 PIT 通道 0 写入一个 16 位计数值（先低字节再高字节）。
///
/// # Safety
///
/// 调用者必须确保此时对 0x40/0x43 的访问是安全的，即：
/// - 未被其他同时运行的逻辑并发访问相同端口；
/// - 不在禁用原子性要求的上下文中误用。
pub unsafe fn write_counter16(value: u16) {
    unsafe {
        outb(PIT_CMD_PORT, PIT_CMD_BYTE);
        outb(PIT_CH0_DATA_PORT, value as u8);
        outb(PIT_CH0_DATA_PORT, (value >> 8) as u8);
    }
}

/// 设置 PIT 通道 0 的重装载值。
///
/// # Safety
///
/// 同 `write_counter16` 的端口访问安全要求。
pub unsafe fn set_channel0_reload(reload: u16) {
    unsafe { write_counter16(reload); }
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack, preserves_flags),
        );
    }
}
