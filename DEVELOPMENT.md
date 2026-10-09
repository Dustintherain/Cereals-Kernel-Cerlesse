# Cerlesse OS 主开发文档（DEVELOPMENT）

- 阶段：第一阶段
- 最后更新：2026-10-08
- 状态：v0.1–v0.5 全部完成（v0.4：中断/Timer/键盘/上下文切换 + H-04/H-05 Go 工具，`make go-test` / `make test-orch` 通过；v0.5：线程模型 + Round Robin 时间片调度 + IPC pipe 骨架，`make test-scheduler` / `make test-ipc` 通过）；
  v0.6/v0.7 仍为占位文档，尚未进入编码
- 关联文档：[docs/architecture.md](docs/architecture.md) · [docs/roadmap.md](docs/roadmap.md) · [docs/project-structure.md](docs/project-structure.md) · [docs/drivers.md](docs/drivers.md) · [docs/syscall.md](docs/syscall.md) · [docs/filesystem.md](docs/filesystem.md) · [docs/process.md](docs/process.md)

---

## 1. 功能需求

### 1.1 核心需求（Rust 内核）

| 编号 | 需求 | 版本 |
| ---- | ---- | ---- |
| R-01 | UEFI 引导进入 Rust 内核入口，串口输出 | v0.1 |
| R-02 | GDT/IDT 与异常处理（#PF/#GP/#DF 等） | v0.2 |
| R-03 | 物理帧分配 + 虚拟内存映射 + 内核堆分配器 | v0.3 |
| R-04 | 中断控制器（PIC/APIC）、Timer、键盘输入 | v0.4 |
| R-05 | 进程/线程模型、上下文切换、Round Robin 调度 | v0.5 |
| R-06 | 系统调用入口与首批 10 个 syscall、用户态隔离 | v0.6 |
| R-07 | VFS + RAMFS + ELF Loader | v0.7 |
| R-08 | 驱动框架、PCI 枚举、磁盘驱动 | v0.8 |
| R-09 | Shell、基础用户程序、简化网络栈 | v0.9 |
| R-10 | 权限/凭证、可完整启动的类 Linux 小型 OS | v1.0 |

### 1.2 主机侧需求（Python / Go）

| 编号 | 需求 | 语言 | 版本 |
| ---- | ---- | ---- | ---- |
| H-01 | 镜像构建脚本（组装 boot + kernel + FS 镜像） | Python | v0.1 |
| H-02 | QEMU 自动化：启动、超时、串口日志断言 | Python | v0.1 |
| H-03 | mkfs 工具（RAMFS/FAT 镜像生成） | Python | v0.7 |
| H-04 | 测试编排服务（并行多场景跑 QEMU 测试） | Go | v0.4 |
| H-05 | 串口日志实时收集与分析 | Go | v0.4 |
| H-06 | 包管理器设计与主机侧原型（高效/安全/轻量、Windows 式灵活性参考） | 待定（先主机侧） | 设计阶段 |

### 1.3 非目标（见 architecture.md 第 5 节）

SMP、USB、Wi-Fi、GPU、完整 POSIX、完整动态链接 ELF、完整 TCP/IP、多架构。

---

## 2. 模块依赖关系

```text
boot ──→ arch(x86_64) ──→ interrupt ──→ time
              │               │
              ↓               ↓
           memory ──────→ process ──→ scheduler
              │               │
              ↓               ↓
            driver ─────→ syscall ──→ fs ──→ user(shell/apps)
                              │
                              ↓
                          network（最后）
```

依赖规则：

- 上游模块未验收，下游模块不得开工。
- `network` 依赖 `driver` + `syscall`，排在最后。
- `security` 依赖 `process` + `fs`，v1.0 才实现。

---

## 3. Cargo Workspace 规划（v0.1 初始化）

```toml
# 根 Cargo.toml（v0.1 已落地）
[workspace]
resolver = "2"
members = ["shared", "boot", "kernel", # "user/*"（v0.6 起加入）
];

[profile.dev]
panic = "abort"

[profile.release]
opt-level = 2
lto = true
panic = "abort"
```

