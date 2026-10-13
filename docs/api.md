# API 参考（系统调用 / 用户态 / 内核内部 / 主机工具）

- 阶段：第八阶段（v0.9 进行中）
- 最后更新：2026-10-11
- 状态：🟡 持续更新 —— 本文汇总**当前代码实际提供**的对外接口：
  系统调用 ABI、用户态 `libc` API、内核内部 trait、主机侧工具 CLI 与 Makefile 目标。
  接口随版本演进，**以本文与源码注释为准**；设计背景见 [syscall.md](./syscall.md)、
  [process.md](./process.md)、[filesystem.md](./filesystem.md)、[drivers.md](./drivers.md)。

> 本文档顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。
> 约定：**负数返回值 = `-errno`**（`-2` = `-ENOENT`）。Rust 侧错误一律用负数，
> 中间层不做翻译，`errno` 常量见第 3 节。

## 1. 系统调用 ABI（x86_64）

### 1.1 寄存器约定

| 项 | 约定 |
| ---- | ---- |
| 指令 | `syscall`（`EFER.SCE` + `IA32_STAR` / `IA32_LSTAR` / `IA32_FMASK`），返回 `sysretq` |
| 调用号 | `rax` |
| 参数 | `rdi, rsi, rdx, r10, r8, r9`（前 5 个与 Linux x86_64 一致；`r9` 是 v1.0 新增的第 6 个参数：`mmap` 的 `offset`、`execve` 的 `envp`） |
| 返回值 | `rax`；**负数 = `-errno`** |
| 被破坏的寄存器 | 仅 `rax`、`rcx`、`r11`；其余寄存器（含 `rdi/rsi/rdx/r10/r8/r9`）**保持进入前的值**（入口汇编保存/恢复） |
| 入口栈 | 内核入口切到**专用系统调用栈**（`syscall` 指令不切栈）；返回前恢复 |

内核入口与分发见 `kernel/src/syscall/trampoline.rs`；**调用号与号段规则的权威定义在 `abi/src/syscall.rs`**
（纯内核化阶段三的单一事实源），内核 `kernel/src/syscall/syscall_table.rs` 只是再导出 + 排表。
用户态封装见 `libc/src/syscall.rs`。

### 1.2 调用号总表

编号沿用 **Linux x86_64** 语义，便于将来直接运行现成用户态程序。
`0..=255` 沿用 Linux 语义，`256..` 是 Cerlesse 扩展号段（256 = `getprocs`，257–260 是 CKM 号段
—— 257/258/259 已在 CKM-1 实现，260 预留给 CKM-4；261–266 由 CSS 使用，其中 261/262/263/264/265
已实现、只有 266 `storage_format` 待 CSS-4；267 = `net_config`，v1.0 新增）；`26 msync` 同样在 v1.0 第五步从 ENOSYS 桩上摘下来；号段上限 `SYS_MAX = 268`；
**号越界**与**号合法但未实现**是两种可区分的失败（前者 `parse` 返回 `None`，后者落在公共 ENOSYS 桩，均返回 `-ENOSYS`）。

| 号 | 名称 | 状态 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | `read` | ✅ | 从 fd 读入缓冲；fd 0 为行缓冲 stdin |
| 1 | `write` | ✅ | fd 1/2 写串口，fd ≥ 3 写文件 |
| 2 | `open` | ✅ | 打开/创建文件，返回 fd（≥ 3） |
| 3 | `close` | ✅ | 关闭动态 fd |
| 9 | `mmap` | ✅ | 匿名私有映射；第 6 参数 `offset`（`r9`）+ 文件映射（`MAP_PRIVATE` 可写不落盘 / `MAP_SHARED` 写回，v1.0） |
| 26 | `msync` | ✅ | 把 `MAP_SHARED` 文件映射的页写回文件（v1.0 第五步） |
| 10 | `mprotect` | ✅ | 改写**已映射**页的保护属性（`PROT_NONE` = ring3 不可访问但帧保留） |
| 11 | `munmap` | ✅ | 整段归还映射（共享文件页先写回；共享映射必须整段归还） |
| 12 | `brk` | ✅ | 调整用户堆顶（Linux 语义） |
| 35 | `nanosleep` | ✅ | 睡眠 N 个 PIT tick（100Hz） |
| 39 | `getpid` | ✅ | 当前进程 PID |
| 41 | `socket` | ✅ | 建 UDP 套接字（`AF_INET` + `SOCK_DGRAM`），返回 fd（v1.0） |
| 44 | `sendto` | ✅ | 发一个 UDP 数据报（返回载荷字节数，v1.0） |
| 45 | `recvfrom` | ✅ | 收一个 UDP 数据报并回填发送方地址（v1.0） |
| 49 | `bind` | ✅ | 绑本地地址/端口（`port = 0` 分配临时端口，v1.0） |
| 57 | `fork` | ✅ | 复制当前进程 |
| 59 | `execve` | ✅ | 换映像（成功后不返回） |
| 60 | `exit` | ✅ | 结束进程并交回退出码 |
| 61 | `wait4` | ✅ | 运行/回收直接子进程，返回退出码 |
| 78 | `getdents` | ✅ | 目录枚举，定长 88 字节记录（v0.9 第五步） |
| 256 | `getprocs` | ✅ | 进程表快照，定长 40 字节记录（v0.9 第五步，扩展号段） |
| 257 | `ckm_policy_query` | ✅ | 当前凭证对某 op 的判定（`0`/`-EPERM`）+ 定长 48 字节记录（CKM-1） |
| 258 | `ckm_audit_read` | ✅ | 读审计环（`seq > since_seq`），定长 48 字节记录；需 `CAP_AUDIT_READ`（CKM-1） |
| 259 | `ckm_component_state` | ✅ | 组件表快照，定长 64 字节记录，按确定性顺序（CKM-1） |
| 260 | `ckm_policy_set` | ENOSYS | 改 CKM 策略（CKM-4；阶段 1 的授予面为零） |
| 261 | `storage_volumes` | ✅ | 卷表快照，定长 64 字节记录（CSS-1，只读、无需能力位） |
| 262 | `storage_mount` | ✅ | 把卷挂到路径（扩展号段；需 `CAP_STORAGE_ADMIN`，CSS-3） |
| 263 | `storage_unmount` | ✅ | 卸载别名，卷仍在卷表（需 `CAP_STORAGE_ADMIN`，CSS-3） |
| 264 | `storage_assign_letter` | ✅ | 分配/取消盘符（需 `CAP_STORAGE_ADMIN`；映射落 CKM 状态区 `storage/letters`，CSS-3） |
| 265 | `storage_rescan` | ✅ | 重扫设备的分区与文件系统（CSS-1；设备不存在 `-ENODEV`） |
| 266 | `storage_format` | ENOSYS | CSS-4 待实现（扩展号段） |
| 267 | `net_config` | ✅ | 导出 DHCP 协商到的本机地址/网关/MAC，定长 16 字节记录（v1.0，扩展号段） |
| 其他 | — | ENOSYS | 号合法但未实现：查表命中 ENOSYS 桩；常见者会打印 `syscall: ENOSYS nr=… name=…` |

