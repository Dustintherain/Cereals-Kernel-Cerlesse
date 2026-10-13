# 进程 / 线程 / 调度（阶段文档）

- 阶段：第五阶段（v0.5）+ v0.9 第三步（进程对象）
- 最后更新：2026-10-11
- 状态：✅ v0.5 已完成（线程模型 + 上下文切换 + Round Robin 时间片调度 + IPC pipe 骨架，均已自动化验收：`make test-scheduler` / `make test-ipc`）
- ✅ v0.9 第三步：**进程对象与 `fork`/`execve`/`wait4` 已落地**（每进程地址空间 + 每进程内核栈 + 完整用户帧），
  交互式 Shell 现在能真的跑起外部程序（`make test-proc`）；用户态仍无抢占、无信号、fd 表仍是全局的（见 1.4）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：memory.md（内核堆/地址空间语义）、interrupt（Timer 驱动调度、上下文切换原语）

## 1. 当前状态（2026-10-08）

### 1.1 已落地

| 能力 | 位置 | 说明 |
| ---- | ---- | ---- |
| 线程控制块 + 状态机 | `kernel/src/process/thread.rs` | `Thread { pid, name, ctx, state, quantum_ticks, stack_top, runs, switches }`；状态 `New/Ready/Running/Blocked/Terminated` |
| PID 分配 | `kernel/src/process/pid.rs` | 单调递增（0 号保留给启动上下文），回收随 v0.6 |
| 线程上下文 | `kernel/src/process/context.rs` | 转发 `arch::x86_64::context` 的 `ThreadContext` / `init_context` / `switch_to` |
| 进程级结构 | `kernel/src/process/process.rs` | v0.5 只是最小登记；**v0.9 第三步换成真正的 PCB**（地址空间 / 用户帧 / 每进程内核栈 / `return_to` / 退出状态），见 1.4。File Table 与 Signal 仍未做（fd 表仍是全局的） |
| 就绪队列 | `kernel/src/scheduler/queue.rs` | 定长环形队列（无堆分配，可在中断上下文安全使用） |
| 调度策略 | `kernel/src/scheduler/round_robin.rs` | Round Robin，默认 20 tick（PIT 100Hz → **200ms**）时间片 |
| 调度器 | `kernel/src/scheduler/scheduler.rs` | 定长任务表（pid 0 = 启动上下文 + 3 个内核任务）、`spawn` / `start` / `on_tick` / `schedule` |

驱动链路：**PIT 100Hz → IRQ0 → `scheduler::on_tick()` → 时间片到期 → `switch` → `context::switch_to`**。

关键实现要点（踩坑记录）：

1. **上下文必须包含 RFLAGS**。调度切换发生在 IRQ0 中断上下文内，中断门会清 `IF`；
   若切换不保存/恢复 `RFLAGS`，切出后 `IF` 永久为 0 → 后续 IRQ 不再递交，调度与心跳同时停摆。
   现 `context_switch` 在栈上保存 `pushfq` 并在恢复时 `popfq`。
2. **IRQ 的 EOI 必须在切换之前发出**。IRQ0 回调可能切换到其它任务，本调用栈要到很晚才恢复；
   若 EOI 在切换之后，PIC 不再递交 IRQ0。现 `irq_dispatch` 采用「先 EOI，后分发」。
3. **启动上下文也要回到就绪队列**，否则 Round Robin 永远绕不回 pid 0，`start()` 无法返回。
4. 被抢占的任务在自己的内核栈上保留完整中断帧，被重新调度时自然继续执行。

### 1.2 已验证（自动化）

`make test-scheduler`（QEMU + 串口断言）确认 4 个任务严格轮转：

```text
sched: spawn pid=1 name=task_a
sched: spawn pid=2 name=task_b
sched: spawn pid=3 name=task_c
sched: tasks spawned; starting round robin
sched: switch pid=0 -> 1 name=task_a      ┐
task_a: entered                            │
sched: switch pid=1 -> 2 name=task_b       │ 三个内核任务均被调度
sched: switch pid=2 -> 3 name=task_c       │
task_c: entered                            │
sched: switch pid=3 -> 0 name=kernel_main  ┘ 绕回启动上下文
sched: tasks=4 switches=4 started=1
sched: round-robin wrap PASS
```

