//! 环境变量（`envp`）—— v1.0 随 syscall ABI 第 6 参数一起可用
//!
//! ## 传递方式（本内核的入口 ABI）
//!
//! `execve` 与内核的顶层运行驱动都把 `envc`/`envp` 放在**入口寄存器**里
//! （与 `argc`/`argv` 同一条路，见 [`crate::args`]）：
//!
//! ```text
//! iretq → _start    rdi = argc，rsi = argv，rdx = envc，r10 = envp
//!        │
//!        ▼  _start 把 r10 搬到 rcx 后 `call cer_start`
//!   cer_start 把四个值存进静态量 → 调用用户 main → exit(main())
//! ```
//!
//! `envp` 用 `r10` 而不是 `rcx`：`rcx` 在 `syscall`/`sysret` 往返里是用户 RIP，
//! 帧恢复时载入的是入口地址；`_start` 先在用户态把它腾出来，再调用 C 入口。
//!
//! 每个环境变量是一个 `NAME=VALUE` 字节串，内核只负责把字符串搬进新进程用户栈，
//! **不解析内容** —— 「名字/值」的拆分在这里（[`get`]）与 Shell 里完成。
//! 生命周期与进程相同，因此取出来就是 `&'static [u8]`。
//!
//! ## 继承语义
//!
//! [`crate::execve`] 默认把当前进程的环境原样传给新映像（POSIX 语义）；要替换环境
//! 用 [`crate::execve_env`]。顶层程序的环境来自内核默认集合
//! （`kernel/src/syscall/user.rs::DEFAULT_ENV`：`PATH=/bin`、`HOME=/`、`TERM=cerlesse`）。

/// 单个环境变量的最大字节数（与内核 `syscall::process::ENV_MAX_LEN` 一致）。
const ENV_MAX_LEN: usize = abi::limits::ENV_MAX_LEN as usize;

/// 环境变量个数（由 `_start` 写入）。
static mut ENVC: usize = 0;
/// 环境指针数组的地址（由 `_start` 写入；指向用户栈上的 `envc + 1` 个 u64）。
static mut ENVP: u64 = 0;

/// 记录入口寄存器里的 `envc`/`envp`（只由 `cer_start` 调用）。
pub(crate) fn set(envc: u64, envp: u64) {
    unsafe {
        ENVC = envc as usize;
        ENVP = envp;
    }
}

/// 环境变量个数。
pub fn count() -> usize {
    unsafe { ENVC }
}

/// 第 `i` 个环境变量（不含结尾的 NUL）；越界或指针为空返回 `None`。
pub fn env(i: usize) -> Option<&'static [u8]> {
    if i >= count() {
        return None;
    }
    let table = unsafe { ENVP } as *const u64;
    if table.is_null() {
        return None;
    }
    let ptr = unsafe { core::ptr::read(table.add(i)) } as *const u8;
    if ptr.is_null() {
        return None;
    }
    // 内核写入的字符串一定有 NUL 结尾；这里仍然限长，避免一个坏指针把进程读飞。
    let mut len = 0usize;
    while len < ENV_MAX_LEN && unsafe { core::ptr::read(ptr.add(len)) } != 0 {
        len += 1;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

/// 第 `i` 个环境变量（UTF-8 视图）；非 UTF-8 返回 `None`。
pub fn env_str(i: usize) -> Option<&'static str> {
    core::str::from_utf8(env(i)?).ok()
}

/// `get(name)`：取 `name` 对应的值（`NAME=VALUE` 里 `=` 之后的字节）。
///
/// 没有这个环境变量返回 `None`；值为空串返回 `Some("")`。
pub fn get(name: &str) -> Option<&'static str> {
    for line in Env::new() {
        if let Some(rest) = line.strip_prefix(name) {
            if let Some(value) = rest.strip_prefix('=') {
                return Some(value);
            }
        }
    }
    None
}

/// `name` 是否已定义（值可能为空串）。
pub fn has(name: &str) -> bool {
    get(name).is_some()
}

/// 环境变量迭代器（按顺序产出每个 `NAME=VALUE`，跳过非 UTF-8 项）。
pub struct Env {
    next: usize,
}

impl Env {
    /// 全部环境变量。
    pub fn new() -> Self {
        Self { next: 0 }
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Iterator for Env {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < count() {
            let item = env_str(self.next);
            self.next += 1;
            if item.is_some() {
                return item;
            }
        }
        None
    }
}