## 2. 系统调用逐条说明

参数顺序即寄存器顺序 `rdi, rsi, rdx, r10, r8, r9`（`r9` = 第 6 参数，v1.0 起）。

### 2.1 `read`（nr=0）

```
isize read(i32 fd, *mut u8 buf, usize count)   // libc: read(fd: i32, buf: &mut [u8]) -> isize
```

- `buf` 必须落在**当前用户地址空间**内且对 ring3 **可写**，否则 `-EFAULT`。
- `fd = 0`（stdin）：**行缓冲（cooked）** —— 交付**一整行**（行尾统一 `\n`），
  无整行时等待，超过 `STDIN_IDLE_TICKS = 300`（PIT 100Hz → **3 秒**）返回 `-EAGAIN`(-11)。
  交互程序应把 `EAGAIN` 当作「暂无输入」而非错误。
- `fd ≥ 3`：从文件读，返回字节数，**0 = EOF**。
- 错误：`EBADF`（fd 无效/模式不符）、`EFAULT`、`EAGAIN`（仅 stdin）、`EISDIR`、`EINVAL`。

### 2.2 `write`（nr=1）

```
isize write(i32 fd, const u8 *buf, usize count) // libc: write(fd: i32, buf: &[u8]) -> isize
```

- `buf` 必须可读（按页校验）。
- `fd = 1` / `2`：写串口（stdout/stderr 均接串口），返回写入字节数。
- `fd ≥ 3`：写文件。
- 错误：`EBADF`、`EFAULT`、`EINVAL`、`ENOSPC`（超过 RAMFS 单文件上限）。

### 2.3 `open`（nr=2）

```
isize open(const u8 *path, usize path_len, i32 flags)  // libc: open(path: &str, flags: i32) -> isize
```

- 路径用**指针 + 长度**传递（内核已校验区间，不在不可信内存上找 NUL）。
- `flags` = `O_RDONLY`(0) / `O_WRONLY`(1) / `O_RDWR`(2) 与可选的 `O_CREAT`(0o100) 组合；
  未知位返回 `EINVAL`。
- 返回 fd **≥ 3**（取最小空闲槽位），或负 errno。
- 错误：`ENOENT`、`ENOTDIR`、`EISDIR`、`EINVAL`、`ENOSPC`（fd 用尽，`FD_MAX = 16`）、`EROFS`（只读后端）。

### 2.4 `close`（nr=3）

```
isize close(i32 fd)   // libc: close(fd: i32) -> isize
```

- 只接受 `fd ≥ 3`；0/1/2 是保留位，关闭它们返回 `-EBADF`。
- 成功返回 0。

### 2.5 `mmap`（nr=9）

```
isize mmap(usize len, i32 prot, i32 flags)   // libc: mmap(len: usize, prot: i32, flags: i32) -> isize
```

- **匿名映射**：`flags` 含 `MAP_PRIVATE|MAP_ANONYMOUS`，`fd = -1`，`offset = 0`（非 0 `-EINVAL`）。
- **文件映射（v1.0 第三步只读、第五步可写）**：`fd ≥ 3` 且可读、`flags` 恰为 `MAP_PRIVATE` **或**
  `MAP_SHARED`、`prot` 不含 `PROT_EXEC`、`offset` **页对齐**；把文件 `[offset, offset+len)` 与文件的
  交集拷进新映射（文件尾之后补零）。`MAP_PRIVATE` 的改写只留在本进程（不落盘）；
  `MAP_SHARED` 在 `msync`/`munmap` 时**写回文件**，`prot` 含 `PROT_WRITE` 时要求 fd **打开可写**
  （否则 `-EBADF`）；描述符表 4 槽用尽 → `-ENOMEM`。`MAP_SHARED|MAP_ANONYMOUS` 仍未实现。
- `prot` 允许 `PROT_READ`(0x1) / `PROT_WRITE`(0x2) / `PROT_EXEC`(0x4) / `PROT_NONE`(0x0，v1.0 起：映射存在但 ring3 不可访问，可由 `mprotect` 启用)；拒绝 `PROT_WRITE|PROT_EXEC`（W^X）。
- 返回页对齐地址（> 0）或负 errno；**新页保证全零**；窗口 256 KiB，first-fit。
- 错误：`EINVAL`（位非法/文件映射参数）、`EBADF`（`MAP_SHARED|PROT_WRITE` 但 fd 只读/未打开）、
  `ENOMEM`（超窗口/无空闲帧/共享映射描述符用尽）。

### 2.6 `mprotect`（nr=10）

```
isize mprotect(u64 addr, usize len, i32 prot)   // libc: mprotect(addr: u64, len: usize, prot: i32) -> isize
```

- 改写**已映射**页的保护属性（页帧不搬家、不清零）；成功返回 0。
- 要求：`addr` 页对齐、`len > 0`、范围完整落在**同一个窗口**（映像 / brk 堆 / 匿名 mmap / 共享库）
  且每一页都已映射【不做「部分成功」】。
- `PROT_NONE`：清页表项的 `USER` 位而保留 `PRESENT`——ring3 访问 #PF，页帧与内容都还在，
  可以再 `mprotect` 回来；同时内核不再拿它当用户缓冲（`write` 之类的调用返回 `-EFAULT`）。
- `PROT_WRITE|PROT_EXEC` 仍被拒（W^X）；W^X 下的 JIT 写法是先 `mmap(PROT_READ|PROT_WRITE)` 写，
  再 `mprotect(PROT_READ|PROT_EXEC)` 执行。
- 用户栈（固定 4 页 `RW` + `NX`）不在支持范围 —— 同样是 `-EINVAL`。
- 错误：`EINVAL`（对齐/长度/保护位/跨窗口/有页未映射/目标为栈），见 `docs/syscall.md` 3.10。

### 2.7 `munmap`（nr=11）

```
isize munmap(u64 addr, usize len)   // libc: munmap(addr: u64, len: usize) -> isize
```

- 整段归还（要求页对齐且全段已映射）；清页表项并 `invlpg`，不留幽灵映射。
- **共享文件页在归还之前写回**（v1.0 第五步）：写回失败不阻止归还，只留一行
  `user: munmap writeback errno=<n>`；一次**会被拒**的归还（对齐/窗口/未映射/部分覆盖共享映射）
  不产生任何 I/O。
- 错误：`EINVAL`（含「共享文件映射必须整段归还」，v1 不做 VMA 分裂）。

### 2.8 `brk`（nr=12）

```
u64 brk(u64 addr)   // libc: brk(addr: u64) -> u64
```

