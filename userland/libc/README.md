# libc（最小用户态运行时）

- 版本：v0.6 建立 / **v0.7 补齐文件 syscall** / **v0.9 第一步补齐用户堆** / **v0.9 第二步补齐 `stdin`** /
  **v0.9 第三步补齐进程调用与 `argc`/`argv`** / **v1.0 第二步补齐 `envp` 与 `mmap` 的 `offset`** /
  **v1.0 第五步补齐可写 / 共享文件映射与 `msync`** /
  **纯内核化：随用户态移入独立 workspace**（现位于 `userland/libc/`）
- 状态：✅ 已实现（2026-10-11，随 `make test-user` **61 项** + `make test-shell` **11 项** + `make test-proc` **8 项 + 4 正则**断言实测通过；
  `user/hello` 作为 RAMFS 上的 `/init`、`user/shell` 作为 `/bin/sh`、`user/hello-bin` 作为 `/bin/hello`
  （**由 Shell `fork` + `execve` 起来**），三者都经 ELF Loader 装载并跑在 ring3）
- 关联：[docs/syscall.md](../../docs/syscall.md) 2.11 / 3.1 / 3.5 / 3.6 / 3.7、[user/hello](../user/hello)、
  [user/shell](../user/shell)、[user/hello-bin](../user/hello-bin)、[docs/process.md](../../docs/process.md) 1.4、
  [docs/filesystem.md](../../docs/filesystem.md) 2.6、[userland/README.md](../README.md)（工作区与构建入口）

## 1. 这是什么

`libc` 把内核 ABI 封装成用户程序可以直接调用的形状，让新用户程序**不必再手写机器码**。
它是用户态脚手架的底座：`libc`（运行时）+ `user/*`（用户程序）+ `userland/Makefile`
（构建 + `tools/check_user_elf.py` 校验 + 写 `manifest.txt`）+ 内核按清单内嵌与 ELF Loader（`syscall/elf.rs`）。

| 组成 | 内容 |
| ---- | ---- |
| `syscall` | 系统调用号常量 + 裸入口 `raw(nr, a1..a5)`（等价 `raw6(nr, a1..a6)`，第 6 参数 `r9`）+ `write` / `read` / `open` / `close` / `getpid` / `nanosleep` / `exit` / `brk` / `mmap`（含 `mmap_with_offset` 与文件映射 `mmap_file`）/ `munmap` / `msync`（v1.0 第五步）/ `fork` / `execve`（含 `execve_env`）/ `wait4` / `socket` / `bind` / `sendto` / `recvfrom` / `net_config`（v1.0 第四步） |
| `args` | `argc()` / `arg(i)` / `arg_str(i)` / `Args`：入口寄存器里的 `rdi`/`rsi` 由 `_start` 存进来（v0.9 第三步） |
| `env` | `env_count()` / `env_var(i)` / `getenv(name)` / `env::Env`：入口寄存器里的 `rdx`/`r10`（`_start` 把 `r10` 搬进 `rcx`）由 `cer_start` 存进来；`execve` 默认继承（v1.0 第二步） |
| `heap` | `#[global_allocator]`（bump over `mmap`，64 KiB 块）—— 用户程序因此可以用 `Vec`/`String`/`Box`（v0.9 第一步） |
| `socket` | `SockAddrIn`（`new`/`any`/`port`/`addr`）与 `NetConfig`（`ip`/`gateway`/`mac`）：16 字节 `sockaddr_in` 与 `net_config` 记录的用户态形状；端口/地址用网络字节序（v1.0 第四步） |
| `console` | `print!` / `println!`（固定 256 字节缓冲，**一次宏调用 = 一次 `write`**） |
| `_start` | `global_asm!` 放在 `.text.start`：`_start` → 用户 `main` → `exit(main())` |
| 运行时支撑 | `#[panic_handler]`（打 stderr 后 `exit(101)`）、`no_std`、errno / `O_*` 常量 |

## 2. 用法

