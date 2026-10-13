//! sh —— 交互式 Shell（v0.9 第一步：行输入 + 解析；第三步：外部程序可用）
//!
//! 提示符格式（默认加载样式）：`用户名@主机名称 盘符>`，即 `root@cerlesse C:>`。
//! 三段都是常量（v0.9 没有用户库/主机名配置），盘符对应将来「切换卷」的动作
//! （`/disk` 已由 v0.8 挂在 VFS 上，但 v0.9 还没有 `cd`）。
//!
//! ## 交互链路（每一环都有可断言的串口轨迹）
//!
//! ```text
//! PS/2 键盘 IRQ1（v0.4）→ 行缓冲 + 行编辑（kernel/src/driver/keyboard.rs）
//!        │  kbd: line ready len=N
//!        ▼
//! read(0)（kernel/src/syscall/file.rs::read_stdin）：整行交付，行尾 `\n`
//!        │  user: read fd=0 want=128 got=5
//!        ▼
//! 本程序：提示符 → 解析 → 内建命令 / 外部程序 → 循环
//! ```
//!
//! ## 外部程序（v0.9 第三步）
//!
//! [`cmd_external`] 走的就是教科书式的三步，**没有任何兜底分支**：
//!
//! ```text
//! fork()  ──▶ 父进程：拿到子进程 PID，wait4 等它跑完，打印退出码
//!         └─▶ 子进程：拿到 0，execve("/bin/<cmd>", argv) 换上 /bin/hello 的映像
//! ```
//!
//! 子进程不是「重新启动一个 main」——内核把 `fork` 那一刻的完整用户寄存器帧复制
//! 给了它，所以子进程回到**这条 `fork()` 调用之后**继续执行（`pid == 0` 分支），
//! 与真实的 `fork` 语义一致（详见 `docs/process.md`）。
//!
//! ## 与 hello 的差异
//!
//! 不用堆（无 `Vec`/`String`/`format!`）：Shell 只需要固定缓冲，映像越小越好
//! （用户映像窗口 16 页 = 64 KiB，`kernel/build.rs` 会在构建期拒绝越界映像）。

#![no_std]
#![no_main]

use libc::syscall::ARG_MAX;
use libc::{
    close, env_count, env_var, execve, exit, fork, getpid, open, print, println, read, wait4, write,
    EAGAIN, O_RDONLY, STDIN,
};

/// 提示符三段：`用户名@主机名称 盘符>`。
const USER: &str = "root";
const HOST: &str = "cerlesse";
const DRIVE: &str = "C:";

/// 单行命令缓冲（内核行缓冲上限 128，含行尾 `\n`）。
const LINE_MAX: usize = 128;
/// 一行最多切出多少个参数（多出的丢掉并在报错里说明）。
const ARG_MAX_WORDS: usize = ARG_MAX;
/// `cat` 的读缓冲。
const CAT_BUF: usize = 128;
/// 解析出的 RAMFS 路径缓冲（`/bin/` + 命令名）。
const PATH_MAX: usize = 64;
/// 连续多少次 `-EAGAIN` 后退出（内核每次等 3 秒 → 空闲 6 秒）。
///
/// 这是**临时**行为：进程模型已经落地，但 `read(0)` 仍是非阻塞的
/// （`-EAGAIN`，见 `docs/syscall.md` 3.6），Shell 必须能自己退出，
/// 否则任何没有键盘输入的启动都会永远停在这里。
/// 值选 2 而不是 1：交互输入是分批到达的（一次击键一个 IRQ）。
const IDLE_LIMIT: u32 = 2;