- 沿用 **Linux 语义**：`addr = 0` 只查询当前堆顶；否则页对齐增长/收缩（窗口 256 KiB）。
- **不返回错误**：失败时返回**原值**，调用方用 `ret >= addr` 判断成功。

### 2.9 `nanosleep`（nr=35）

```
isize nanosleep(u64 ticks)   // libc: nanosleep(ticks: u64) -> isize
```

- 睡眠 `ticks` 个 PIT tick（100Hz → 1 tick = 10ms）；等待期间 `sti; hlt`，tick 真实推进。

### 2.10 `getpid`（nr=39）

```
u64 getpid()   // libc: getpid() -> u64
```

- 当前进程 PID（内核分配的顺序号）。

### 2.11 `fork`（nr=57）

```
isize fork()   // libc: fork() -> isize
```

- **父进程**返回子进程 PID（> 0）；**子进程**返回 0，并从**同一条用户指令**继续
  （内核恢复 `fork` 时刻的完整寄存器现场，只把返回值改成 0）。
- 失败：`-EAGAIN`（进程表满）、`-ENOMEM`（地址空间复制失败）。
- **调度差异**：单 CPU、用户态无抢占，子进程**不会与父进程并发运行**，要等父进程 `wait4`
  才被恢复 —— 标准写法 `if pid == 0 { execve(..) } else { wait4(pid) }` 原样可用。

### 2.12 `execve`（nr=59）

```
isize execve(const u8 *path, usize path_len, const u64 *argv_pairs, usize argc, u64 _)
// libc: execve(path: &str, argv: &[&str]) -> isize
```

- **成功后不返回**（内核换 CR3 并恢复新入口帧）；失败返回负 errno，调用方通常立刻 `exit`。
- `argv` 用「指针 + 长度」对数组传递：`argv_pairs[2i]` = 字符串指针、`argv_pairs[2i+1]` = 长度；
  内核逐段校验后写进**新进程的用户栈**。上限 `ARG_MAX = 8` 项、单项 `ARG_MAX_LEN = 256` 字节。
- 错误：`ENOENT`（映像不存在）、`E2BIG`（argv 超限）、`EFAULT`、`EINVAL`（ELF 被拒）。

### 2.13 `exit`（nr=60）

```
! exit(i32 code)   // libc: exit(code: i32) -> !
```

- 结束当前进程并把退出码交回内核。顶层程序（`/init`、`/bin/sh`）结束后控制权回到内核；
  子进程结束后回到父进程的 `wait4`。**不返回**。

### 2.14 `wait4`（nr=61）

```
isize wait4(i64 pid)   // libc: wait4(pid: i64) -> isize
```

- 运行并**回收**直接子进程（地址空间整棵归还），返回它的退出码（可为负）。
- 非直接子进程 / 已回收：`-ECHILD`(-10)。

### 2.15 `ckm_policy_query`（nr=257，CKM-1）

```
isize ckm_policy_query(u32 op, u64 arg, u8 *out, usize out_len, 0)   // libc: ckm_policy_query(op, arg, out: &mut [u8]) -> isize
```

- `op` 是「操作」枚举（`docs/ckm.md` 4.6）：`1 fs_write` / `2 state_write` / `3 policy_set` / `4 audit_read` /
  `5 policy_query` / `6 exec_component` / `12..16 storage_*`；`arg` 由 op 解释（如 `fs_write` 的受保护前缀编号）。
- 返回 `0` = 允许，`-EPERM`(-1) = 能力不足；**返回值与记录里的 `result` 一致**（两者不一致即实现错）。
- `out`（≥ 48 字节）收 48 字节记录（下表）；只允许查**当前进程**自己的凭证，没有「查别人」的参数。
- 每次调用都会产生一条审计事件与一行 `ckm: policy …` 串口证据。

| 偏移 | 大小 | 字段 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 8 | `seq` | 审计序号（与 `ckm_audit_read` 同一序列） |
| 8 | 8 | `pid` | 发起进程 |
| 16 | 4 | `uid` / 20:4 `gid` | 凭证 |
| 24 | 2 | `op` | 被查操作 |
| 26 | 1 | `result` | `0` allow / `1` deny |
| 28 | 4 | `need` | deny 时缺少的能力位，否则 0 |
| 32 | 8 | `detail` | op 相关（`fs_write` = 前缀编号，`storage_rescan` = 设备号） |

### 2.16 `ckm_audit_read`（nr=258，CKM-1）

```
isize ckm_audit_read(u8 *buf, usize buf_len, u64 since_seq, u64 *out_count, 0)
```

- 需 `CAP_AUDIT_READ`（uid 1000/1001）：否则 `-EPERM` **且这次拒绝本身也进审计**。
- 拷贝 `seq > since_seq` 的记录（每条 48 字节，**与 2.15 的表相同**），返回**条数**（不是字节数）；
  `out_count`（可选）收审计环的总序号（环容量 64，回绕后旧记录被覆盖）。
- `buf_len` 不足一条记录 → `-EINVAL`；用户缓冲不可写 → `-EFAULT`。

### 2.17 `ckm_component_state`（nr=259，CKM-1）

```
isize ckm_component_state(u8 *buf, usize buf_len, u64 *out_count, 0, 0)
```

- 无能力要求（只读）：导出组件/扩展表快照，每条 64 字节，**顺序确定性**：`kind` 升序 →
  `priority` 降序 → `id` 字节序升序（相同输入两次调用逐字节相同）。

| 偏移 | 大小 | 字段 |
| ---- | ---- | ---- |
| 0 | 16 | `id`（ASCII，NUL 填充） |
| 16 | 2 | `kind`（1 userland / 2 system-component / 3 system-override / 4 kernel-module） |
| 18 | 1 | `state`（0 absent / 1 staged / 2 installed / 3 active / 4 disabled / 5 failed / 6 removed-pending） |
| 20 | 4 | `version_code` |
| 24 | 4 | `abi_kernel` |
| 28 | 4 | `priority` |
| 32 | 24 | `rev`（内容修订 = payload SHA-256 前 24 字节） |

记录的权威定义在内核（`kernel/src/ckm/{audit,state}.rs`），用户态只镜像一次解析器
（`libc::ckm_record_*` / `component_*`），**不自己算偏移**。

### 2.18 `socket` / `bind` / `sendto` / `recvfrom`（nr=41/49/44/45，v1.0）

UDP 套接字最小集（`AF_INET` + `SOCK_DGRAM`）。地址用 16 字节 `sockaddr_in`
（`family` 主机序，`port`/`addr` **网络字节序**）：

```text
0  2  sin_family (u16, = AF_INET / 2)   2  2  sin_port (u16 BE)
4  4  sin_addr   (u32 BE)               8  8  sin_zero (= 0)
```