```rust
// user/<程序名>/src/main.rs
#![no_std]
#![no_main]

use libc::{close, open, println, read, write, O_CREAT, O_RDONLY, O_WRONLY};

#[no_mangle]
pub extern "C" fn main() -> i32 {
    println!("hello from user space"); // 一次 write（fd=1）

    // 读内核提供的文件（RAMFS）
    let fd = open("/etc/motd", O_RDONLY);   // fd ≥ 3，或负 errno
    if fd >= 0 {
        let mut buf = [0u8; 64];
        let n = read(fd as i32, &mut buf);  // 0 = EOF
        if n > 0 {
            let _ = write(1, &buf[..n as usize]);
        }
        let _ = close(fd as i32);
    }

    // 写文件（内核随后可从 RAMFS 读回，见 `fs: readback ...`）
    let wfd = open("/tmp/note.txt", O_CREAT | O_WRONLY);
    if wfd >= 0 {
        let _ = write(wfd as i32, b"written by user program\n");
        let _ = close(wfd as i32);
    }

    0 // 退出码
}
```

文件 syscall 的形状（与 Linux 的差异写在类型里）：

| 调用 | 参数 | 返回 |
| ---- | ---- | ---- |
| `open(path: &str, flags: i32)` | 路径用**指针 + 长度**（内核已校验区间，不在不可信内存上找 NUL）；`flags` = `O_RDONLY`/`O_WRONLY`/`O_RDWR` 与 `O_CREAT` 的组合 | fd（≥ 3）或负 errno |
| `read(fd, buf: &mut [u8])` | `buf` 必须对 ring3 **可写**（栈或 `RW` 段） | 读到字节数（0 = EOF）或负 errno |
| `write(fd, buf: &[u8])` | fd 1/2 写串口，fd ≥ 3 写文件 | 写出字节数或负 errno |
| `close(fd)` | 只接受 fd ≥ 3（0/1/2 是保留位） | 0 或负 errno |

判定错误请用 `libc` 里的 errno 常量（`ENOENT`/`EBADF`/`EAGAIN`/`EFAULT`/`EISDIR`/`EINVAL`/`ENOSPC`/`ENOMEM`/`ENOSYS`）。\
**`read(0)` 是行缓冲（cooked）**：内核交付**一整行**（行尾统一为 `\n`），所以 `cat` 之类的程序
不必自己拼行；没有整行的 3 秒内返回 `-EAGAIN`(-11)（无进程模型时内核不会无限等，
因此交互程序应该把 `EAGAIN` 当作“暂无输入”而不是错误）。\
**寄存器约定**：与 Linux syscall ABI 一致 —— 一次往返后只有 `rax`（返回值）、`rcx`、`r11` 变化，
其余寄存器保持进入前的值（内核入口汇编会保存/恢复 `rdi/rsi/rdx/r10/r8/r9`）。

## 3. 新增一个用户程序

1. 复制 `user/hello/`（`Cargo.toml` + `build.rs` + `user.ld` + `src/main.rs`）并改包名；
2. 在 `userland/Cargo.toml` 的 `members` 里加上 `"user/<程序名>"`；
3. 在 `userland/Makefile` 的 `PROGRAMS` 里加上包名、在 recipe 里 `cp` 出 ELF，
   并在 `manifest.txt` 里加一行 `P <RAMFS 路径> <文件名>`（内核按清单写进 RAMFS，**不认识程序名**）；
4. 单独验证脚手架：`make -C userland images` —— 编译 + 链接 + `tools/check_user_elf.py` 检查段布局。

> 注意：内核侧**不需要**改任何东西（这正是纯内核化的目的）——只要清单里的路径
> 出现在 `kernel/src/syscall/user.rs` 的契约常量里就保持一致，否则 `selftest` 会
> 明确报出缺哪条路径。

构建参数不是可选的（改错会在链接期或 ring3 立刻炸）：

| 参数 | 取值 | 原因 |
| ---- | ---- | ---- |
| 目标 | `x86_64-unknown-none` | 裸机用户态，无宿主 libc |
| 代码模型 | `-C code-model=large` | 链接地址 2^39 超出 small 模型 ±2GiB，否则 `R_X86_64_32S out of range` |
| 重定位模型 | `-C relocation-model=static` | 内核不做加载时重定位（ELF Loader 只接受 `ET_EXEC`） |
| profile | `user-release`（`userland/Cargo.toml` 定义） | large 模型与 LTO 不兼容，该 profile `lto = false`、`opt-level = 2` |
| 链接脚本 | `user.ld`（由各程序的 `build.rs` 传入） | 固定链接到 `USER_VIRT_BASE`，每个段 `ALIGN(0x1000)` |

## 4. 四条硬约束（都有实测理由）