之后持续轮转（每 200ms 一切换），同时 `IRQ0_heartbeat tick=` 与键盘回显仍正常（`make test-keyboard` 一并通过）。

### 1.3 未完成

- 基础 IPC：pipe 骨架已落地（`kernel/src/ipc/pipe.rs`，`make test-ipc` 通过）；channel / shared_memory 仍为占位。
- `Blocked` / `Terminated` 状态的实际语义（阻塞与退出）依赖 IPC 与进程退出路径。
- ~~**进程对象（Process/PCB）与 `fork`/`execve`/`wait4`**~~：**v0.9 第三步已完成**（见 1.4）。
  这一项曾长期是 v0.9 的硬阻塞点，原因写在 `docs/syscall.md` 3.6：旧入口 `enter_ring3` 假设
  「一次只有一个用户程序」（用户上下文槽与陷入栈全局唯一），所以**不能**用「从 syscall 里
  嵌套再 `enter_ring3`」来冒充 `fork` —— 子程序会覆盖父程序停在 syscall 里的现场。
  真正的出路就是本节的进程对象（每进程上下文 + 每进程内核栈），已在本轮补齐。
- 每进程多线程：仍未做（现在是「一个进程 = 一个线程」）。
  进程级的地址空间切换（CR3）已在 `fork`/`execve`/`wait4` 里落地，见 1.4。

### 1.4 进程对象与 `fork` / `execve` / `wait4`（v0.9 第三步）

| 能力 | 位置 | 说明 |
| ---- | ---- | ---- |
| PCB | `kernel/src/process/process.rs` | `pid` / `name` / **`comm`（定长 16 字节，`ps` 读它）** / `parent_pid` / `slot` / `state` / `exit_code` / `frame`（停在 syscall 里的完整用户寄存器帧）/ `space` / `return_to` |
| 进程状态 | 同上 | `ProcState::{Ready, Running, Zombie}`：`fork` 后子进程是 Ready，`wait4` 恢复它变 Running，`exit` 变 Zombie，`wait4` 取走退出码后释放槽位。`comm` 与状态分开：`create` 用调试名填充，`execve` 成功后把它改写成程序名（`/bin/ls` → `ls`），因此 `ps` 看到的是程序而不是 `"exec"` |
| 每进程内核栈 | 同上 | 64 KiB × `MAX_PROC`；`TSS.RSP0` 与 `CER_SYSCALL_STACK_TOP` 指向同一个栈顶（syscall 与 ring3 中断共用） |
| `fork` | `kernel/src/syscall/process.rs` | 复制地址空间（`UserSpace::clone_for_fork`）+ 克隆当前用户帧（`rax = 0`），父进程返回子进程 PID |
| `execve` | 同上 | 新地址空间 + 新 `argv`/`envp` 用户栈 + 新帧；**先切 CR3 再 Drop 旧地址空间**，成功后不返回 |
| `wait4` | 同上 | 「延迟调度」：子进程在父进程等待时被恢复运行，结束后回收其地址空间并返回退出码 |
| `argc`/`argv`、`envc`/`envp` | `kernel/src/syscall/process.rs::build_user_frame` | 字符串 + 指针数组写到新进程用户栈，入口寄存器 `rdi = argc` / `rsi = argv` / `rdx = envc` / `r10 = envp`（`libc::args`、`libc::env`）；`envp` v1.0 第二步落地，`execve` 默认继承父环境 |
| 进程表快照 | `kernel/src/syscall/process.rs::sys_getprocs` | v0.9 第五步：定长 40 字节记录（`pid`/`ppid`/`state`/`comm`）写进用户缓冲，按槽位顺序；编号 **256**（Cerlesse 扩展号段），`user/ps` 的落点 |

**为什么是每进程内核栈**（这是整个改动的关键）：父进程停在 `sys_wait` 里时，子进程的
`syscall` 与 ring3 中断**不能**落到同一条栈上 —— 全局栈会让子进程的入口汇编把父进程
停在 syscall 里的栈帧踩坏。

