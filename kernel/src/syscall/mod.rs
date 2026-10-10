//! 系统调用子系统（v0.6）
//!
//! 职责：
//! - 系统调用表：把 syscall 号映射到内核 handler。
//! - 地址空间隔离：区分内核页表根 `kcr3`、用户进程页表根 `ucr3`（v0.6 先记录，不做页表切换），
//!   以及当前活动 `cr3` 窗口。用户态程序以 tcb 中保存的用户页表根被切换进来。
//! - 最小用户态入口：内核 `kernel_main` 从固定内核地址 `user_entry` 取得用户入口，
//!   通过 `switch_to` 把第一次调度切到用户态上下文；用户态第一次执行 `sys_enter` 时
//!   恢复 `rip` 到用户入口，随后从 `main` 开始运行。
//!
//! 约定：
//! - 按 Linux x86_64 `syscall` ABI：`rax = syscall number`，`rdi..r10` = 4 个参数
//!   （第 5 个参数走栈 `rbx`），返回值于 `rax`。
//! - 所有从用户态传入的数据（指针、长度）必须先由内核校验，再由内核读写。
//! - 内核返回用户态时统一返回 `Ok(())`，并把 `rax` 作为用户 `syscall` 返回值。
//!   线程退出走同一个通道：`sys_exit` 把 `rax = 0` 后进入 `context::switch_to`
//!   的退出路径，让调度器回收该线程。

pub mod address_space;
pub mod syscall_table;
pub mod trampoline;
pub mod sys_enter;

use crate::arch::x86_64::context::{init_context, switch_to};
use crate::arch::x86_64::paging::AddressSpace;
use crate::process::context::ThreadContext;
use crate::process::pid;

/// 内核页表根（v0.6 启动时由 `kernel_main` 记录当前 CR3；植入内核栈可见）。
pub static mut KCR3: u64 = 0;

/// 用户进程页表根（用户态 `AddressSpace` 的根）。v0.6 生命周期只到进程退出。
pub static mut UCR3: u64 = 0;

/// 当前活动页表根窗口。v0.6 按进程粒度记录：0 = 内核共享窗口，>0 = 用户页表根。
pub static mut CURRENT_CR3: u64 = 0;

/// 用户态入口地址（用户态 `start` 后返回到用户入口 `UserMain`）。链接脚本填 `user_entry`。
#[repr(C)]
pub struct UserEntry {
    /// 用户程序入口点（`start` → `main`）。
    pub entry: usize,
    /// 链接脚本放入的内核调度器入口（`switch_to` 首次回到用户态时执行的 `sys_enter` 伪代码）。
    pub dispatcher: usize,
}

pub static USER_ENTRY: UserEntry = UserEntry {
    entry: 0,
    dispatcher: 0,
};

/// 初始化：记录内核页表根，建立唯一空的用户页表根（v0.6 只做隔离记录，不切换）。
pub fn init() {
    unsafe {
        KCR3 = crate::arch::x86_64::paging::current_root();
    }
}

/// 返回当前活动页表根（0 = 共享内核/用户窗口）。
pub fn current_crp() -> u64 {
    unsafe { CURRENT_CR3 }
}

/// 将当前活动页表根窗口切到 `AddressSpace`。v0.6 仅在 `switch_to_user` 之后调用。
pub fn set_current_crp(as_: &AddressSpace) {
    unsafe {
        CURRENT_CR3 = as_.root();
        core::arch::asm!("mov cr3, {0}", in(reg) as_.root(), options(nostack, preserves_flags));
    }
}

/// 切换到用户态任务。`rt` 必须指向已用 `init_context` 初始化、`cr3` 已设为用户
/// 页表根的 `ThreadContext`；`ctx` 保存用户态的完整寄存器快照。
///
/// # Safety
/// 调用者保证：任务栈安全、`rt` 指向已初始化的 `ThreadContext`、`ctx` 指向有效寄存器区。
pub unsafe fn switch_to_user(rt: *mut ThreadContext, ctx: *mut ThreadContext, space: &AddressSpace) {
    unsafe {
        // 先切换页表窗口，再做寄存器切换。cr3 切换发生在用户栈切换前，保证
        // 切出后的 `switch_to` 看到完整的用户态寄存器与用户页表。
        set_current_crp(space);
        (*rt).cr3 = space.root();
        switch_to(rt, ctx);
    }
}

// ---------------------------------------------------------------------------
// 首批 syscall handler：v0.6 至少实现 sys_exit / sys_sleep，让用户态任务能让出 CPU
// 并优雅退出。sys_write / sys_getpid 保持 stub，后续再补。
// ---------------------------------------------------------------------------

/// `sys_exit(exit_code)`：将当前线程状态设为 Terminated，并把 `rax = exit_code` 返回给用户态。
/// v0.6 中“当前线程”语义仍由调度器/主程序维护；sys_exit 仅请求退出，实际回收交给调度器。
pub unsafe extern "sysv64" fn sys_exit(args: [u64; 5], _: *const crate::syscall::address_space::UserSpace) -> u64 {
    let _exit_code = args[0];
    let _ = crate::process::thread::exit();
    _exit_code
}

/// `sys_sleep(ticks)`：立刻让出 CPU。
/// v0.6 最小实现：把当前线程标记为让出，并立即请求一次调度器轮转，
/// 避免用户态任务霸占一个时间片。
/// 真实的定时唤醒（基于 timer::sleep_1tick）在用户态 sleep 语义成熟后再接入。
pub unsafe extern "sysv64" fn sys_sleep(args: [u64; 5], _: *const crate::syscall::address_space::UserSpace) -> u64 {
    let _ticks = args[0];
    // 这里不直接调用 timer，因为用户态当前尚无定时器唤醒路径；
    // 先让当前线程主动让出 CPU，避免用户态任务霸占一个时间片。
    let _ = crate::process::thread::yield_cpu();
    0
}

/// `sys_write(fd, buf, len)`：v0.6 stub，尚未接入 VFS/fd 表。
pub unsafe extern "sysv64" fn sys_write(args: [u64; 5], _: *const crate::syscall::address_space::UserSpace) -> u64 {
    let _fd = args[0];
    let _buf = args[1];
    let _len = args[2];
    0
}

/// `sys_getpid()`：v0.6 stub，尚未接入进程模型。
pub unsafe extern "sysv64" fn sys_getpid(args: [u64; 5], _: *const crate::syscall::address_space::UserSpace) -> u64 {
    let _ = args;
    0
}
