//! PIT 定时器配置（v0.4 起）
//!
//! 本模块提供对 8253/8254 PIT（端口 0x40/0x43）的最小直接操作接口，
//! 把通道 0 配置为周期性 IRQ0 源，供 `time::clock` 维护 tick 计数。
//!
//! 端口约定：
//! - PIT 命令端口：0x43
//! - PIT 通道 0 数据端口：0x40
//!
//! 频率换算：
//! `reload = PIT_BASE_FREQUENCY / TICKS_PER_SECOND`。
//! 取 100Hz 时 reload = 1193182 / 100 ≈ 11932，即每个 tick ≈ 10ms。
//!
//! 命令字节 0x36 = 0b0011_0110：
//! - bit7..6 = 00：通道 0
//! - bit5..4 = 11：先低字节后高字节加载
//! - bit3..1 = 011：模式 3（方波）
//! - bit0 = 0：二进制计数
//!
//! 注意：本模块不负责中断使能/屏蔽，只负责 PIT 自身的计数器配置；
//! IRQ0 是否真正递交取决于 PIC mask 与处理器 IF。

#![allow(dead_code)]

const PIT_CMD_PORT: u16 = 0x43;
const PIT_CH0_DATA_PORT: u16 = 0x40;

/// PIT 输入基准频率（Hz）。
pub const PIT_BASE_FREQUENCY: u32 = 1_193_182;

/// 内核当前使用的 tick 频率（Hz）：100Hz → 10ms 一个 tick。
pub const TICKS_PER_SECOND: u32 = 100;

/// 100Hz 对应的通道 0 重装载值。
pub const TICK_RELOAD_100HZ: u16 = (PIT_BASE_FREQUENCY / TICKS_PER_SECOND) as u16;

/// 一个 tick 的毫秒数（100Hz → 10ms）。
pub const TICK_MILLIS: u32 = 1000 / TICKS_PER_SECOND;

/// PIT 命令字节（通道 0、分两次加载、模式 3、二进制）。
pub const PIT_CMD_BYTE: u8 = 0x36;

/// 对 PIT 通道 0 写入一个 16 位计数值（先低字节再高字节）。
///
/// # Safety
///
/// 调用者必须确保此刻对 0x40/0x43 的访问不被其他逻辑并发进行。
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
    unsafe { write_counter16(reload) }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_matches_100hz() {
        assert_eq!(TICKS_PER_SECOND, 100);
        assert_eq!(TICK_MILLIS, 10);
        // 1193182 / 100 = 11931（整数除法），误差 < 0.01%
        assert_eq!(TICK_RELOAD_100HZ, 11931);
    }
}
