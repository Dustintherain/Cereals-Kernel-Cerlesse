//! 系统调用号到内核处理函数的表
//!
//! 本子模块仅维护编号与签名，不放具体实现；具体 handler 放在
//! `kernel/src/syscall/{process,file,memory}.rs` 里供整个子模块导入。


/// 系统调用号（Linux x86_64 语义的最小子集）。内核直接调用 `syscall` 指令
///，因此 `rax` 必须是合法编号；未注册号按 `ENOSYS` 失败。
pub const SYS_EXIT: u64 = 60;
pub const SYS_WRITE: u64 = 64;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SLEEP: u64 = 35;

/// 系统调用号上限（不包含），用于边界检查。
pub const SYS_MAX: u64 = 256;

/// 系统调用处理函数的类型。用户态寄存器快照与页表根由调用方在 `sys_enter` 层填好。
pub type SyscallHandler = unsafe extern "sysv64" fn(
    args: [u64; 5],
    space: *const crate::syscall::address_space::UserSpace,
) -> u64;

/// 公共 ENOSYS 桩，由 `sys_enter` 与 syscall 表默认槽位共享语义。
pub(crate) unsafe extern "sysv64" fn syscall_enosys(_: [u64; 5], _: *const crate::syscall::address_space::UserSpace) -> u64 {
    0xFFFFFFFFFFFFFFFE
}

/// 系统调用表（按 SYS_MAX 排满，已注册槽位替换为具体 handler）。
#[allow(clippy::missing_const_for_fn)]
pub const SYS_TABLE: [SyscallHandler; SYS_MAX as usize] = {
    let mut table: [SyscallHandler; SYS_MAX as usize] = [syscall_enosys; SYS_MAX as usize];
    table[SYS_EXIT as usize] = crate::syscall::sys_exit;
    table[SYS_WRITE as usize] = crate::syscall::sys_write;
    table[SYS_GETPID as usize] = crate::syscall::sys_getpid;
    table[SYS_SLEEP as usize] = crate::syscall::sys_sleep;
    table
};

/// 解析系统调用号：`0 <= nid < SYS_MAX` 且该槽位有 handler 才合法。
#[must_use]
pub fn parse(nid: u64) -> Option<usize> {
    if nid < SYS_MAX {
        Some(nid as usize)
    } else {
        None
    }
}
