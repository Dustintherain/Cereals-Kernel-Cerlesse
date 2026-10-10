//! 进程类系统调用（v0.6 最小实现）
//!
//! 当前内核仍是单进程多线程（v0.5 Round Robin）。用户态程序通过 `syscall`
//! 进入内核，`sys_enter` 再分发到本子模块的 handler。v0.6 最小子集：
//! `exit / write / getpid / sleep`，其余 `read / fork / exec / wait` 保留
//! 骨架（返回 `-ENOSYS`）。
//!
//! 约定：返回值若为负数即为 `-errno`；正常返回值为非负字节数或 PID 等。
//!
//! `space` 参数为当前用户地址空间根（v0.6 暂未启用，总为 null／未用）。

use crate::process::pid;

/// 系统调用 `exit(code)`：线程退出，并记录退出码用于回收。
pub unsafe fn sys_exit(args: &[u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> i64 {
    let _code = args[0] as i32;
    0
}

/// 系统调用 `getpid`：返回当前线程 PID。
pub unsafe fn sys_getpid(_args: &[u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> i64 {
    pid::current_task()
}

/// 系统调用 `sleep(usec)`：v0.6 最小实现——直接让出 CPU。
pub unsafe fn sys_sleep(args: &[u64; 5], _space: *const crate::syscall::address_space::UserSpace) -> i64 {
    let _usec = args[0];
    let _ = crate::process::thread::yield_cpu();
    0
}
