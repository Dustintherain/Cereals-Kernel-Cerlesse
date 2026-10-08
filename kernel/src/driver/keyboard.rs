//! PS/2 键盘驱动（v0.4）
//!
//! 目标是建立最小可验证的键盘输入路径（DEVELOPMENT.md v0.4 验收项）：
//! - 键盘控制器（8042 兼容）初始化：排空输出缓冲、打开 IRQ1 使能位、启用键盘；
//! - IRQ1 回调读取扫描码 → 解码 set 1 扫描码 → 串口回显；
//! - 回显格式固定为 `KB <字符>`，便于集成测试断言。
//!
//! 端口约定（x86 PS/2 兼容控制器）：
//! - 数据端口：0x60
//! - 命令/状态端口：0x64
//!
//! 状态字节位：
//! - bit0：输出缓冲区满（OBF，有数据可读）
//! - bit1：输入缓冲区满（IBF，写入前需等待清零）
//!
//! 扫描码语义（set 1，控制器翻译开启时的常见形态）：
//! - `0x01..0x58`：按下；
//! - `0x80 | code`：释放（本驱动忽略）；
//! - `0xE0` / `0xE1`：扩展前缀（本驱动忽略其后的键）。
//!
//! 注意：当前阶段串口是唯一日志通道，键盘回显经串口打印，
//! 故 IRQ1 回调打印前后使用 `interrupt::irq` 的串口重入保护。

#![allow(dead_code)]

const KB_DATA_PORT: u16 = 0x60;
const KB_CMD_PORT: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

/// 8042 命令：读配置字节（命令字节写回数据端口读）。
const CMD_READ_CONFIG: u8 = 0x20;
/// 8042 命令：写配置字节（写入前把配置字节写到数据端口）。
const CMD_WRITE_CONFIG: u8 = 0x60;
/// 8042 命令：禁用键盘接口（不产出扫描码）。
const CMD_DISABLE_KEYBOARD: u8 = 0xAD;
/// 8042 命令：启用键盘接口。
const CMD_ENABLE_KEYBOARD: u8 = 0xAE;

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nomem, nostack, preserves_flags),
        );
    }
    val
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

/// 等待输入缓冲区可写（IBF 清零）。
///
/// # Safety
///
/// 需要键盘控制器存在；带次数上限，避免控制器异常时死循环。
unsafe fn wait_input_clear() {
    unsafe {
        let mut guard = 0u32;
        while inb(KB_CMD_PORT) & STATUS_INPUT_FULL != 0 && guard < 100_000 {
            guard += 1;
            core::hint::spin_loop();
        }
    }
}

/// 等待输出缓冲区有数据（OBF 置位）。
///
/// # Safety
///
/// 同上；返回 `false` 表示超时（控制器无响应）。
unsafe fn wait_output_full() -> bool {
    unsafe {
        let mut guard = 0u32;
        while inb(KB_CMD_PORT) & STATUS_OUTPUT_FULL == 0 {
            if guard >= 100_000 {
                return false;
            }
            guard += 1;
            core::hint::spin_loop();
        }
        true
    }
}

/// 读取数据端口，丢弃一个字节（用于排空残留数据）。
///
/// # Safety
///
/// 仅在确认 OBF 置位（或愿意接受无数据）时调用。
unsafe fn drain_output() {
    unsafe {
        while inb(KB_CMD_PORT) & STATUS_OUTPUT_FULL != 0 {
            let _ = inb(KB_DATA_PORT);
        }
    }
}

/// 键盘控制器初始化。
///
/// 流程（沿用常见的 8042 初始化序列）：
/// 1. 排空输出缓冲区（丢弃上电自检 `0xAA`、ACK `0xFA` 等残留字节）；
/// 2. 读配置字节，置位 bit0（打开 IRQ1）、保留 bit6（保留扫描码翻译）；
/// 3. 写回配置字节；
/// 4. 启用键盘接口，并再次排空（吸收 `0xAE` 的 ACK）。
///
/// 调用前提：串口已初始化；调用方在启用 PIC 的 IRQ1 之前调用本函数。
/// 排在启用 IRQ1 之前是关键：若输出缓冲仍有残留字节，启用 IRQ1 会立刻产生一次伪中断。
///
/// # Safety
///
/// 会直接读写 8042 端口；调用方需保证当前为关中断（或尚未启用 IRQ1）的单核上下文。
pub unsafe fn init() {
    unsafe {
        drain_output();

        // 读取当前配置字节
        wait_input_clear();
        outb(KB_CMD_PORT, CMD_READ_CONFIG);
        if !wait_output_full() {
            return;
        }
        let config = inb(KB_DATA_PORT);

        // bit0: 打开键盘中断（IRQ1）；bit6: 保留控制器的 set2→set1 翻译设置。
        let new_config = (config | 0x01) & !0x10; // 同时清 bit4（键盘时钟禁用位）

        wait_input_clear();
        outb(KB_CMD_PORT, CMD_WRITE_CONFIG);
        wait_input_clear();
        outb(KB_DATA_PORT, new_config);

        wait_input_clear();
        outb(KB_CMD_PORT, CMD_ENABLE_KEYBOARD);

        // 吸收启用命令产生的 ACK，避免启用 IRQ1 后的伪中断。
        drain_output();
    }
}

