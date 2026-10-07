//! CPU 初始化与控制寄存器操作（v0.1–v0.3）
//!
//! v0.3：读取 CR3（当前页表根）供内存子系统使用；
//! 其余 CR0/CR4 配置随后续版本（v0.4 中断、v0.6 用户态）补充。

/// 读取 CR3（当前页表根物理地址）。
pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}