约定：

- `shared`：boot 与 kernel 共享的 `BootInfo` 等纯数据结构（no_std）。
- `kernel`：`#![no_std]`，目标 `x86_64-unknown-none`，链接脚本 `kernel/link.ld`（32MiB）。
- `boot`：UEFI 引导 crate，目标 `x86_64-unknown-uefi`；与内核边界调用必须 `extern "sysv64"`（ADR-006）。
- `user/*`：`no_std` 用户态程序，产出静态 ELF（v0.6 起加入 workspace）。
- `tools/`（Python）与 `tools-go/`（Go）**不进 Cargo workspace**，各自独立管理。
- 交叉编译在 Linux 与 Windows 主机上均通过 `rustup target add` 相同目标完成（见 docs/boot.md）。

---

## 4. 目录结构

见 [docs/project-structure.md](docs/project-structure.md)（已同步创建占位文件框架）。

---

## 5. 版本任务清单（v0.1 – v1.0）

### v0.1 — Bootloader + Rust Kernel ✅（2026-10-06 完成）
- [x] 初始化 Cargo Workspace 与 target 配置（shared/boot/kernel；uefi + none 双目标）
- [x] UEFI Bootloader：解析内存映射、页分配、加载 ELF、BootInfo、ExitBootServices
- [x] `kernel_main` 入口 + Serial Console 输出（自建栈，sysv64 边界调用）
- [x] Python 镜像构建脚本（H-01，mtools 组装 64MiB FAT）与 QEMU 启动测试（H-02）
- [x] 验收：QEMU 打印 "Kernel started!"（`make test` 断言通过）
- [x] 文档完成：[docs/boot.md](docs/boot.md)（v0.1 实现记录、踩坑记录 ADR-006/007/008）

### v0.2 — GDT + IDT + Exception ✅（2026-10-06 完成）
- [x] 自建 GDT（null/kcode/kdata）+ TSS（IST1 专用双错栈、RSP0 预留）+ ltr
- [x] IDT 32 个异常向量，汇编 stub（区分有/无错误码）+ 统一保存/恢复（`exception_dispatch`）
- [x] 异常 handler 诊断输出：向量名、错误码、RIP/CS/RFLAGS/RSP/SS、通用寄存器、#PF 时 CR2
- [x] 验收（自动化）：`make test` 回归 PASS；`make test-exception` PASS（串口含 `EXCEPTION: divide error` + 完整寄存器转储 + `KERNEL PANIC: unhandled exception`）
- [x] 文档完成：[docs/boot.md](docs/boot.md)（v0.1/v0.2 实现记录、B-01/B-05/B-06 修复）

### v0.1 缺陷复查与修复（2026-10-06，随 v0.2 一并落地）

| # | 级别 | 问题 | 修复措施 |
| ---- | ---- | ---- | ---- |
| B-01 | 高 | `_start` 到 `kernel_main` 内 `cli` 之间存在中断窗口：ExitBootServices 后 IF 仍为 1，而 IDT 还是固件遗留的，中断可能跳进已失效的固件代码 | `_start` 汇编第一行 `cli`，内核入口即刻关中断（v0.4 重新开启） |
| B-02 | 中 | 内核无自有 GDT/IDT，全程依赖固件遗留状态；任何异常直接 triple fault，不可诊断 | v0.2 本体：自建 GDT/IDT + 异常诊断 |
| B-03 | 低 | 异常处理无自动化回归（v0.1 仅验证成功路径） | 新增 `make test-exception`（feature 门控触发 #DE） |
| B-04 | 低 | 启动日志含一次性低地址内存诊断输出（10 行） | 保留：仅打印 <64MiB 区域，用于装载地址冲突诊断；后续改 QEMU 调试开关 |
| B-05 | **高** | 内核目标默认链接为 **PIE（ELF DYN）**：函数地址表等绝对数据依赖 `.rela.dyn` 的 `R_X86_64_RELATIVE` 加载时重定位，而 bootloader 只按程序头拷贝段、不处理动态重定位 → v0.1 未暴露（全是 RIP 相对访问），v0.2 异常注入时 IDT handler 地址读到错误值，直接把 #DE 分发到垃圾地址 | `.cargo/config.toml` 对 `x86_64-unknown-none` 强制 `-C relocation-model=static`；重建后 ELF 为 EXEC、`There are no relocations`，测试通过 |