| 调用 | 参数 | 返回 / 拒绝 |
| ---- | ---- | ---- |
| `socket` | `(domain, type, protocol)` | fd（≥ 3）；`AF_* != AF_INET` → `-EAFNOSUPPORT`；`type & SOCK_TYPE_MASK != SOCK_DGRAM` → `-EPROTONOSUPPORT`；表满 / fd 用尽 → `-ENOSPC` |
| `bind` | `(fd, addr_ptr, addr_len)` | `0`；端口被占 → `-EADDRINUSE`；外机地址 → `-EADDRNOTAVAIL`；非套接字 fd → `-ENOTSOCK` |
| `sendto` | `(fd, buf, len, flags, addr_ptr, addr_len)` | 载荷字节数；`flags != 0` → `-EOPNOTSUPP`；`len > 1472` → `-EMSGSIZE`；无网卡/未 DHCP → `-ENETDOWN` |
| `recvfrom` | `(fd, buf, len, flags, addr_ptr, addr_len_ptr)` | 拷入字节数；未 `bind` → `-EINVAL`；2 秒超时 → `-EAGAIN` |

`addr_len` 至少 16；`recvfrom` 的 `addr_len_ptr`（`socklen_t` = u32）是「缓冲长度进、实际长度出」。
套接字与文件**共用一个 fd 编号空间**，`close` 对两者都有效；`read`/`write` 一个 UDP 套接字返回
`-ENOTCONN`（没有 `connect`）。内核同步轮询收包：目的端口不匹配的数据报被丢弃（无等待队列）。
详见 `docs/syscall.md` 3.14。

### 2.19 `net_config`（nr=267，v1.0，Cerlesse 扩展号）

`net_config(buf_ptr, buf_len)`：把内核 DHCP 协商到的配置写进 16 字节记录：

```text
0  4  本机 IPv4 (BE)    4  4  网关 IPv4 (BE)    8  6  本机 MAC    14  2  保留(=0)
```

返回 16；`buf_len < 16` → `-EINVAL`；没有网卡 / DHCP 尚未成功 → `-ENODEV`。

### 2.20 `msync`（nr=26，v1.0 第五步）

```
isize msync(u64 addr, usize len, i32 flags)   // libc: msync(addr: u64, len: usize, flags: i32) -> isize
```

- 把 `MAP_SHARED` **文件映射**的页写回文件；成功返回 `0`。
- **只有共享文件页会被写回**：`MAP_PRIVATE` 文件页与匿名页跳过（串口上 `pages=0 bytes=0`）——
  对一段纯匿名内存调用它是**合法**的空操作，不是错误。
- 每页写回 `min(PAGE_SIZE, file_len − 页文件偏移)` 字节，`file_len` 是 **`mmap` 那一刻**的文件大小：
  映射比文件长（尾部补零）**不会把文件撑大**。
- `flags`：至少要有一位 `MS_SYNC`(0x4) 或 `MS_ASYNC`(0x1)（本内核同步写回，二者等价）；
  `MS_INVALIDATE`(0x2) 接受并忽略；未知位 `-EINVAL`。
- 要求：`addr` 页对齐、`len > 0`、范围在 mmap 窗口内且**每一页已映射**，缺一即 `-EINVAL`
  （Linux 这里用 `-ENOMEM`；本内核与 `munmap`/`mprotect` 保持同一约定）。
- 写回失败（文件系统只读等）返回 `vfs::write` 的 `-errno`；**不回滚**已写回的页。
- 证据行：`user: msync addr=… len=… flags=0x? -> pages=N bytes=M`（失败则
  `user: msync errno=<n> …`）。

## 3. errno 常量

| 数值 | 名称 | 含义 | 用户态 `libc` 导出 |
| ---- | ---- | ---- | ---- |
| -2 | `ENOENT` | 路径不存在 | ✅ |
| -5 | `EIO` | 块设备读写失败（内核内部） | — |
| -8 | `ENOEXEC` | ELF 格式错误（内核内部，execve 折算为 EINVAL） | — |
| -9 | `EBADF` | fd 无效 / 未打开 / 模式不符 | ✅ |
| -10 | `ECHILD` | `wait4` 找不到直接子进程 | ✅ |
| -11 | `EAGAIN` | 暂无数据（非阻塞语义，**不是错误**） | ✅ |
| -12 | `ENOMEM` | 内存不足 | ✅ |
| -14 | `EFAULT` | 用户地址区间非法 | ✅ |
| -17 | `EEXIST` | 目标已存在（内核内部） | — |
| -20 | `ENOTDIR` | 路径中间分量不是目录 | — |
| -21 | `EISDIR` | 目标是目录，但被当文件用 | ✅ |
| -22 | `EINVAL` | 参数非法 | ✅ |
| -28 | `ENOSPC` | 空间/句柄不足 | ✅ |
| -30 | `EROFS` | 只读文件系统 | — |
| -38 | `ENOSYS` | 号合法但未实现（v1.0 第一步实现 `mprotect` 后无此类调用；保留给后续新增号） | ✅ |
| -117 | `EUCLEAN` | 磁盘结构损坏（簇链断/越界/目录项非法） | — |

## 4. 用户态 `libc` API

`libc/` 是最小用户态运行时（`no_std`）。普通用户程序**只用这一节**；内核细节见第 5 节。
完整用法与新程序模板见 [../libc/README.md](../userland/libc/README.md)。

### 4.1 系统调用封装（`libc::syscall`，全部 re-export 到 crate 根）