**踩坑记录（写下来避免下一个人重踩）**：

1. **内核栈必须够大**。`UserSpace` 约 3 KiB，但 **debug 构建**里它在 `fork` 路径上会被
   反复搬运（`Self::new()` 的返回、`clone_for_fork` 的局部量、`sys_fork` 的临时量），
   实测栈用量约 32 KiB。16 KiB 的栈会**在报错之前**越出栈底，踩掉相邻的 `.bss`
   （帧分配器的 `INITIALIZED` 被清零 ⇒ 之后所有 `frame::allocate()` 都返回 `None`），
   日志上只表现为「莫名其妙的 ENOMEM」。现在栈是 64 KiB，且每个栈底有一个
   `STACK_CANARY` 哨兵，`process::release` 会立刻报 `KERNEL STACK OVERFLOW`。
2. **切换顺序**：子进程退出回父进程时，必须**先恢复父进程的 CR3**，再允许任何
   `Drop` 掉子进程的地址空间 —— 顺序反了就是「释放自己正在用的页表」。
3. **内核栈在恒等映射里**（所有地址空间共享内核映射），所以切 CR3 不会让当前栈失效 ——
   这条性质是「在子进程的栈上恢复父进程现场」能成立的前提。

**已知限制**（不是 bug，写在这里以保持文档诚实）：

- 用户态**没有抢占**：一个进程跑到 `exit`/异常为止；`fork` 出的子进程与父进程
  不并发（「父进程等待时子进程在跑」）；
- `fork` **不复制 fd 表**（fd 表仍是全局的 `fs::file`），`execve` 会 `fs::file::reset()`；
- 没有信号、进程组、`wait4` 的 `WNOHANG`、没有 PID 复用防护（`pid.rs` 单调递增到 4096 后回绕）。

验收：`make test-proc`（从 Shell 交互触发 `hello world` / `ls` / `exit`）断言
fork → exec → argv → 退出码 → 回收的**完整链路**，并且用**反向引用**把 fork / execve / wait4
三处的 PID 绑定为同一个值（PID 的具体数值依赖启动顺序，写常数会碎）；同时断言
`user: frames free before=N after=N` —— 父子进程的地址空间**全部归还**，没有页帧泄漏。

## 2. 概述

v0.5 进程/线程/调度阶段依赖以下前置能力：

- 内核堆与地址空间语义（v0.3 已完成，高半区/用户态隔离计划见 syscall.md）。
- Timer 与上下文切换原语（v0.4 末启动，见 drivers.md 中断/Timer/键盘部分）。
- 中断上下文中可能需要分配时的线程安全措施（v0.4 引入中断后再补自旋锁）。

建议顺序（粗粒度）：

1. Task/Thread 结构与状态机定义。
2. 内核级上下文切换实现（save/restore registers， scheduler 介入）。
3. Round Robin 调度器 + 时间片（Timer 中断驱动）。
4. 基础 IPC（pipe）与进程退出/回收语义。
5. 验收：≥3 个内核任务轮转且不崩溃。

## 2. v0.5（Process + Thread + Scheduler）

### 2.1 Process 结构（计划）

Process 结构至少包括（具体字段在实现时定）：

- Address Space：进程地址空间语义（用户态隔离后，在 v0.6 中进一步落实）。
- File Table：打开文件描述符集合（v0.7 文件系统对接后再完整化）。
- Signal：信号语义（计划字段，第一版可保守推迟）。
- Threads：进程下的线程集合/线程组语义。

### 2.2 Thread 结构（计划）

Thread 结构至少包括（具体字段在实现时定）：

- Registers：上下文寄存器快照（切换时保存/恢复）。
- Stack：线程栈语义。
- State：线程状态（NEW / READY / RUNNING / WAITING / TERMINATED 等）。
- Scheduling Info：调度所需的队列/优先级/时间片等信息。

### 2.3 状态机（计划）

典型状态机（粗粒度）：