1. **每个段必须页对齐**（`user.ld` 里 `ALIGN(0x1000)`）。v0.7 的 ELF Loader 按**页**设置权限
   （`W` → 可写、非 `X` → `NX`）；同页既 `W` 又 `X` 会让 `W^X` 退化，装载器直接拒绝：
   `user: ELF rejected: W and X segments share a page`；
2. **必须固定链接到 `USER_VIRT_BASE`（0x0000_0080_0000_0000）**：链接地址 = 运行地址，
   程序里的绝对地址因此不需要重定位（内核不做加载时重定位）；
3. **静态变量必须归入 `.bss`**：链接在 2^39（large 代码模型）时 rustc 把零初始化静态量放进
   按名字命名的 `.lbss.<symbol>` 段，`user.ld` 必须显式把它们收进 `.bss`，否则它们会变成
   **孤儿段**被 lld 塞在 `.data` 之后，RW 段不再页对齐（实测落在 `0x8000003040`）；
4. **内存窗口是固定布局**：映像（`.text`/`.rodata`/`.data`/`.bss`）上限 64 KiB、栈 16 KiB，
   v0.9 第一步又加了 **brk 堆窗口 256 KiB** 与 **mmap 窗口 256 KiB**（匿名 + 文件映射，见第 5 节）；
   超出窗口的 `brk`/`mmap` 会失败（`brk` 返回原值、`mmap` 返回 `-ENOMEM`）；
   窗口内的保护属性可以用 `mprotect` 在运行期改写（v1.0 第一步，见第 5.1 节），
   但**用户栈的保护属性固定**（`RW` + `NX`）且不能改。

`make user` 会跑 `tools/check_user_elf.py`，逐项检查上述 1/2（以及映像窗口、无重定位），
不用启动 QEMU 就能在改坏链接脚本时立刻发现。

尚未实现的调用：**号已定义的调用已全部落地**（`mprotect` 是最后一个，见 5.1）；
**匿名**共享映射（`MAP_SHARED|MAP_ANONYMOUS`）、用户态抢占与**完整**动态链接属后续版本。

## 5. 用户堆（v0.9 第一步）

```rust
use libc::{brk, mmap, munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use alloc::vec::Vec;   // 用户程序里 `extern crate alloc;`

let base = brk(0);                       // 查询当前堆顶
let grown = brk(base + 8192);            // 增长两页（页对齐）
let p = base as *mut u8;                 // 新页保证全零，可直接写
unsafe { core::ptr::write_volatile(p, 7) };
println!("brk {} -> {}", base, grown);

let m = mmap(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS); // >0 = 地址，<0 = -errno
let _ = munmap(m as u64, 4096);

let mut v: Vec<u32> = Vec::new();        // 全局分配器在 heap.rs，走 mmap
v.push(1);
```

内核侧语义（完整版见 [docs/syscall.md](../../docs/syscall.md) 3.5）：

| 调用 | 语义 | 失败 |
| ---- | ---- | ---- |
| `brk(addr)` | `addr = 0` 查询堆顶；否则页对齐增长/收缩（256 KiB 窗口） | **不返回错误**：返回原值（用 `ret < addr` 判断） |
| `mmap(len, prot, flags)` / `mmap_with_offset(len, prot, flags, offset)` | 匿名映射：`MAP_PRIVATE\|MAP_ANONYMOUS`、`fd` 固定 -1、`offset=0`；first-fit；新页全零 | `-EINVAL`（位非法/`PROT_WRITE\|PROT_EXEC`/非 0 `offset`）、`-ENOMEM`（超窗口） |
| `mmap_file(fd, len, prot, flags, offset)` | **文件映射**（v1.0 第三步只读 / 第五步可写）：`fd ≥ 3` 可读、`flags` = `MAP_PRIVATE`（改动不落盘）或 `MAP_SHARED`（`msync`/`munmap` 写回）、`prot` 不含 `PROT_EXEC`、`offset` 页对齐；把文件 `[offset, offset+len)` 与文件的交集映射进来 | `-EINVAL`（非页对齐 `offset`/`PROT_EXEC`/`MAP_SHARED\|MAP_ANONYMOUS`）、`-EBADF`（`MAP_SHARED\|PROT_WRITE` 但 fd 只读）、`-ENOMEM`（描述符用尽） |
| `msync(addr, len, flags)` | 把 `MAP_SHARED` 文件页写回文件（v1.0 第五步）：页对齐、全段已映射；非共享页跳过；`MS_SYNC`/`MS_ASYNC`（等价）、`MS_INVALIDATE` 忽略；写回长度夹在映射那一刻的文件大小内 | `-EINVAL`（位/对齐/长度/有页未映射），`vfs::write` 的 `-errno`（只读文件系统等） |
| `mprotect(addr, len, prot)` | 改**已映射**页的保护属性（v1.0）：页对齐、同一窗口、全段已映射；`PROT_NONE` 关页但留住页帧 | `-EINVAL`（对齐/长度/位/跨窗口/有页未映射/目标为栈） |
| `munmap(addr, len)` | 整段归还（要求页对齐且全段已映射） | `-EINVAL` |