复查结论：v0.1 在其自身功能路径（ELF 装载、内存映射、ExitBootServices、ABI 边界）上无回归；
但 v0.2 的异常注入测试暴露出 B-05 这一高危潜伏缺陷（静态重定位未处理），已在 v0.2 根治。
异常防御缺失（B-02）同样由 v0.2 补齐并自动化验证。

### v0.3 — Physical/Virtual Memory + Heap ✅（2026-10-07 完成）
- [x] 解析 BootInfo MemoryMap（UEFI 描述符，仅 `EfiConventionalMemory` 可用）
- [x] FrameAllocator（位图，1 bit/帧，128KiB 静态位图覆盖 4GiB；保留内核映像/低 1MiB/BootInfo/堆区）
- [x] 页表 Mapper（4 级 translate / map_4k / unmap_4k / TLB 刷新 / AddressSpace / 恒等映射补齐）
      —— 内核高半区映射移至 v0.6（ADR-009，与用户态隔离一并完成）
- [x] 内核堆 + `#[global_allocator]`（内核映像末尾预留 1MiB + 首次适配空闲链表，带合并）
- [x] 验收（自动化）：`make test-memory` PASS（`Box`/`Vec`/`String` + 1024 帧压力 + 页表映射自测）；
      `make test-pagefault` PASS（#PF 诊断 + CR2）；`make test` / `make test-exception` 回归 PASS
- [x] 文档完成：[docs/memory.md](docs/memory.md)（v0.3 实现记录、B-06/B-07、ADR-009/010/011 落地）

v0.3 发现并修复的缺陷（随本阶段一并落地）：

| # | 级别 | 问题 | 修复措施 |
| ---- | ---- | ---- | ---- |
| B-06 | 中 | v0.2 异常名表自向量 9 起错位一位（漏记 #9 coprocessor segment overrun）→ #PF 打印成 `reserved (#14)`、#GP 打印成 `page fault`；v0.2 的 #DE 测试刚好在错位之前 | 按 Intel SDM Table 6-1 重建 32 项名表；新增 `make test-pagefault` 覆盖 #PF 名称 + CR2 |
| B-07 | 中 | 堆分配器首版把对齐前导 padding 计入已分配块大小 → 块尾越界覆盖后继空闲块头（空闲链表自环、分配死循环） | 块大小改为 `want - pad`；压力测试增加填充模式校验（抓重叠/越界），已回归 |

