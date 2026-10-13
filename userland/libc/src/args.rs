//! 命令行参数（`argc` / `argv`）—— v0.9 第三步随 `execve` 一起可用
//!
//! ## 传递方式（本内核的入口 ABI）
//!
//! `execve` 与内核的顶层运行驱动都把 `argc`/`argv` 放在**入口寄存器**里：
//!
//! ```text
//! iretq → _start    rdi = argc，rsi = argv（指向 argc 个 NUL 结尾的 C 字符串）
//!        │
//!        ▼  _start 的 `call cer_start`
//!   cer_start 把两者存进本模块的静态量 → 调用用户 main → exit(main())
//! ```
//!
//! Linux 把 `argc` 放在用户栈顶、`argv` 紧接着它；本内核选择寄存器，是因为
//! 总入口是我们自己的 `_start`（`libc/src/lib.rs`）—— 两者对用户程序都是透明的。
//!
//! 字符串本身由内核写在**新进程的用户栈**上（见 `syscall::process::build_user_frame`），
//! 因此生命周期与进程相同，取出来就是 `&'static [u8]`。
//!
//! 环境变量（`envp`）走同一条入口寄存器通道，但单独放在 [`crate::env`] 模块；
//! `argv[0]` 的规范化仍未做（内核原样透传调用方给的内容）。

/// 单个参数的最大字节数（与内核 `syscall::process::ARG_MAX_LEN` 一致）。
const ARG_MAX_LEN: usize = 256;

/// 参数个数（由 `_start` 写入）。
static mut ARGC: usize = 0;
/// 参数指针数组的地址（由 `_start` 写入；指向用户栈上的 `argc + 1` 个 u64）。
static mut ARGV: u64 = 0;

/// 记录入口寄存器里的 `argc`/`argv`（只由 `cer_start` 调用）。
pub(crate) fn set(argc: u64, argv: u64) {
    unsafe {
        ARGC = argc as usize;
        ARGV = argv;
    }
}

/// 参数个数（`argv` 的长度，不含结尾的 NULL）。
pub fn argc() -> usize {
    unsafe { ARGC }
}

/// 第 `i` 个参数（不含结尾的 NUL）；越界或指针为空返回 `None`。
pub fn arg(i: usize) -> Option<&'static [u8]> {
    if i >= argc() {
        return None;
    }
    let table = unsafe { ARGV } as *const u64;
    if table.is_null() {
        return None;
    }
    let ptr = unsafe { core::ptr::read(table.add(i)) } as *const u8;
    if ptr.is_null() {
        return None;
    }
    // 内核写入的字符串一定有 NUL 结尾；这里仍然限长，避免一个坏指针把进程读飞。
    let mut len = 0usize;
    while len < ARG_MAX_LEN && unsafe { core::ptr::read(ptr.add(len)) } != 0 {
        len += 1;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// 第 `i` 个参数（UTF-8 视图）；非 UTF-8 返回 `None`）。
pub fn arg_str(i: usize) -> Option<&'static str> {
    core::str::from_utf8(arg(i)?).ok()
}

/// 参数迭代器（按顺序产出每个参数，不含结尾的 NULL）。
pub struct Args {
    next: usize,
}

impl Args {
    /// 全部参数。
    pub fn new() -> Self {
        Self { next: 0 }
    }
}

impl Default for Args {
    fn default() -> Self {
        Self::new()
    }
}

impl Iterator for Args {
    type Item = &'static [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let item = arg(self.next)?;
        self.next += 1;
        Some(item)
    }
}