/// 若输出缓冲区有数据，读取并返回一个扫描码字节。
///
/// # Safety
///
/// 调用前提：键盘控制器已初始化。中断上下文与轮询上下文均可用。
pub unsafe fn poll_scan() -> Option<u8> {
    unsafe {
        if inb(KB_CMD_PORT) & STATUS_OUTPUT_FULL == 0 {
            return None;
        }
        Some(inb(KB_DATA_PORT))
    }
}

/// 扫描码解码状态（单核内核，用模块级 `static mut` 保存）。
///
/// 当前仅跟踪扩展前缀与 Shift，用于把字母/数字/Space 解码为可打印字符。
static mut EXTENDED_PREFIX: bool = false;
static mut SHIFT_HELD: bool = false;

/// 释放某个处理状态（重启/自测用，仅开发期调用）。
pub unsafe fn reset_decode_state() {
    unsafe {
        EXTENDED_PREFIX = false;
        SHIFT_HELD = false;
    }
}

/// set 1 扫描码 → 可打印字符。
///
/// 返回 `None` 表示该字节不产生字符输出（按下 Shift、释放码、扩展键等）。
///
/// # Safety
///
/// 单核 + 串行调用（轮询或 IRQ1 回调二选一），不得并发调用。
pub unsafe fn decode_set_1_scan(code: u8) -> Option<u8> {
    unsafe {
        // 扩展前缀：后续一个字节的键一律忽略（方向键、小键盘等）。
        if code == 0xE0 || code == 0xE1 {
            EXTENDED_PREFIX = true;
            return None;
        }
        if EXTENDED_PREFIX {
            EXTENDED_PREFIX = false;
            return None;
        }

        // 修饰键：Shift 左右按下/释放。
        match code {
            0x2A | 0x36 => {
                SHIFT_HELD = true;
                return None;
            }
            0xAA | 0xB6 => {
                SHIFT_HELD = false;
                return None;
            }
            _ => {}
        }

        // 其余释放码（bit7 置位）忽略。
        if code & 0x80 != 0 {
            return None;
        }

        let shifted = SHIFT_HELD;
        let ch = match code {
            // 字母区
            0x1E => b'a', 0x30 => b'b', 0x2E => b'c', 0x20 => b'd', 0x12 => b'e',
            0x21 => b'f', 0x22 => b'g', 0x23 => b'h', 0x17 => b'i', 0x24 => b'j',
            0x25 => b'k', 0x26 => b'l', 0x32 => b'm', 0x31 => b'n', 0x18 => b'o',
            0x19 => b'p', 0x10 => b'q', 0x13 => b'r', 0x1F => b's', 0x14 => b't',
            0x16 => b'u', 0x2F => b'v', 0x11 => b'w', 0x2D => b'x', 0x15 => b'y',
            0x2C => b'z',
            // 数字区
            0x0B => b'0', 0x02 => b'1', 0x03 => b'2', 0x04 => b'3', 0x05 => b'4',
            0x06 => b'5', 0x07 => b'6', 0x08 => b'7', 0x09 => b'8', 0x0A => b'9',
            // 其他常用键
            0x39 => b' ', 0x1C => b'\r', 0x0E => 0x08, // Space / Enter / Backspace
            0x0C => b'-', 0x0D => b'=', 0x1A => b'[', 0x1B => b']',
            0x27 => b';', 0x28 => b'\'', 0x29 => b'`', 0x2B => b'\\',
            0x33 => b',', 0x34 => b'.', 0x35 => b'/',
            _ => return None,
        };

        Some(if shifted && ch.is_ascii_lowercase() {
            ch.to_ascii_uppercase()
        } else {
            ch
        })
    }
}

/// 串口回显一个已解码字符，格式：`KB <字符>`。
///
/// # Safety
///
/// 需要串口已初始化；调用方负责串口重入保护。
pub unsafe fn echo_char(c: u8) {
    crate::driver::serial::print("KB ");
    crate::driver::serial::putc(c);
    crate::driver::serial::print("\r\n");
}

/// IRQ1 回调：读取扫描码、解码并回显。
///
/// 关键顺序：先把扫描码读走（清 OBF），再打印。
/// 若串口正被占用（`serial_debug_enter` 失败），仍已完成读取，
/// 只是本次不输出回显，避免串口输出互相穿插。
///
/// # Safety
///
/// 仅由 IRQ1 中断上下文调用（`interrupt::irq` 分发）。
pub unsafe extern "C" fn keyboard_irq1_handler(_vector: u8) {
    let code = unsafe { poll_scan() };

    let acquired = unsafe { crate::interrupt::irq::serial_debug_enter() };
    if !acquired {
        return;
    }

    crate::driver::serial::print("KBIRQ ENTRY\r\n");
    if let Some(code) = code {
        if let Some(ch) = unsafe { decode_set_1_scan(code) } {
            unsafe { echo_char(ch) };
        }
    }
    crate::driver::serial::print("KBIRQ EXIT\r\n");

    unsafe { crate::interrupt::irq::serial_debug_exit() };
}