### v0.4 — Interrupt + Timer + Keyboard ✅（2026-10-08 完成）
- [x] 中断控制器骨架与 PIC 第一版（初始化、IRQ→向量映射、mask/EOI 原语）
- [x] `mod interrupt` 绑定到 `controller / exception / irq` 子模块
- [x] `kernel_main` 中接入 PIC 初始化（默认全屏蔽，不开启外设 IRQ）
- [x] 这次额外纳入的外部流程文档：`kernel/src/interrupt/doc CONTRIBUTING_CHAIN.md`（当前作为本模块集成上下文快照，不覆盖 DEVELOPMENT.md 本身的阶段定义）
- [x] PIC 选型策略与当前状态记录（`kernel/src/interrupt/pic_policy.md`、`kernel/src/interrupt/known_state.md`）
- [x] PIT Timer tick + 系统计时雏形（`kernel/src/time/timer.rs` 设置通道 0 重装载值、`kernel/src/time/clock.rs` 提供 tick 计数）
- [x] IRQ 分发入口与最小 IRQ0 注册表骨架（`kernel/src/interrupt/irq.rs`），串口心跳防重入保护具备
- [x] IRQ0（PIT）回调已注册，在串口观测心跳（每 10 个 tick 打印一次 `tick=<n>`，经回归测试环境确认可观察）
- [x] PIT 通道 0 频率校正为 **100Hz（10ms/tick）**（`time::timer::TICK_RELOAD_100HZ`，心跳节流每 100 tick = 1 秒）
- [x] IRQ 汇编 stub 修正：向量 32..47 全部走 `irq_common` 分发（原先只有 IRQ0 特例，其余 IRQ 会落入异常分发而 panic）
- [x] IRQ 统一入口 `irq_common` 保存/恢复全部通用寄存器（见 B-09）
- [x] `kernel/src/driver/keyboard.rs`：PS/2 键盘控制器初始化（排空输出缓冲、打开 IRQ1 使能位）
- [x] PS/2 键盘中断输入：IRQ1 扫描码接收 → set 1 解码（字母/数字/Shift/扩展前缀）→ 串口回显 `KB <字符>`
- [x] 上下文切换原语（为 v0.5 铺路）：`arch::x86_64::context` 的 `switch_to` / `init_context` + 内核自测 `context: switch PASS`
- [x] 并行共享磁盘镜像：QEMU 改用 `snapshot=on`（临时写时复制），多场景并发不抢镜像写锁且不污染原镜像
- [x] H-04 测试编排器（Go）：`tools-go/cmd/testorch` + `internal/orchestrator`（场景配置校验、并发信号量、场景超时、串口日志二次复核）
- [x] H-05 串口日志分析（Go）：`tools-go/cmd/serialmon` + `internal/serialparse`（行分类、心跳递增检测、`-expect` 核对、`-allow-panic`）
- [x] 验收（自动化）：`make go-test`（`go vet` + `go test`）PASS；
      `make test-orch`（编排器并行跑 6 个集成测试场景 + 串口二次复核）PASS（6 passed / 0 failed）

v0.4 发现并修复的缺陷（随本阶段一并落地）：

| # | 级别 | 问题 | 修复措施 |
| ---- | ---- | ---- | ---- |
| B-08 | **高** | IRQ 汇编 stub 只把 IRQ0（`isr_32`）改为调用 `irq_dispatch`，向量 33..47 仍 `jmp isr_common` → 走向 `exception_dispatch`；IRQ1 一触发就 `EXCEPTION: irq(1)` + `KERNEL PANIC` | 16 个 IRQ stub 统一 `push 0/pushN/jmp irq_common`；`make test-keyboard` 覆盖 IRQ1 派发路径 |
| B-09 | **高** | IRQ 路径直接 `call irq_dispatch` 而不保存通用寄存器，`iretq` 后被中断的代码带着被破坏的 caller-saved 寄存器继续执行 → 随机 #GP（表现为内存初始化的 `bit_used` 解引用垃圾指针） | `irq_common` 参照 `isr_common` 保存/恢复 15 个通用寄存器，并在 `call` 前强制 16 字节栈对齐 |

### v0.5 — Process + Thread + Scheduler + IPC ✅（2026-10-08 完成）
- [x] Task/Thread 结构与状态机（`process/thread.rs`：`Thread` + `ThreadState`；`process/pid.rs` PID 分配）
- [x] 内核级上下文切换接入调度流程（`process/context.rs` 转发 `arch/x86_64/context.rs`，含 RFLAGS 保存）
- [x] 就绪队列与调度策略抽象（`scheduler/queue.rs` 定长环形队列；`scheduler/round_robin.rs` 200ms 时间片）
- [x] Round Robin 调度器 + PIT 时间片抢占（`scheduler/scheduler.rs`，IRQ0 → `on_tick` → `switch`）
- [x] 基础 IPC（pipe）—— v0.5 最后一块，单向 pipe 骨架已落地
    - `kernel/src/ipc/pipe.rs`：固定容量（64B）环形缓冲单向管道，非阻塞语义
      （写满短写、空读返回 0），创建/写入/读取均打印串口摘要日志
- [x] IPC 模块占位（pipe 优先；channel / shared_memory 后续）：
    - `kernel/src/ipc/pipe.rs`（已实现，含 `selftest`）
    - `kernel/src/ipc/channel.rs`（尚未编写，保留位置；已更新占位说明）
    - `kernel/src/ipc/shared_memory.rs`（尚未编写，保留位置；已更新占位说明）