分配器的**限制**（写在代码注释里，也写在这里）：`libc::heap` 是只前进的 bump 分配器，
`dealloc` 是空操作 —— 目前靠「一次运行结束由内核 `UserSpace::Drop` 整棵释放」兜底，
长驻程序的内存占用会单调上升；回收式分配器属后续版本（`mprotect` 已在 5.1 落地）。

### 5.1 `mprotect`：运行期改保护属性（v1.0 第一步）

```rust
use libc::{mmap, mprotect, munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE};

// W^X 只允许「先写、再换只执行」：匿名页先拿 RW，写完改成 RX 再调用
let p = mmap(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS) as usize;
let code = [0xb8, 42, 0, 0, 0, 0xc3]; // mov eax, 42; ret
unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, code.len()) };

let _ = mprotect(p as u64, 4096, PROT_NONE);   // 0 返回：页帧还在，但本进程访问会 #PF
let _ = mprotect(p as u64, 4096, PROT_READ | PROT_EXEC); // 换成只读可执行
let f: extern "C" fn() -> u32 = unsafe { core::mem::transmute(p) };
println!("ret={}", f());            // 42
let _ = munmap(p as u64, 4096);     // mprotect 过的页照常归还
```

- 成功返回 `0`（内核串口留 `user: mprotect addr=… -> pages=N`）；失败返回 `-22`（`EINVAL`），
  且每类原因在串口上可区分（未页对齐 / `len=0` / 未知保护位 / `W^X` / 跨窗口 / 有页未映射）；
- `PROT_NONE` **不归还页帧**：它只是把这一段的 `USER` 位清掉，内容原样保留，`mprotect` 回来又能用。
  注意此时**内核也不再把它当合法用户缓冲** —— 拿它去 `write`/`read` 会得到 `-EFAULT`；
- `PROT_WRITE | PROT_EXEC` 依旧被拒（W^X 不能被 `mprotect` 绕开）。

## 6. 交互式输入（v0.9 第二步）

`read(0)` 是**行缓冲**：内核的行缓冲（`driver/keyboard.rs`）已经做了行编辑与回显，
一次 `read(0)` 给你一整行（行尾是 `\n`，`\r` 不会出现）。没有整行时等待，但**最多等 3 秒**
后返回 `EAGAIN` —— 所以在 `-EAGAIN` 上要能继续循环（而不是当成错误退出）：

```rust
use libc::{print, println, read, EAGAIN, STDIN};

let mut line = [0u8; 128];
let mut idles = 0u32;
loop {
    print!("root@cerlesse C:>");            // 一次 write（日志里可见 len=17）
    let n = read(STDIN, &mut line);
    if n == EAGAIN {
        // 暂无输入：交互式程序应继续等；这里用连续两次空闲退出来避免卡住整机
        idles += 1;
        if idles >= 2 { break; }
        continue;
    }
    if n <= 0 { break; }                   // 其他错误（0 在 stdin 接通后不会出现）
    idles = 0;
    let text = core::str::from_utf8(&line[..n as usize]).unwrap_or("").trim();
    println!("got: {}", text);
}
```

参考实现是 [user/shell](../user/shell)（提示符、参数切分、内建命令、外部命令走的 `fork`+`execve`+`wait4`）。
外部程序现在真的能跑起来 —— 见第 7 节。

## 7. 进程调用与命令行参数（v0.9 第三步）

三个包装的后端是真正的进程对象（独立地址空间 + 独立内核栈 + 停在 `syscall` 里的完整
寄存器帧），内核侧实现见 [docs/process.md](../../docs/process.md) 1.4：