| 函数 | 签名 | 备注 |
| ---- | ---- | ---- |
| `raw` | `unsafe fn raw(nr, a1..a5) -> isize` | 裸入口；不要用魔法数调用 |
| `write` | `fn write(fd: i32, buf: &[u8]) -> isize` | |
| `read` | `fn read(fd: i32, buf: &mut [u8]) -> isize` | stdin 行缓冲（见 2.1） |
| `open` | `fn open(path: &str, flags: i32) -> isize` | 返回 fd ≥ 3 或负 errno |
| `close` | `fn close(fd: i32) -> isize` | |
| `brk` | `fn brk(addr: u64) -> u64` | Linux 语义，失败返回原值 |
| `mmap` | `fn mmap(len: usize, prot: i32, flags: i32) -> isize` | 匿名私有（`offset` 固定 0） |
| `mmap_with_offset` | `fn mmap_with_offset(len, prot, flags, offset: u64) -> isize` | 带 `offset`（`r9`）；匿名映射要求 `offset=0` |
| `mmap_file` | `fn mmap_file(fd: i32, len, prot, flags, offset: u64) -> isize` | 文件映射：`MAP_PRIVATE`（可写不落盘）或 `MAP_SHARED`（写回），`offset` 页对齐 |
| `mprotect` | `fn mprotect(addr: u64, len: usize, prot: i32) -> isize` | 改写已映射页的保护属性（v1.0） |
| `munmap` | `fn munmap(addr: u64, len: usize) -> isize` | 归还；共享文件页在归还前自动写回 |
| `msync` | `fn msync(addr: u64, len: usize, flags: i32) -> isize` | 显式写回 `MAP_SHARED` 文件页（v1.0 第五步） |
| `getpid` | `fn getpid() -> u64` | |
| `nanosleep` | `fn nanosleep(ticks: u64) -> isize` | 1 tick = 10ms |
| `fork` | `fn fork() -> isize` | 父 = 子 PID，子 = 0 |
| `execve` | `fn execve(path: &str, argv: &[&str]) -> isize` | 成功不返回；**继承当前环境** |
| `execve_env` | `fn execve_env(path: &str, argv: &[&str], envp: &[&str]) -> isize` | 成功不返回；显式替换环境 |
| `getenv` / `env_count` / `env_var` | `fn getenv(name: &str) -> Option<&'static str>` 等 | 环境变量（`libc::env`，v1.0） |
| `wait4` | `fn wait4(pid: i64) -> isize` | 返回退出码或 `ECHILD` |
| `getdents` | `fn getdents(path: &str, buf: &mut [u8]) -> isize` | 定长 88 字节记录（`dirent_*` 解析器） |
| `getprocs` | `fn getprocs(buf: &mut [u8]) -> isize` | 定长 40 字节记录（`procinfo_*` 解析器） |
| `storage_volumes` | `fn storage_volumes(buf: &mut [u8], out_count: Option<&mut u64>) -> isize` | 定长 64 字节记录（CSS-1，`volume_*` 解析器） |
| `storage_rescan` | `fn storage_rescan(disk_index: u32) -> isize` | `u32::MAX` = 全部（CSS-1） |
| `storage_mount` | `fn storage_mount(vol_id: &str, point: &str, read_only: bool) -> isize` | 需 `CAP_STORAGE_ADMIN`（CSS-3） |
| `storage_unmount` | `fn storage_unmount(point: &str) -> isize` | 需 `CAP_STORAGE_ADMIN`（CSS-3） |
| `storage_assign_letter` | `fn storage_assign_letter(vol_id: &str, letter: u8) -> isize` | `letter = 0` 取消；需 `CAP_STORAGE_ADMIN`（CSS-3） |
| `path::resolve` / `path::resolve_str` / `path::is_windows` | `(&str, &mut [u8]) -> Result<…, isize>` / `(&str) -> bool` | `C:\x` 查 `storage_volumes` 重写为 POSIX（CSS-3） |
| `socket` | `fn socket(domain: u16, ty: u32, protocol: u32) -> isize` | `AF_INET` + `SOCK_DGRAM`；返回 fd（v1.0） |
| `bind` | `fn bind(fd: i32, addr: &SockAddrIn) -> isize` | `SockAddrIn::any(0)` 分配临时端口（v1.0） |
| `sendto` | `fn sendto(fd: i32, buf: &[u8], dst: &SockAddrIn) -> isize` | 返回载荷字节数（v1.0） |
| `recvfrom` | `fn recvfrom(fd: i32, buf: &mut [u8], src: Option<&mut SockAddrIn>) -> isize` | 超时 `-EAGAIN`（v1.0） |
| `net_config` | `fn net_config(cfg: &mut NetConfig) -> isize` | 无网卡 `-ENODEV`（v1.0） |
| `SockAddrIn` / `NetConfig` | `libc::socket::{SockAddrIn, NetConfig}` | `new`/`any`/`port`/`addr`/`ip`/`gateway`/`mac`（v1.0） |
| `ckm_policy_query` | `fn ckm_policy_query(op: u16, arg: u64, out: &mut [u8]) -> isize` | `0`/`-EPERM`；48 字节记录（CKM-1） |
| `ckm_audit_read` | `fn ckm_audit_read(buf: &mut [u8], since_seq: u64, out_count: Option<&mut u64>) -> isize` | 返回**条数**；需 `CAP_AUDIT_READ`（CKM-1） |
| `ckm_component_state` | `fn ckm_component_state(buf: &mut [u8], out_count: Option<&mut u64>) -> isize` | 64 字节记录 × 32 槽，确定性顺序（CKM-1） |
| `ckm_record_seq/_pid/_uid/_gid/_op/_is_deny/_need/_detail` | `(&[u8], usize) -> …` | 48 字节记录（审计/判定）的解析器 |
| `component_id` / `component_kind` / `component_state` / `component_version_code` / `component_abi_kernel` / `component_priority` | `(&[u8], usize) -> …` | 64 字节组件记录的解析器（`rev` 是 24 字节的内容修订，未单独开解析器） |
| `ckm_op_name` / `ckm_op_required_caps` / `component_kind_name` / `component_state_name` | `(u16/u8) -> &'static str` | 串口/日志用短名与「需要哪些能力」镜像 |
| `exit` | `fn exit(code: i32) -> !` | |

### 4.2 常量

- fd：`STDIN = 0`、`STDOUT = 1`、`STDERR = 2`、`FD_FIRST_DYNAMIC = 3`。
- `open` 标志：`O_RDONLY = 0`、`O_WRONLY = 1`、`O_RDWR = 2`、`O_CREAT = 0o100`。
- `mmap`/`mprotect` 保护位：`PROT_NONE = 0x0`、`PROT_READ = 0x1`、`PROT_WRITE = 0x2`、`PROT_EXEC = 0x4`。
- `mmap` 标志：`MAP_SHARED = 0x1`、`MAP_PRIVATE = 0x2`、`MAP_ANONYMOUS = 0x20`
  （`MAP_SHARED|MAP_ANONYMOUS` 仍未实现）。
- `msync` 标志：`MS_ASYNC = 0x1`、`MS_INVALIDATE = 0x2`、`MS_SYNC = 0x4`。
- errno：`ENOENT/EBADF/ECHILD/EAGAIN/ENOMEM/EFAULT/EISDIR/EINVAL/ENOSPC/ENOSYS/EPERM/EACCES`（数值见第 3 节）。
- socket（v1.0）：`AF_INET = 2`、`SOCK_DGRAM = 2`、`IPPROTO_UDP = 17`、`UDP_PAYLOAD_MAX = 1472`、
  `sockaddr_in` 布局 `SOCKADDR_IN_SIZE = 16`、`NET_CONFIG_SIZE = 16`；errno 追加
  `ENODEV(-19)/ENOTSOCK(-88)/EMSGSIZE(-90)/EPROTONOSUPPORT(-93)/EOPNOTSUPP(-95)/EAFNOSUPPORT(-97)/EADDRINUSE(-98)/EADDRNOTAVAIL(-99)/ENETDOWN(-100)/ENOTCONN(-107)/EHOSTUNREACH(-113)`。
