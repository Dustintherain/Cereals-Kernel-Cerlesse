//! 用户态 ↔ 内核态边界（v0.6）
//!
//! 任务（模块级函数）：保存用户寄存器快照、参照链接脚本固定地址进入内核
//! `dispatch`、在 `cr3` 切换后 `sysretq` 回用户态。内核侧真正的切换由
//! `arch::x86_64::context::switch_to` 完成；本模块只是提供稳定、可链接的入口。

use core::arch::naked_asm;

/// 宏包装：在 `#[unsafe(naked)]` 函数中，可通过 `trampoline_asm!` 直接写裸指令，
/// 但它底层仍是 `naked_asm!`，因此只能使用全局作用域内联汇编语法（字符串字面量 + options）。
#[allow(unused_macros)]
macro_rules! trampoline_asm {
    ($($tt:tt)*) => {
        naked_asm!($($tt)*)
    };
}

/// 用户态启动入口（`user/src/start.rs` 放的 `start` → `main` 后回到这里）。
///
/// 链接脚本固定该符号位置，以便 `sys_enter` 内 `sysretq` 回到它。VMA 由链接脚本控制；
/// v0.6 默认构建无虚拟内存，所有用户程序在内核同一段地址空间可见。
///
/// 约定：
/// - `user_enter` 由用户 `start` 通过 `jmp` 进入，入口栈是空的（无返回帧）。
/// - `user_enter` 把第 5 个参数从栈写到 `r8`，保存 `rflags/cs/rsp` 到 `r9`，
///   然后 `call *kernel_dispatch_user_entry`。`call` 把返回 `rip` 压栈，`r8`
///   还是第 5 个参数（`rbx` 的拷贝）。
/// - 通过 `sysretq` 退出：`rsp += 8` 弹出由 `kernel_dispatch_user_entry` 压的
///   5th-arg 快照，`ss`、`rsp`、`rflags`、`cs`、`rip` 按 user-mode 栈恢复。
#[unsafe(naked)]
pub unsafe extern "sysv64" fn user_enter() {
    trampoline_asm!(
        // 保存第 5 个参数：调用方保证 `rbx` 已是该参数；用户 `start` 按 SysV 放栈上。
        "mov r8, rbx",
        // 保存用户 rflags/cs/rsp。
        "pushfq",
        "push r8",
        // 进入内核分发：`kernel_dispatch_user_entry` 由链接脚本固定，需现场保存 r10 用作
        // 返回值槽（被调用者不会覆盖它），保存完 r10 后 `call *kernel_dispatch_user_entry`
        // 的返回地址会压栈，之后也可用来恢复 iret 需要的用户 `rsp`。
        "mov r10, r10",
        "call *kernel_dispatch_user_entry",
        // 返回到用户态：`kernel_dispatch_user_entry` 已把 5th arg 弹到 `rax`，`r8` 存
        // 着用户第 5 个参数。`sysretq` 弹出 iret 帧：`ss`、`rsp`、`rflags`、`cs`、`rip`。
        "sysretq",
        // 链接脚本锚点，供 `kernel_dispatch_user_entry` 用 `lea` 取地址。
        "user_entry",
        "kernel_dispatch_user_entry",
    );
}

/// 用户态 `dispatcher` 入口。v0.6 把它放到一条稳定表里；
/// 链接脚本在 `user_entry` 段固定其地址，避免 rustc 寄存器分配。
///
/// 约定：
/// - 被 `user_enter` 的 `call *kernel_dispatch_user_entry` 直接调用（不经函数调用约定）。
/// - 栈上 `[rsp + 8]` 存放用户态第 5 个参数的快照（用户 `start` 压在调用前）；
///   `[rsp + 0]` 是 `call` 压入的返回地址，即用户 `rip` 值。
/// - 读到 5th arg 后 `pop r10`，`sys_enter(nid, args, space)` 的用户态返回值
///   通过 `r10` → `sysretq` 恢复到 `rax`。
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kernel_dispatch_user_entry() {
    trampoline_asm!(
        "mov rbx, [rsp + 8]",
        "pop r10",
        "mov rdi, rbx",
        "mov rsi, r10",
        "mov rdx, 0",
        "mov rcx, 0",
        "mov r8, 0",
        "call crate::syscall::sys_enter",
        "mov r10, rax",
        "sysretq",
        "kernel_dispatch_user_entry",
    );
}

// 链接脚本锚点：用于将来在运行期填入 `user_entry` / dispatcher 符号地址。
#[no_mangle]
pub static USER_ENTRY_ANCHOR: u8 = 0;
#[no_mangle]
pub static KERNEL_DISPATCH_USER_ENTRY_ANCHOR: u8 = 0;