| 调用 | 返回 | 说明 |
| ---- | ---- | ---- |
| `fork() -> isize` | 父进程：子进程 PID；子进程：`0`；失败：负 errno（进程表满 `EAGAIN`） | 地址空间与**当前寄存器帧**一起复制，子进程从同一条用户指令继续 |
| `execve(path, argv: &[&str]) -> isize` | **成功不返回**；失败：`-ENOENT` / `-E2BIG` / `-EFAULT` / `-EINVAL` | 换映像 + 换地址空间，`argv` 与**当前环境**（`libc::env`）写进新进程的用户栈（默认继承） |
| `execve_env(path, argv: &[&str], envp: &[&str]) -> isize` | 同上 | 用 `envp` 显式替换新映像的环境（`NAME=VALUE`） |
| `wait4(pid: i64) -> isize` | 子进程退出码（可为负）；非直接子进程：`ECHILD` | 运行并**回收**子进程（地址空间整棵归还） |

标准写法原样可用（`user/shell` 的 `cmd_external` 就是这么写的）：

```rust
use libc::{execve, exit, fork, wait4};

let pid = fork();
if pid == 0 {
    let _ = execve("/bin/hello", &["hello", "world"]); // 成功不返回
    exit(127);                                          // 失败：exec 的常规退出码
}
let code = wait4(pid);
println!("exit code={}", code);
```

**参数与环境在入口寄存器里**（不是 Linux 的「压在用户栈顶」）：`_start` 的 `rdi = argc`、
`rsi = argv`、`rdx = envc`、`r10 = envp`（`_start` 把 `r10` 搬进 `rcx` 再 `call cer_start`），
由 `cer_start` 分别存进 `libc::args` 与 `libc::env`。用户程序侧：

```rust
use libc::{arg_str, argc, println};

println!("argc={}", argc());
for i in 0..argc() {
    println!("argv[{}]={}", i, arg_str(i).unwrap_or("<non-utf8>"));
}
```

字符串由内核写在**新进程的用户栈**上（`syscall::process::build_user_frame`），
生命周期与进程相同，所以 `arg(i)` 给的是 `&'static [u8]`；上限 `ARG_MAX` = 8 项、
单项 256 字节。

**与 Linux 的调度差异**（刻意，且对调用方透明）：单 CPU、用户态无抢占，`fork` 之后
子进程**不会立刻运行**，它要等父进程 `wait4` 时才被恢复 —— 也就是「父进程在等的时候，
子进程在跑」。因此 `fork`/`execve`/`wait4` 的标准写法不需要为这个内核改写。

## 8. 目录与进程表枚举（v0.9 第五步）

两个「读内核表」的调用。它们都往**调用方提供的缓冲**里写**定长记录**，并返回写入的
记录数（装不下就截断，每条都完整）；偏移不要自己算 —— 本层提供解析器，
权威定义在内核侧（`syscall/dir.rs` 与 `syscall/process.rs` 的模块文档）。

| 调用 | 记录 | 解析器 |
| ---- | ---- | ---- |
| `getdents(path, buf) -> isize` | 88 字节：`kind`(1) + 保留(7) + `size`(u64) + `name_len`(u16) + 保留(6) + `name`(64) | `dirent_name(buf, i)` / `dirent_is_dir(buf, i)` / `dirent_size(buf, i)` |
| `getprocs(buf) -> isize` | 40 字节：`pid`(u64) + `ppid`(u64) + `state`(1) + 保留(7) + `comm`(16) | `procinfo_pid(buf, i)` / `procinfo_ppid(buf, i)` / `procinfo_state(buf, i)` / `procinfo_name(buf, i)` |

参考实现是 [user/ls](../user/ls)（列目录）与 [user/ps](../user/ps)（列进程）：

```rust
use libc::{dirent_is_dir, dirent_name, dirent_size, getdents, println, DIRENT_SIZE};

let mut buf = [0u8; 1024];
let n = getdents("/", &mut buf);
if n < 0 {
    println!("ls: errno {}", n);
} else {
    for i in 0..n as usize {
        let kind = if dirent_is_dir(&buf, i) { "dir" } else { "file" };
        println!("{} {} {}", dirent_name(&buf, i), kind, dirent_size(&buf, i));
    }
}
let _ = DIRENT_SIZE;
```

注意两件事：

1. 缓冲至少要有**一条记录**那么大，否则内核返回 `-EINVAL`（而不是静默返回 0 ——
   那会让「缓冲太小」与「空目录」混为一谈）；
2. `getprocs` 枚举的是**进程表槽位顺序**，不承诺按 PID 排序（`user/ps` 因此不做排序）。
   `getprocs` 是 **Cerlesse 扩展号段（编号 256）** 的调用：Linux x86_64 没有它
   （它的 `ps` 读 `/proc`，而本内核的 VFS 还没有虚拟文件）。