- CKM（CKM-1）：能力位 `CAP_CKM_ADMIN = 0x1`、`CAP_CKM_SERVICE = 0x2`、`CAP_FS_SYSTEM_WRITE = 0x4`、
  `CAP_AUDIT_READ = 0x8`、`CAP_STORAGE_ADMIN = 0x10`、`CAP_STORAGE_FORMAT = 0x20`；
  op 枚举 `CKM_OP_FS_WRITE = 1` … `CKM_OP_STORAGE_RESCAN = 16`；
  记录尺寸 `AUDIT_RECORD_SIZE = 48`、`COMPONENT_RECORD_SIZE = 64`；前缀编号 `CKM_PREFIX_ETC/BIN/LIB/INIT`。
- 权限可区分：能力不足 `EPERM(-1)` vs mode 不许可 `EACCES(-13)`（受保护前缀额外要能力，见 `docs/ckm.md` 3.3）。
- 进程：`ARG_MAX = 8`。

### 4.3 宏与运行时支撑

| 项 | 说明 |
| ---- | ---- |
| `print!` / `println!` | 固定 256 字节缓冲；**一次宏调用 = 一次 `write`**（日志里可见 `len=`） |
| `_start` | `global_asm!` 放在 `.text.start`：`_start` → 用户 `main` → `exit(main())` |
| `#[panic_handler]` | 打 stderr 后 `exit(101)` |
| `heap` | `#[global_allocator]`：bump over `mmap`（64 KiB 块）；`dealloc` 是空操作，长驻程序内存会单调上升 |
| `args` | `argc()` / `arg(i)` / `arg_str(i)` / `Args`：入口寄存器 `rdi = argc`、`rsi = argv` |

### 4.4 用户程序构建约束（硬约束，改错会在链接期或 ring3 立刻炸）

| 约束 | 取值 | 原因 |
| ---- | ---- | ---- |
| 目标 | `x86_64-unknown-none` | 裸机用户态 |
| 代码模型 | `-C code-model=large` | 链接在 2^39，超出 small 模型 |
| 重定位 | `-C relocation-model=static` | 内核不做加载时重定位（只接受 `ET_EXEC`） |
| profile | `user-release`（`lto = false`） | large 模型与 LTO 不兼容 |
| 链接基址 | `USER_VIRT_BASE = 0x0000_0080_0000_0000` | 链接地址 = 运行地址，无需重定位 |
| 段对齐 | 每段 `ALIGN(0x1000)` | 装载器按页设权限，同页 W+X 会让 W^X 退化被拒 |
| 静态变量 | 归入 `.bss`（含 `.lbss.*`） | 否则成孤儿段，RW 段不再页对齐 |
| 内存窗口 | 映像 ≤ 64 KiB、栈 16 KiB、brk 堆 256 KiB、匿名 mmap 256 KiB | 固定布局，超窗失败 |

## 5. 内核内部接口（crate 内 trait / 系统级入口）

> 这些接口只在内核编译单元内可见，不是用户 ABI；列在此处便于二次开发与评审。

### 5.1 文件系统：`FileSystem` trait（`kernel/src/fs/vfs.rs`）

后端实现**挂载点内的相对路径**（`/a/b`；挂载点本身是 `/`），挂载点前缀剥离由挂载表完成。
所有路径操作收 `&mut self`（挂载表独占持有后端）。错误一律负数 `i64`。

```rust
pub trait FileSystem {
    fn name(&self) -> &'static str;
    fn stat(&mut self, path: &str) -> Result<Metadata, i64>;
    fn read(&mut self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, i64>;
    fn read_all(&mut self, path: &str) -> Result<Vec<u8>, i64>;
    fn write(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<usize, i64>;
    fn create(&mut self, path: &str) -> Result<(), i64>;
    fn mkdir(&mut self, path: &str) -> Result<(), i64>;
    fn list(&mut self, path: &str) -> Result<Vec<DirEntry>, i64>;
    fn usage(&self) -> (usize, usize) { (0, 0) }   // (文件数, 占用字节数)
    // CSS-2：两个只读查询，缺省是保守实现（只读 / 无卷信息）
    fn caps(&self) -> FsCaps { FsCaps::read_only_unknown() }
    fn volume_info(&self) -> VolumeInfo { VolumeInfo::unknown() }
}
```

现有实现：`ramfs::RamFs`（内存，可读写，单文件 ≤ 256 KiB；`caps` = 可写）、
`fat::FatFs`（FAT12/16/32 **只读**，写操作 `EROFS`；`caps` = 只读 + 大小写不敏感 + 有卷标，
`volume_info` = BPB 序列号/卷标/容量 —— 与 `storage_volumes` 记录逐字段一致，开机自测 `storage: volcheck` 会验）。

`FsCaps`/`VolumeInfo`/`MountOptions` 都定义在 `fs/vfs.rs`；`ENOTSUP`(-95)=「盘在、格式没后端」，
与 `ENOENT`（没有）区分。

**VFS 系统级入口**（经挂载表最长前缀路由；`kernel/src/fs/vfs.rs`）：
`stat` / `read` / `read_all` / `write` / `create` / `mkdir` / `list` / `usage` /
`is_mount_point` / `backend_name`；CSS-2 追加 `route(path)`（挂载点 + 后端 + `vol_id` + 选项）、
`backend_caps(path)`、`backend_volume_info(path)`。
路径工具：`normalize`（拒绝 `.`/`..`）、`components`、`within`。

挂载表（`fs/mount.rs`）：CSS-2 拆成 **`backends`（后端存储）+ `mounts`（挂载点条目）**，
一个后端可被多个挂载点引用（同一卷多入口，不是两份数据）。
`mount(point, fs)`（非卷挂载，如 RAMFS 根）/ `mount_volume(point, fs, vol_id, options)` /
`alias_volume(point, vol_id, options)`（给已挂载的卷再加一个点）/ `umount(point)`（`/` → `EINVAL`，
非挂载点 → `ENOENT`；后端不释放），错误一律负数 `i64`；`resolve(path)` → `(&mut dyn FileSystem, String)`；
`is_mount_point`；`route(path)`；挂载/卸载会把「哪个卷挂了几个点」写回卷记录（偏移 56）。
启动时 `fs::init` 建立初始树（`/init`、`/etc/motd`、`/tmp`、`/bin`），`fs::mount_disks` 经卷表挂
`/disk`（启动盘）与 `/mnt/fat`（测试盘，可能不是 FAT → 跳过并打 errno）；
`fs::storage_mount_selftest` 随后验证多挂载点与卸载（证据行 `storage: mount … vol=… ro=…`、
`fs: route path=… vol=…`、`fs: stat … vol=…`、`storage: unmount … mounts=…`）。

### 5.2 驱动：`Driver` trait（`kernel/src/driver/driver.rs`）

```rust
pub trait Driver {
    fn name(&self) -> &'static str;
    fn matches(&self, dev: &PciDevice) -> bool;   // 无副作用
    fn attach(&self, dev: &PciDevice) -> Result<(), &'static str>;
}
```

