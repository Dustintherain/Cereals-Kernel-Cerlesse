//! 驱动模块
//!
//! - `serial`：16550 UART（v0.1，开发期唯一日志通道）
//! - `keyboard`：PS/2 键盘（v0.4，IRQ1 扫描码 → set 1 解码 → 串口回显）
//! - `serial` 入口：`init / putc / print / println / print_dec / print_hex`，
//!   以及 `put_bytes` 批量入口，供 `syscall::file::sys_write` 直接调用。
pub mod keyboard;
pub mod serial;
// TODO(v0.8): pub mod pci / block;
