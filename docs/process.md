# 进程 / 线程 / 调度（阶段文档）

- 阶段：第五阶段（v0.5）
- 最后更新：2026-10-08
- 状态：✅ v0.5 已完成（线程模型 + 上下文切换 + Round Robin 时间片调度 + IPC pipe 骨架，均已自动化验收：`make test-scheduler` / `make test-ipc`）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：memory.md（内核堆/地址空间语义）、interrupt（Timer 驱动调度、上下文切换原语）

## 1. 当前状态（2026-10-08）

### 1.1 已落地

| 能力 | 位置 | 说明 |
| ---- | ---- | ---- |
| 线程控制块 + 状态机 | `kernel/src/process/thread.rs` | `Thread { pid, name, ctx, state, quantum_ticks, stack_top, runs, switches }`；状态 `New/Ready/Running/Blocked/Terminated` |
| PID 分配 | `kernel/src/process/pid.rs` | 单调递增（0 号保留给启动上下文），回收随 v0.6 |
| 线程上下文 | `kernel/src/process/context.rs` | 转发 `arch::x86_64::context` 的 `ThreadContext` / `init_context` / `switch_to` |
| 进程级结构 | `kernel/src/process/process.rs` | 最小 `Process` 登记；Address Space / File Table / Signal 待 v0.6/v0.7 |
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
- 每进程多线程、地址空间切换（CR3）与用户态：v0.6。

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
| 2026-10-08 | 本次 docs 迭代统一文档顶部元信息（阶段 / 最后更新 / 状态）与修订记录，保持与 DEVELOPMENT.md 与索引文档描述一致 |