1. NEW
2. READY
3. RUNNING
4. WAITING
5. 回到 READY
6. TERMINATED

状态迁移的具体触发条件与回收语义在实现时记录。

### 2.4 上下文切换

- 上下文切换原语在 v0.4 末启动，v0.5 中将其接入调度流程。
- 基本流程：save registers → Scheduler 选择下一个线程 → restore registers。
- 第一版不引入复杂的抢占模型，先保证切换不崩溃且可轮转。

### 2.5 第一版调度器

- 第一版调度器计划采用 Round Robin（10ms 时间片）（ADR-005）。
- 后续可扩展 Priority / CFS-like / Real-time 调度策略，但不在 v0.5 实现。
- Timer 中断驱动的时间片轮转是本阶段验收的关键可观察点。

### 2.6 IPC（v0.5 余项，按计划先做 pipe 骨架）

IPC 是本阶段最后一块。按规划顺序，现在应先完成 pipe 骨架，再补验收。

#### 2.6.1 规划原则

- IPC 不追求完整语义，v0.5 目标是**可验证骨架**。
- 第一版优先实现**单向 pipe**，后续再补 channel / shared_memory。
- v0.5 IPC 目前不依赖用户态隔离，不引入文件描述符表完整语义。
- 管道语义重点是：产生端 / 读取端、有限缓冲、阻塞/非阻塞雏形、在串口可观测。

#### 2.6.2 模块位置

- `kernel/src/ipc/pipe.rs` — 单向 pipe 骨架
- `kernel/src/ipc/channel.rs` — 消息通道（后续）
- `kernel/src/ipc/shared_memory.rs` — 共享内存（后续）

#### 2.6.3 pipe 骨架的最小计划字段

- 管道对象语义：
  - `Pipe { write_end, read_end, buffer, ... }`
  - 单向：一端写，一端读
- 最小能力：
  - 创建 pipe
  - 写端写入有限字节
  - 读端读取已写入字节
  - 缓冲为空时读阻塞雏形（或立即返回 0/EAGAIN 风格语义，视 v0.5 复杂度决定）
- 调试可观测点：
  - 创建时打印管道标识
  - 写入/读取时打印少量摘要，便于串口断言

#### 2.6.4 v0.5 IPC 不做的事

- 不在 v0.5 引入完整进程文件描述符表语义（v0.7 对接）
- 不在 v0.5 做双向 pipe、select/poll、信号量、完整阻塞调度闭环
- channel / shared_memory 仅保留占位，暂不编写

### 2.7 验收（v0.5）

- [x] 两个内核任务可来回切换且不崩溃（`context::selftest`，随 `make test-memory` 断言）
- [x] ≥3 个内核任务轮转且不崩溃（`make test-scheduler`：pid 1/2/3 依次运行）
- [x] Timer 中断驱动的时间片轮转可观察到（`sched: switch` 日志每 200ms 一次）
- [x] 基础 IPC（pipe）至少具备可验证骨架（`make test-ipc`，2026-10-08 通过）
  - [x] 能创建单向 pipe（`pipe: create id=1 cap=64`）
  - [x] 写端可写入、读端可读取已写字节（`pipe: write id=1 len=10` / `pipe: read id=1 len=10` + 读回比对）
  - [x] 串口可观测到 pipe 创建/读写摘要日志
  - [x] 调度器巡回不崩溃（pipe 自测在 Round Robin 巡回之后执行，`pipe: selftest PASS` 且调度回归仍通过）
- [x] v0.5 IPC 后续推进标记（2026-10-08 完成）：
    - pipe 骨架已落代码并通过验收
    - channel / shared_memory 仍为后续占位，不在本轮推进范围
    - v0.5 据此标记为整体完成

## 3. 非目标 / 后续

- 完整用户态隔离/进程隔离语义：计划在 syscall.md 对应阶段（v0.6）完成。
- 文件描述符表完整化：依赖 v0.7 文件系统对接。
- 信号、权限、凭证、细粒度安全模型：计划在 v1.0 阶段完成（security 模块）。
- SMP、多核调度、高级调度策略：非第一版目标。