- [x] 验收（自动化）：`make test-scheduler`（pid 1/2/3 三个内核任务依次运行 + `sched: switch pid=3 -> 0` + `sched: round-robin wrap PASS`）；
      `context: switch PASS` 随 `make test-memory` 断言
- [x] IPC 验收（v0.5 余项，2026-10-08 通过，新增 `make test-ipc`）：
    - [x] 能创建单向 pipe（`pipe: create id=1 cap=64`）
    - [x] 写端可写入、读端可读取已写字节（`pipe: write id=1 len=10` / `pipe: read id=1 len=10` + 读回比对）
    - [x] 串口可观测到 pipe 创建/读写摘要日志
    - [x] 调度器巡回不崩溃（pipe 自测在 Round Robin 巡回后执行，`pipe: selftest PASS`；调度回归仍通过）
- [x] v0.5 收官标记（2026-10-08）：IPC 验收全部通过，v0.5 标记为整体完成；
      channel / shared_memory 仍为后续占位，不在本轮范围

### v0.4/v0.5 当前完成判断（2026-10-08 复核后更新）：
- v0.4 整体标记为**已完成**：内核侧（PIC / PIT 100Hz / PS/2 键盘 / 上下文切换原语）已验收，
  主机侧 H-04/H-05（testorch / serialmon）已落地，`make go-test` 与 `make test-orch`（6 场景并行）均实测通过。
- v0.5 整体标记为**已完成**：线程模型 + 上下文切换 + Round Robin 时间片调度已验收，
  IPC（pipe 骨架）已落代码并通过 `make test-ipc` 验收；channel / shared_memory 按计划保留为后续占位。
- 下一开发顺序按 DEVELOPMENT.md 第 5 节版本顺序继续：进入 v0.6（syscall/user space）。

v0.5 踩坑记录（已修复，建议保留）：

| # | 级别 | 问题 | 修复措施 |
| ---- | ---- | ---- | ---- |
| B-10 | **高** | 上下文切换未保存 `RFLAGS`。调度切换发生在 IRQ0 中断上下文内（中断门已清 IF），切出后 IF 永久为 0 → 后续 IRQ 不再递交，调度与心跳同时停摆 | `context_switch` 加入 `pushfq`/`popfq`；`init_context` 的新上下文默认 `RFLAGS=0x202`（IF=1） |
| B-11 | **高** | IRQ 的 EOI 原先在回调**之后**发出；IRQ0 回调会触发任务切换，本调用栈很晚才恢复 → PIC 不再递交 IRQ0 | `irq_dispatch` 改为「先 EOI，后分发」（中断门已清 IF，提前 EOI 无嵌套风险） |
| B-12 | 中 | 启动上下文切出后没有回到就绪队列，Round Robin 永远绕不回 pid 0，`start()` 无法返回 | `start()` 在切出前把 pid 0 压回队尾 |

### v0.6 — Syscall + User Space（尚未开始编码；本次已把文档细化到可直接开工程度；当前仍为规划/占位）
- [ ] syscall 指令入口（`syscall`/`sysret` 或 `int 0x80`）
    - 当前草案优先候选 `syscall`/`sysret`，备选 `int 0x80` 风格入口
    - 选型与寄存器/错误返回约定在编码时固化
- [ ] `exit/write/read/open/close/fork/exec/wait/getpid/sleep`
    - v0.6 最小集：`exit / write / getpid / sleep` 为核心目标
    - `read / fork / exec / wait` 可在 v0.6 起步或留到 v0.7 完善
    - `open / close` 留到 v0.7 与 VFS 对接
- [ ] 用户地址空间隔离 + 最小 libc（`libc/`）
    - 高半区映射与用户态隔离按 ADR-009 推迟到本阶段一并做
    - `libc/` 至少提供 `start -> main` 胶水 + `exit / write / read` 等封装