注册表 API：`register(driver) -> Result<usize, &str>`、`count()`、`get(i)`、
`register_builtin()`、`report()`、`probe_all() -> ProbeStats`、`driver_for(dev)`。
匹配语义：按注册顺序，首个 `matches` 为真的驱动认领设备；一个设备最多挂接一次；
`attach` 失败不阻断其它设备，计入 `ProbeStats.failed`。

```rust
pub struct ProbeStats { pub devices: usize, pub matched: usize,
                        pub attached: usize, pub failed: usize }
```

### 5.3 块设备：`BlockDevice` trait（`kernel/src/driver/block.rs`）

```rust
pub const SECTOR_SIZE: u32 = 512;
pub trait BlockDevice {
    fn name(&self) -> &'static str;
    fn sector_size(&self) -> u32;                 // v0.8 恒为 512
    fn sectors(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<usize, &'static str>;
    fn write(&mut self, lba: u64, data: &[u8]) -> Result<usize, &'static str>;
    fn capacity_bytes(&self) -> u64 { self.sectors() * self.sector_size() as u64 }
}
```

- `lba` = 绝对逻辑块地址（v0.8 不解析分区表）；长度必须是扇区整数倍，否则报错（不静默取整）。
- 注册表：`register` / `count` / `get(i)` / `first`；`is_sector_aligned(len)`。
- 现有实现：`virtio_blk::VIRTIO_BLK`（legacy virtio-blk 0.9.5，同步轮询 + 页对齐 bounce buffer）。

### 5.4 内存与调度（速览）

| 模块 | 关键接口 |
| ---- | ---- |
| `memory::frame` | 位图帧分配器；`allocate` / `free` / `allocate_contiguous` / `free_contiguous`（DMA/virtqueue 用） |
| `arch::x86_64::paging` | 4 级页表：`translate` / `map_4k` / `unmap_4k` / TLB 刷新；`AddressSpace` |
| `memory::heap` | `#[global_allocator]`（内核堆，映像末尾 1MiB + 首次适配空闲链表） |
| `scheduler` | Round Robin（200ms 时间片）；`suspend` / `resume` / `spawn` |
| `process` | `Process`（PCB：地址空间 / 用户帧 / 每进程 64 KiB 内核栈 / `Ready\|Running\|Zombie`） |
| `shared` | `BootInfo`、`BOOT_MAGIC`（boot 与 kernel 共享的纯数据结构，`no_std`） |

### 5.5 构建期接口（`kernel/build.rs`）

- 用嵌套 `cargo` 构建 `USER_PROGRAMS` 列表中的用户程序（large 代码模型 + `user-release`）；
- 校验 ELF（装载基址 = `USER_VIRT_BASE`、段页对齐、`W^X`、映像窗口、无重定位）后**原样内嵌**；
- v0.9 动态链接：用 `as` + `ld` 现编 `user/dynhello/*.s`（`$OUT_DIR/dyn/` 下的 `dynhello`/`libgreet.so`/`ld.so`），
  与用户程序一样内嵌（因此构建内核需要 binutils 在 `PATH` 上；`make dyn-hostcheck` 缺工具时会明确跳过）；
- `kernel/src/syscall/user.rs` 以 `include_bytes!` 取回并写入 RAMFS 对应路径（程序 8 个 + `/lib` 2 个）。

## 6. 主机侧工具 CLI

主机侧工具**不进入内核**，只在开发机上运行（Python / Go）。更多说明见
[../tools/README.md](../tools/README.md) 与 [../tools-go/README.md](../tools-go/README.md)。

| 工具 | 命令 | 说明 |
| ---- | ---- | ---- |
| 镜像构建 | `tools/image-builder/`（`make image`） | 组装 boot + kernel + 64 MiB FAT 磁盘镜像 |
| ELF 检查 | `python3 userland/tools/check_user_elf.py <file.elf>` | 段布局/基址/页对齐/`W^X`/窗口/重定位（不启 QEMU；阶段五起属用户态 workspace） |
| mkfs | `python3 tools/mkfs/mkfs.py --out <img> --from <dir>` | 生成 FAT12/16/32 镜像；mtools 独立复核（CSS-1 起还生成 GPT/MBR 分区表夹具，`make storage-images`） |
| CKC 打包 | `python3 userland/pkgmgr/ckc.py pack <tree> -o <out.ckc> [--print-rev]` | 生成 `.ckc` 容器（确定性 ustar + 64 字节头） |
| CKC 校验 | `python3 userland/pkgmgr/ckc.py verify <file.ckc>` | 校验头字段与 payload 哈希 |
| CKC 信息 | `python3 userland/pkgmgr/ckc.py info <file.ckc>` | 打印头与条目信息 |
| 测试编排 (Go) | `go run ./cmd/testorch -config ../tests/orchestrate.json -root ..` | 并行跑场景 + 串口复核 |
| 串口分析 (Go) | `go run ./cmd/serialmon -file <log> [-once] [-expect S]... [-allow-panic]` | 跟随/快照分析串口日志 |

CKC 退出码：`0` 成功、`1` 用法/参数错误、`2` 校验失败、`4` I/O 错误。
serialmon 退出码：`0` 通过、`1` 异常/缺失、`2` 用法或 IO 错误。

## 7. 构建与测试目标（Makefile）

```bash
make build / make run            # 构建镜像 / QEMU + OVMF 启动
make test ... test-network       # 各集成测试（断言串口日志）
make test-all                    # test + exception + memory + pagefault + keyboard + shell + proc
                                 #   + programs + dyn + scheduler + ipc + fs + user + ckm + driver + fat + network
make dyn-hostcheck               # 转发到 `make -C userland dyn-hostcheck`：动态链接夹具的宿主 glibc 独立复核（缺工具则跳过）
make userland-images / make user # 在 userland/ 里构建 + 校验全部用户程序（不启 QEMU、不需要内核源码）
make image-kernel / test-kernel-only # 纯内核：不内嵌用户态的镜像 / 断言内核单独启动到诊断空闲
make check-boundary / make check # 内核不得依赖用户态（6 项门禁 + 反向自检）/ 门禁 + go-test
make test-pkg                    # CKC 包容器测试（主机侧，不启 QEMU）
make go-test / make test-orch    # Go 单元测试 / 编排器并行跑全部场景
make fat-image                   # 生成 FAT32 测试盘
make clean
```