## 4. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
| 2026-10-07 | 补记路线细化：明确 Task/Thread/状态机/上下文切换/调度器/IPC 的计划字段，补充依赖说明（内存/Timer/上下文切换原语/中断线程安全），补充验收占位 |
| 2026-10-08 | v0.5 内核侧落地：线程模型/PID/就绪队列/Round Robin 时间片调度接入 PIT/IRQ0，新增 `make test-scheduler`；记录 RFLAGS 与 EOI 时序两个关键踩坑；IPC 仍待做（未选型、未落代码、未定验收） |
| 2026-10-08 | 继续 v0.5 后续开发前补记当前状态：v0.5 未完成项仅剩 IPC(pipe/channel/shared_memory)，本轮未开始编写代码，仅更新文档与开发文档流程约定一致 |
| 2026-10-08 | 按开发文档流程继续 v0.5 后续计划细化：将 IPC 拆为三模块占位（pipe 优先），明确 v0.5 仅做 pipe 骨架；channel/shared_memory 仍为后续占位 |
| 2026-10-08 | v0.5 收官：`kernel/src/ipc/pipe.rs` 落地（环形缓冲单向管道 + 串口摘要日志 + 自测），新增 `make test-ipc` 并入 `test-all` 与编排场景；验收项全部勾选，v0.5 标记为整体完成 |
| 2026-10-11 | **进程模型升级为 v0.9 的硬阻塞点**：1.3 明确 `fork`/`execve`/`wait4` 与进程对象仍未实现，并记录「不能靠 syscall 内嵌套 `enter_ring3` 绕过」的原因（用户上下文槽/陷入栈全局唯一）与影响面（Shell 只能跑内建命令） |
| 2026-10-11 | **v0.9 第三步：进程对象 + `fork`/`execve`/`wait4`**（本节新增 1.4）—— `process/process.rs` 从「最小登记」换成真正的 PCB（地址空间 / 用户帧 / 每进程 64 KiB 内核栈 + 栈底哨兵 / `return_to` / `Ready|Running|Zombie`）、新增 `kernel/src/syscall/process.rs`（fork 复制地址空间与当前帧、execve 换映像并重建 argv 用户栈、wait4 延迟调度 + 回收）、`trampoline` 改为按进程切换（CR3 / TSS.RSP0 / syscall 栈顶 / 当前进程下标）；记录两条踩坑（debug 构建下 16 KiB 内核栈静默溢出踩坏 `.bss` 帧分配器；退出时必须先恢复 CR3 再 Drop 地址空间）；新增 `make test-proc`（8 项断言 + 4 条正则，含 PID 一致性反向引用与帧全回收）；1.3 中该阻塞点标记为已完成 |
| 2026-10-11 | **v0.9 第五步：进程表可被用户态枚举** —— PCB 新增定长 `comm: [u8; 16]`（`create` 用调试名填充，`execve` 成功后改写为程序名，否则 `ps` 看到的永远是 `"exec"`）；新增 `syscall/process.rs::sys_getprocs`（编号 **256**，定长 40 字节记录 `pid`/`ppid`/`state`/`comm`，按槽位顺序）与 `user/ps` → `/bin/ps`（1.4 表格同步） |
| 2026-10-12 | **v1.0 第二步：`execve` 传递环境变量 `envp`** —— `syscall/process.rs::sys_exec` 新增 `args[4]`/`args[5]`（`envp_ptr`/`envc`，与 `argv` 同一套指针+长度数组与区间校验），`build_user_frame` 在 `argv` 块下方再写 `envp` 字符串 + 指针数组（各有独立栈预算），`UserFrame::new_user` 用 `rdx`/`r10` 传 `envc`/`envp`；顶层程序（`user.rs::DEFAULT_ENV`）注入默认环境，`libc::execve` 默认继承、`execve_env` 可替换，Shell 新增 `env` 内建；1.4 表格同步 |
| 2026-10-08 | 本次 docs 迭代统一文档顶部元信息（阶段 / 最后更新 / 状态）与修订记录，保持与 DEVELOPMENT.md 与索引文档描述一致 |