- [ ] 验收：用户态 `hello` 运行、退出码回收
- [ ] v0.6 验收（细化后）：
    - [ ] 用户态 `hello` 程序通过 ELF Loader 加载并运行
    - [ ] 用户态隔离下 syscall 入口/返回可观察
    - [ ] `exit` 能回收退出码（至少在串口可观察到）
    - [ ] `fork`/`exec`/`wait` 基本流程可用（v0.7 完善前，v0.6 至少具备可验证骨架）
    - [ ] `getpid`/`sleep` 可用

### v0.7 — VFS + RAMFS + ELF
- [ ] VFS trait（FileSystem/File/Directory、Inode、Mount）
- [ ] RAMFS 实现与目录骨架
- [ ] ELF Loader 完整化（User Stack/Heap）
- [ ] Python mkfs（H-03）
- [ ] 验收：`ls`/`cat`/`cd` 操作 RAMFS

### v0.8 — Driver + PCI + Disk
- [ ] Driver Manager 与驱动注册框架
- [ ] PCI 枚举 + Vendor/Device 匹配
- [ ] 块设备驱动（virtio-blk 或 ATA）
- [ ] 磁盘 FS 后端（FAT32）挂载到 VFS
- [ ] 验收：PCI 列表输出、磁盘读写、FAT32 挂载

### v0.9 — Shell + User Programs + Network
- [ ] `init` + `sh`（ls/cat/cp/mv/rm/mkdir/ps/kill/echo）
- [ ] 简化网络：virtio-net → Ethernet → ARP → IPv4 → UDP（TCP 尽力）
- [ ] Socket syscall 最小集
- [ ] 验收：Shell 交互可用；ping/HTTP 简例可跑

### v1.0 — 完整可启动类 Linux 小型 OS
- [ ] 用户/组/权限（`-rwxr-xr-x`、UID/GID）
- [ ] 进程隔离与 syscall 参数校验
- [ ] 启动链：Kernel → init → Shell → User Programs
- [ ] 文档全量修订、发布 v1.0 tag
- [ ] 验收：QEMU 一键启动到可交互 Shell

---

## 6. 测试方案

| 层级 | 内容 | 工具 | 开始版本 |
| ---- | ---- | ---- | ---- |
| 单元测试 | 纯逻辑模块（分配器算法、ELF 解析、路径解析）在 host 上 `cargo test` | Rust | v0.4（v0.3 暂以内核内自测覆盖，见 docs/memory.md 第 5 节） |
| 集成测试 | QEMU 启动 → 断言串口日志包含预期输出 | Python (H-02) | v0.1 |
| 压力测试 | 帧分配、任务轮转、fork 风暴 | Python + Go 编排 | v0.3/v0.5 |
| 回归测试 | 每个里程碑的验收标准固化为测试用例 | Go 编排服务 | v0.4 |
| 异常测试 | 故意触发 #DE/#PF，校验诊断输出（`make test-exception` / `make test-pagefault`） | Python | v0.2（#PF 于 v0.3 加入） |

约定：

- 每个版本任务清单中的"验收"必须落成自动化测试。
- 串口是唯一权威日志通道，测试断言只依赖串口输出。

## 7. QEMU 调试方案

启动（计划的 Makefile 目标）：

```bash
# 构建镜像后启动（v0.1 起提供）
make run
# 底层等价于：
qemu-system-x86_64 -machine q35 -m 512M -serial stdio -display none \
  -drive format=raw,file=build/disk.img
```

调试流程：

1. **串口日志**：`-serial stdio`，所有 `println!` 重定向到串口，开发期唯一输出通道。
2. **GDB 联调**：QEMU 加 `-s -S`，主机 `gdb target remote :1234`，配合 `cargo run`/`gdb` 脚本单步内核。
3. **崩溃分析**：`-no-reboot -d int,cpu_reset`，配合异常 handler 的寄存器转储定位 triple fault。
4. **断点**：内核符号保留在 ELF 中，`add-symbol-file kernel.elf` 后可按函数名下断点。
5. **Windows 主机**：同一套 QEMU 命令（QEMU for Windows），GDB 用 `gdb-multiarch`/`gdb.exe`，Makefile 目标保持跨平台一致。

## 8. GitHub Issue / 里程碑规划