## 9. 存储卷枚举（CSS-1）

`storage_volumes` 把内核的**卷表**写成定长记录（64 字节/条），是 `getdents`/`getprocs` 同一套做法：
调用方给缓冲，返回写入条数；`total` 出参用来先问「总共有多少条」再决定缓冲。
偏移不要自己算 —— 本层提供解析器，权威定义在内核侧（`storage/volume.rs`）。

| 调用 | 记录 | 解析器 |
| ---- | ---- | ---- |
| `storage_volumes(buf, out_count: Option<&mut u64>) -> isize` | 64 字节：`id`(16) + `disk`(u16) + `part`(u8) + `fs`(u8) + `read_only`(u8) + `health`(u8) + `flags`(u16) + `letter`(u8) + `label`(11) + `uuid`(4) + `total_bytes`(u64) + `free_bytes`(u64) + `mounts`(u32) | `volume_id(buf, i)` / `volume_disk` / `volume_part` / `volume_fs` / `volume_read_only` / `volume_health` / `volume_label` / `volume_total_bytes` / `volume_free_bytes` / `volume_mounts` |

参考实现是 [user/volumes](../user/volumes)：

```rust
use libc::{storage_volumes, println, volume_fs, volume_health, volume_label};

let mut buf = [0u8; 64 * 8];
let mut total = 0u64;
let n = storage_volumes(&mut buf, Some(&mut total));
if n < 0 {
    println!("volumes: errno {}", n);
} else {
    for i in 0..n as usize {
        println!("{} {} {}", volume_label(&buf, i), volume_fs(&buf, i), volume_health(&buf, i));
    }
}
```

注意：缓冲至少要有**一条记录**（64 字节），否则内核返回 `-EINVAL`；
枚举顺序是**确定性的**（`disk` 升序 → `part` 升序 → `id` 字节序升序），因此同一镜像两次调用逐字节相同。


## 10. UDP 套接字（v1.0 第四步）

`socket` / `bind` / `sendto` / `recvfrom` 是 BSD 风格的 UDP 最小集（内核只支持 `AF_INET` +
`SOCK_DGRAM`，见 `docs/syscall.md` 3.14）。地址用 16 字节 `sockaddr_in`，
端口与地址是**网络字节序**；本层给 [`SockAddrIn`] 与 [`NetConfig`] 两个类型，不要自己按偏移拼字节。

| 调用 | 说明 |
| ---- | ---- |
| `socket(domain, ty, protocol) -> isize` | `AF_INET` + `SOCK_DGRAM`；返回 fd（与文件共用编号空间） |
| `bind(fd, &SockAddrIn) -> isize` | `SockAddrIn::any(0)` 分配临时端口；端口被占 `EADDRINUSE` |
| `sendto(fd, buf, &SockAddrIn) -> isize` | 返回**载荷字节数**；套接字未绑定时隐式分配端口 |
| `recvfrom(fd, buf, Some(&mut SockAddrIn)) -> isize` | 回填发送方地址；2 秒无数据 `EAGAIN`，未绑定 `EINVAL` |
| `net_config(&mut NetConfig) -> isize` | 内核 DHCP 协商到的本机地址/网关/MAC；无网卡 `ENODEV` |

参考实现是 [user/hello](../user/hello) 的 `socket_selftest`：

```rust
use libc::{socket, bind, sendto, recvfrom, net_config, SockAddrIn, NetConfig, AF_INET, SOCK_DGRAM};

let mut cfg = NetConfig::new();
if net_config(&mut cfg) < 0 {
    // 没有网卡：跳过网络部分（不当作失败）
}
let fd = socket(AF_INET, SOCK_DGRAM, 0);
let _ = bind(fd as i32, &SockAddrIn::any(0));
let dst = SockAddrIn::new([10, 0, 2, 2], 9);
let _ = sendto(fd as i32, b"ping", &dst);
let mut buf = [0u8; 64];
let mut src = SockAddrIn::default();
let n = recvfrom(fd as i32, &mut buf, Some(&mut src));
```

`read`/`write` 一个 UDP 套接字返回 `ENOTCONN`（本内核没有 `connect`）—— 用 `sendto`/`recvfrom`。
TCP、`connect`/`listen`/`accept`、`MSG_*` 尚未实现（明确拒绝）。
