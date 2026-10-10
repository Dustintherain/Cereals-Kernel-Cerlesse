//! 系统调用号到内核处理函数的表（v0.6 骨架）
//!
//! 本子模块仅维护编号与分发签名；具体 handler 放在
//! `kernel/src/syscall/{process,file,memory}.rs` 里供整个子模块导入。
//!
//! Linux x86_64 syscall 号的最小子集（v0.6）：
//! read=0, write=1, open=2, close=3, getpid=39, nanosleep=35, exit=60,
//! fork=57, execve=59, wait4=61。未注册号按 `-ENOSYS` 失败。

/// 系统调用号（Linux x86_64 语义）
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SLEEP: u64 = 35;
pub const SYS_EXIT: u64 = 60;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXEC: u64 = 59;
pub const SYS_WAIT: u64 = 61;
/// 系统调用号上限（不包含），用于边界检查。
pub const SYS_MAX: u64 = 256;

/// 系统调用处理函数的类型。`args[0..5]` = rdi, rsi, rdx, r10, r8。
pub type SyscallHandler = unsafe fn(args: &[u64; 5], space: *const crate::syscall::address_space::UserSpace) -> i64;

/// 公共 ENOSYS 桩。
pub(crate) fn syscall_enosys(_args: &[u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> i64 {
    -38 // -ENOSYS
}

/// 系统调用表（按 SYS_MAX 排满，已注册槽位替换为具体 handler）。
pub const SYS_TABLE: [SyscallHandler; SYS_MAX as usize] = {
    let mut table: [SyscallHandler; SYS_MAX as usize] = [syscall_enosys; SYS_MAX as usize];
    table[SYS_EXIT as usize] = crate::syscall::process::sys_exit;
    table[SYS_WRITE as usize] = crate::syscall::file::sys_write;
    table[SYS_GETPID as usize] = crate::syscall::process::sys_getpid;
    table[SYS_SLEEP as usize] = crate::syscall::process::sys_sleep;
    table
};