/// 内核通过 `_start` 调用该入口；返回值即退出码。
#[no_mangle]
pub extern "C" fn main() -> i32 {
    println!("sh: Cerlesse interactive shell (v0.9: line input, builtins + external programs)");
    println!("sh: prompt is user@host drive>; type `help` for the builtin list");

    let mut line = [0u8; LINE_MAX];
    let mut idles = 0u32;

    loop {
        // 提示符一次 write 送出（日志里是 `user: write fd=1 len=17`）
        print!("{}@{} {}>", USER, HOST, DRIVE);

        let n = read(STDIN, &mut line);
        if n == EAGAIN {
            idles += 1;
            if idles >= IDLE_LIMIT {
                println!("sh: no input (2 idle windows), exiting");
                break;
            }
            continue;
        }
        if n <= 0 {
            // stdin 接通后不该出现（0 = 没有数据、<0 = 其他错误），留痕比静默退出好
            println!("sh: stdin read returned {}", n);
            break;
        }
        idles = 0;

        let text = match core::str::from_utf8(&line[..n as usize]) {
            Ok(text) => text.trim(),
            Err(_) => {
                println!("sh: line rejected (not utf-8)");
                continue;
            }
        };
        if text.is_empty() {
            continue; // 空行：只重打提示符
        }
        if !dispatch(text) {
            break;
        }
    }

    println!("sh: bye");
    0
}

/// 解析并执行一行。返回 `false` 表示请求退出 Shell。
fn dispatch(text: &str) -> bool {
    let mut args: [&str; ARG_MAX_WORDS] = [""; ARG_MAX_WORDS];
    let argc = split_args(text, &mut args);
    if argc == 0 {
        return true;
    }
    let cmd = args[0];
    let rest = &args[1..argc];

    match cmd {
        "help" => cmd_help(),
        "echo" => cmd_echo(rest),
        "cat" => {
            if rest.is_empty() {
                println!("sh: cat: missing file path (usage: cat <path>)");
            } else {
                cmd_cat(rest[0]);
            }
        }
        "pid" => println!("sh: pid={}", getpid()),
        "env" => cmd_env(),
        "exit" => return false,
        // 其余都当外部程序：解析成 `/bin/<cmd>`（或原样用绝对路径）后 fork/execve/wait4
        _ => cmd_external(cmd, rest),
    }
    true
}

/// 按空格切分参数（连续空格折叠；超过 [`ARG_MAX_WORDS`] 的部分丢弃并说明）。
fn split_args<'a>(text: &'a str, args: &mut [&'a str; ARG_MAX_WORDS]) -> usize {
    let mut argc = 0;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if argc == ARG_MAX_WORDS {
            println!("sh: too many arguments (max 7), extra ones ignored");
            break;
        }
        args[argc] = word;
        argc += 1;
    }
    argc
}

/// 内建命令列表 + 外部程序的说明。
fn cmd_help() {
    println!("sh: builtins (v0.9 step 3):");
    println!("sh:   help            - this list");
    println!("sh:   echo [args...]  - print arguments (argument splitting is real)");
    println!("sh:   cat <path>      - print a file through open/read/write/close");
    println!("sh:   pid             - pid of this shell (kernel-assigned)");
    println!("sh:   env             - print environment variables (inherited via execve)");
    println!("sh:   exit            - leave the shell loop");
    println!("sh: anything else runs as an external program via fork/execve/wait4");
    println!("sh:   /bin/hello      - external program (prints its pid + argv)");
    println!("sh:   <name>          - resolved to /bin/<name> (e.g. `ls` -> /bin/ls)");
    println!("sh: external programs (v0.9 step 4): ls, ps, cat, echo");
    println!("sh:   ls [path]       - list a directory through getdents");
    println!("sh:   ps              - print the process table through getprocs");
    println!("sh:   /bin/cat /bin/echo - the programs (builtins shadow the bare names)");
}

/// `echo`：逐个参数写出（每个参数一次 `write`，日志里能看到切分结果）。
fn cmd_echo(args: &[&str]) {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", arg);
    }
    println!();
}