| 目标 | 断言内容 | 项数 |
| ---- | ---- | ---- |
| `test` | 串口出现 `Kernel started!` | — |
| `test-exception` | 注入 #DE，异常诊断 | — |
| `test-memory` | 帧/堆/页表自测 + 上下文切换 | — |
| `test-pagefault` | #PF 诊断 + CR2 | — |
| `test-keyboard` | IRQ1 行缓冲整行交付 + PIT 心跳 | — |
| `test-scheduler` | Round Robin 轮转 | — |
| `test-ipc` | pipe 创建/写/读 | — |
| `test-fs` | VFS 路由 + RAMFS 读/写/ls + 负例 | 21 |
| `test-user` | ELF 装载 + 文件 syscall + 用户堆 + `mprotect`（`PROT_NONE`/`RX` 转换）+ 文件映射（只读/可写/共享 + `msync`）+ 隔离 + Shell + 动态链接 + `/init` 的 CKM-1 证据 | 136 |
| `test-ckm` | CKM-1：内核能力判定自测（46 项）+ `/init` 的允许/拒绝 + 组件枚举确定性 + 审计环回绕 | 22 + 2 正则 |
| `test-shell` | 交互式 Shell（注入按键） | 11 |
| `test-proc` | fork/execve/wait4 全链路 + PID 一致性 | 26 + 4 正则 |
| `test-programs` | getdents/getprocs + 基础用户程序（含 `/bin/volumes`）真的被 exec | 32 + 7 正则 |
| `storage-images` | 生成 GPT/MBR/主表损坏/无表四类夹具镜像（不启 QEMU） | — |
| `test-storage-fixtures` | 夹具结构断言（GPT 主/备表头 + CRC、MBR 四主分区 + EBR 链） | — |
| `test-storage` | 卷枚举确定性 + 无分区表退化 + 只读原因 + `degraded`（GPT 场景） | 40 + 3 正则 |
| `test-storage-mbr` | MBR 场景：EBR 链 + 未识别文件系统仍可见 | 15 + 1 正则 |
| `test-network` | virtio-net → Ethernet/ARP/IPv4/UDP（DHCP 往返）+ ICMP Echo + UDP 套接字最小集（用户态 DHCP 往返） | 30 |
| `test-dyn` | PT_INTERP + DT_NEEDED + RELATIVE/JUMP_SLOT（内核侧最小动态链接） | 10 |
| `test-driver` | PCI 枚举 + 驱动挂接 + 扇区读写/负例 | 22 |
| `test-fat` | 双盘 FAT16/32 读/LFN/跨簇/偏移/EROFS | 21 |
| `test-pkg` | `.ckc` 容器规则 + `tar` 独立复核 | 37 |

## 8. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-13 | **v1.0 第五步：可写 / 共享文件映射与 `msync`** —— 号表新增 26 `msync`，9 `mmap` 与 11 `munmap` 行改述为「文件映射可写 / 共享写回」「归还前写回」；新增 2.20 `msync` 逐条语义；2.5 的文件映射段落改为两种模式（`MAP_PRIVATE` 不落盘 / `MAP_SHARED` 写回 + 可写 fd 门槛 + 描述符 4 槽）；4.1 新增 `msync` 封装并改述 `mmap_file`，4.2 新增 `MAP_SHARED` 与 `MS_*`；第 7 节 `test-user` 91 → **136 项**，并同步 `test-proc`/`test-programs`/`test-storage`/`test-storage-mbr` 的陈旧计数（8 → 26 + 4、23 → 32 + 7、26 → 40 + 3、13 → 15 + 1） |
| 2026-10-13 | **v1.0 第四步：UDP 套接字最小集** —— 号表新增 41 `socket`/44 `sendto`/45 `recvfrom`/49 `bind` 与 267 `net_config`，`SYS_MAX` 267 → **268**；新增 2.18/2.19（地址/记录布局与逐条语义）；4.1 新增五个封装与 `SockAddrIn`/`NetConfig`，4.2 新增 socket 常量与 errno；第 7 节 `test-network` 15 → **30 项** |
| 2026-10-12 | **CKM-1（内核级权限控制）**：号表新增 `ckm_policy_query`(257)/`ckm_audit_read`(258)/`ckm_component_state`(259) 与 `ckm_policy_set`(260) 的 ENOSYS 行；新增 2.15/2.16/2.17（op 枚举、48/64 字节记录布局、errno）；4.1/4.2 新增 `libc` 封装、解析器与 `CAP_*`/`CKM_OP_*`/`EPERM`/`EACCES`/记录尺寸常量；号段说明改为「257–259 已实现」；第 7 节新增 `test-ckm`（22 + 2 正则）并标注 `test-user` 包含 CKM 证据 |
| 2026-10-11 | **CSS-2（VFS 收口）**：5.1 节的 `FileSystem` 补 `caps`/`volume_info`（保守缺省）、`FatFs::open` 的错误类型改为 `OpenError { errno, reason }`、VFS 入口提 `route`/`backend_caps`/`backend_volume_info`、挂载表改为 `backends`+`mounts` 并补 `mount_volume`/`alias_volume`/`umount`；5.3 号表行标注 262–264 的内核落点已先备好 |
| 2026-10-11 | 创建本文档：汇总系统调用 ABI（寄存器约定/号表/逐条语义）、errno 表、`libc` API、
内核内部 trait（`FileSystem`/`Driver`/`BlockDevice`）、主机侧工具 CLI 与 Makefile 目标 |
| 2026-10-11 | 补记 v0.9 第六步（最小动态链接）：5.5 构建期接口新增 `as`+`ld` 夹具说明；第 7 节新增 `test-dyn`（10 项）与 `dyn-hostcheck`、补上漏记的 `test-programs`/`test-network` 行，`test-fs` 20 → 21 |
| 2026-10-11 | **v1.0 第一步：`mprotect`** —— 号表第 10 行由 ⛔ 改为 ✅；新增 2.5/2.6 的 `PROT_NONE` 语义与 `mprotect` 逐条说明（2.6 之后的节号顺移一位）；4.1 新增 `mprotect` 封装、4.2 新增 `PROT_NONE`；errno 表 `-38` 行改写；第 7 节 `test-user` 61 → **91** 项 |
| 2026-10-11 | **CSS-1：存储枚举** —— `SYS_MAX` 256 → **267**（号段说明补 256/257–260/261–266 划分）；号表新增 `getdents`(78)/`getprocs`(256)/`storage_volumes`(261)/`storage_rescan`(265) 与 262–264/266 的 ENOSYS 行；第 7 节新增 `storage-images`/`test-storage-fixtures`/`test-storage`/`test-storage-mbr` 并把 `test-programs` 更新为 23 + 6 正则 |
| 2026-10-12 | **CSS-3：卷管理（262/263/264）** —— 号表 262–264 由 ENOSYS 改为 ✅（`storage_mount`/`storage_unmount`/`storage_assign_letter`，均需 `CAP_STORAGE_ADMIN`，判定点 8）；4.1 新增三个 `libc` 封装与 `path::resolve`/`resolve_str`/`is_windows`（`C:\x` 查卷表重写为 POSIX，不硬编码字母）；号段说明改为「只余 266 待 CSS-4」；第 7 节 `test-storage`/`test-programs`/`test-ckm` 的盘符与双入口断言同步 |