里程碑与版本一一对应（Milestone 标签 `v0.1` … `v1.0`），Issue 模板分类：

| 标签 | 用途 |
| ---- | ---- |
| `boot` / `memory` / `interrupt` / `process` / `syscall` / `fs` / `driver` / `network` / `security` | 模块归属 |
| `phase:bug` / `phase:feature` / `phase:refactor` / `phase:doc` | 类型 |
| `host:python` / `host:go` | 主机侧工具 |
| `blocked:upstream` | 被依赖项阻塞 |

规划规则：

1. 每个里程碑开工时，把第 5 节对应版本的 checklist 拆成 Issues（一个 checklist 项 ≈ 1 个 Issue，粒度过大的再拆）。
2. Issue 标题格式：`[v0.x] 模块: 简述`，例如 `[v0.3] memory: 实现 bitmap FrameAllocator`。
3. 里程碑关闭条件：该版本任务清单全部勾选 + 验收测试通过 + 对应阶段文档更新。
4. 第一阶段只建 Milestone `v0.1` 及其 Issues，后续版本的 Issues 在开工前一个版本创建。

---

## 9. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段初稿：需求、依赖、workspace 规划、v0.1–v1.0 任务清单、测试/QEMU/Issue 规划 |
| 2026-10-07 | v0.3 完成：内存管理（帧分配器/页表 Mapper/内核堆）；新增 test-memory、test-pagefault 验收目标；补记 B-06/B-07 |
| 2026-10-08 | v0.4 完成：PIC/PIT 100Hz/PS-2 键盘/上下文切换原语；新增 test-keyboard 验收目标；补记 B-08/B-09；H-04/H-05 主机侧 Go 工具仍待开工 |
| 2026-10-08 | v0.5 内核侧落地：线程模型/PID/就绪队列/Round Robin 时间片调度接入 PIT/IRQ0；新增 test-scheduler；补记 B-10/B-11/B-12；IPC 仍待做 |
| 2026-10-08 | v0.4 收尾（H-04/H-05）：复核发现测试编排器（testorch）/串口日志分析（serialmon）代码已落地，实测 `make go-test`（vet + test）与 `make test-orch`（6 场景并行，6 passed / 0 failed）均通过，v0.4 标记为整体完成 |
| 2026-10-08 | v0.4 收官：复核发现 H-04/H-05 代码已落地，实测 `make go-test`（vet + test）与 `make test-orch`（6 场景并行，6 passed / 0 failed）均通过，v0.4 标记为整体完成 |
| 2026-10-08 | v0.5 收官：IPC（pipe）落地（`kernel/src/ipc/pipe.rs` 环形缓冲单向管道 + 串口摘要日志 + 自测），新增 `make test-ipc` 并入 `test-all` 与 orchestrate.json 编排场景；IPC 验收项全部通过，v0.5 标记为整体完成 |
| 2026-10-08 | 本次 docs 迭代统一各阶段文档顶部元信息（阶段 / 最后更新 / 状态）、修订记录与本文件描述一致，保持索引与主开发文档一致 |
| 2026-10-08 | 本次 docs 迭代统一文档与代码库状态不一致点：v0.4 收官前，部分文档一度记载 H-04/H-05 仍待编写；但工具代码已实际落地；随后复核确认 `make go-test`（vet + test）与 `make test-orch`（6 场景并行，6 passed / 0 failed）均通过，v0.4 最终标记为整体完成 |
| 2026-10-08 | 继续 v0.5 后续开发前，统一当前未完成项表述：v0.5 仅剩 IPC(pipe/channel/shared_memory) 未落地；syscall.md 与 filesystem.md 仍为占位文档，尚未进入 v0.6/v0.7 编码 |
| 2026-10-08 | 按开发流程复查后更新：v0.4 与 v0.5 都尚未在 DEVELOPMENT 里标记为整体“以完成”，因为当前尚未完成项尚未通过验收/尚未实现 |
| 2026-10-08 | 本次 docs 迭代统一各阶段文档顶部元信息（阶段 / 最后更新 / 状态）、修订记录与本文件描述一致，保持索引与主开发文档一致 |