/// `env`：打印本进程的环境变量（v1.0：`execve` 传递并默认继承 `envp`）。
///
/// 每一项来自内核写进用户栈的 `NAME=VALUE`；Shell 自己的环境由内核顶层运行
/// 驱动注入（`DEFAULT_ENV`），它 `fork` + `execve` 出来的子进程再原样继承。
fn cmd_env() {
    let mut i = 0;
    while let Some(bytes) = env_var(i) {
        match core::str::from_utf8(bytes) {
            Ok(text) => println!("sh: env {}", text),
            Err(_) => println!("sh: env <not utf-8>"),
        }
        i += 1;
    }
    println!("sh: env count={}", env_count());
}

/// `cat <path>`：走一遍真实的文件 syscall（open → read → write → close）。
fn cmd_cat(path: &str) {
    let fd = open(path, O_RDONLY);
    if fd < 0 {
        println!("sh: cat: cannot open {} (errno {})", path, fd);
        return;
    }
    let fd = fd as i32;
    let mut buf = [0u8; CAT_BUF];
    let mut total = 0usize;
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            println!("sh: cat: read failed (errno {})", n);
            break;
        }
        if n == 0 {
            break; // EOF
        }
        let n = n as usize;
        let written = write(1, &buf[..n]);
        if written != n as isize {
            println!("sh: cat: short write ({} of {})", written, n);
            break;
        }
        total += n;
    }
    let _ = close(fd);
    println!("sh: cat {} bytes={}", path, total);
}

/// 外部命令：`fork` → 子进程 `execve` → 父进程 `wait4`。
///
/// 命令名解析规则：以 `/` 开头就用它自己，否则拼成 `/bin/<cmd>`。
/// 解析或 `execve` 失败时，子进程打印 errno 并以 127 退出（与真实 shell 一致），
/// 父进程仍然会把退出码报出来 —— 所以「程序不存在」在这个 Shell 里是一等公民：
///
/// ```text
/// sh> ls
/// user: fork parent=3 -> child pid=4
/// user: run child pid=4 name=fork-child
/// user: exec /bin/ls -> ENOENT (no such program)
/// sh: exec /bin/ls failed errno=-2
/// user: exit code=127
/// user: reaped pid=4 exit=127
/// sh: ls: exit code=127
/// ```
fn cmd_external(cmd: &str, args: &[&str]) {
    let mut path_buf = [0u8; PATH_MAX];
    let Some(path) = resolve(cmd, &mut path_buf) else {
        println!("sh: {}: command name too long", cmd);
        return;
    };

    let pid = fork();
    if pid == 0 {
        // 子进程：换映像。argv[0] 用命令名本身（与真实 shell 一致）。
        let mut argv: [&str; ARG_MAX] = [""; ARG_MAX];
        argv[0] = cmd;
        let n = (args.len() + 1).min(ARG_MAX);
        for i in 1..n {
            argv[i] = args[i - 1];
        }
        println!("sh: child pid={} exec {} (argc={})", getpid(), path, n);
        let rc = execve(path, &argv[..n]);
        // 只有 execve 失败才会走到这里
        println!("sh: exec {} failed errno={}", path, rc);
        exit(127);
    }
    if pid > 0 {
        let code = wait4(pid as i64);
        println!("sh: {}: exit code={}", cmd, code);
        return;
    }
    println!("sh: {}: fork failed errno={}", cmd, pid);
}

/// 把命令名解析成 RAMFS 路径：`/foo` 原样；`foo` → `/bin/foo`。
fn resolve<'a>(cmd: &str, buf: &'a mut [u8; PATH_MAX]) -> Option<&'a str> {
    const PREFIX: &[u8] = b"/bin/";
    let (prefix, name): (&[u8], &[u8]) = if cmd.starts_with('/') {
        (b"", cmd.as_bytes())
    } else {
        (PREFIX, cmd.as_bytes())
    };
    if prefix.len() + name.len() > buf.len() {
        return None;
    }
    buf[..prefix.len()].copy_from_slice(prefix);
    buf[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
    core::str::from_utf8(&buf[..prefix.len() + name.len()]).ok()
}
