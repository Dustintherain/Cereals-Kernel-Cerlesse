# Cereals-Kernel-Cerlesse

> 代号 **Cerlesse OS** — 使用 Rust 从零开发的类 Linux 高性能宏内核。
> 内核核心：**Rust**；主机侧工具：**Python**（构建/测试）+ **Go**（编排/日志服务）。
> 支持 Linux / Windows 主机交叉开发，目标平台 x86_64（UEFI + QEMU 优先）。

## 当前进度

**v0.5 内核侧核心已完成（2026-10-08）**：内核线程 + Round Robin 时间片调度。
线程控制块与状态机（`process/thread.rs`）→ PID 分配（`process/pid.rs`）→ 定长环形就绪队列（`scheduler/queue.rs`）
→ Round Robin 策略（`scheduler/round_robin.rs`，200ms 时间片）→ PIT 100Hz/IRQ0 驱动的抢占式调度
（`scheduler/scheduler.rs`）：pid 1/2/3 三个内核任务严格轮转并绕回 pid 0（`sched: round-robin wrap PASS`）。

**v0.4 已完成（2026-10-08）**：中断控制器 + Timer + 键盘 + 上下文切换原语。
PIC（8259A）初始化与 IRQ→向量（32..47）映射 → IRQ 分发注册表 → PIT 通道 0 以 **100Hz（10ms/tick）**
产生 IRQ0 并维护 tick 计数（每 100 tick 打印一次 `IRQ0_heartbeat tick=<n>`）→
PS/2 键盘：8042 初始化（排空输出缓冲、打开 IRQ1 使能位）→ IRQ1 扫描码读取 →
set 1 解码（字母/数字/Shift/扩展前缀）→ 串口回显 `KB <字符>` →
内核态上下文切换原语（callee-saved + 栈切换，含内核自测 `context: switch PASS`）。

自动化验证：`make test` 回归 PASS；`make test-exception`（#DE）PASS；
`make test-memory`（堆/帧/页表 + 上下文切换自测）PASS；`make test-pagefault`（#PF 诊断 + CR2）PASS；
`make test-keyboard`（经 QEMU monitor 注入真实按键，断言 `KB a` / `KBIRQ ENTRY` / `KBIRQ EXIT` / 心跳）PASS。

`make test-scheduler`（v0.5 调度器）PASS；`make test-orch`（H-04 Go 编排器并行跑 6 场景 + 串口二次复核）PASS；
`make go-test`（H-04/H-05 宿主侧 Go 单元测试）PASS。

v0.4/v0.5 期间修复的缺陷：异常向量 stub 之外的 IRQ 向量曾误入异常分发而 panic（B-08）、
IRQ 路径未保存通用寄存器导致被打断的代码偶发 #GP（B-09）、
上下文切换未保存 RFLAGS 导致切出中断上下文后 IF 永久为 0（B-10）、
IRQ 的 EOI 时序错误导致 PIC 停摆（B-11）。

下一步：v0.5 的 IPC（pipe），随后 v0.6 系统调用与用户态（见 [DEVELOPMENT.md](DEVELOPMENT.md) 任务清单）。

## 文档入口

- [docs/README.md](docs/README.md) — 文档中心与阶段状态
- [docs/architecture.md](docs/architecture.md) — 内核架构与语言分工
- [docs/roadmap.md](docs/roadmap.md) — 开发路线与 v0.1–v1.0 里程碑
- [docs/project-structure.md](docs/project-structure.md) — 目录结构与模块职责
- [DEVELOPMENT.md](DEVELOPMENT.md) — 主开发文档（需求/任务清单/测试/QEMU/Issue 规划）

## 项目结构（概览）

```text
boot/      UEFI 引导（v0.1 ✅）   kernel/    Rust 内核主体（v0.5 ✅，含内存/中断/时钟/键盘/调度）
shared/    boot/kernel 共享结构    user/      用户态程序（v0.6+）
tools/     Python 主机工具        tools-go/  Go 主机服务
tests/     QEMU 集成测试（v0.1 ✅） docs/     全部开发文档
```

## 常用命令

```bash
make build           # 构建 boot + kernel + 磁盘镜像
make run             # QEMU + OVMF 启动（串口输出到终端）
make test            # 集成测试：断言串口出现 "Kernel started!"
make test-exception  # v0.2：注入 #DE，断言异常诊断输出
make test-memory     # v0.3/v0.4：断言帧分配器 / 内核堆 / 页表 Mapper / 上下文切换自测
make test-pagefault  # v0.3：访问未映射地址，断言 #PF 诊断 + CR2
make test-keyboard   # v0.4：经 QEMU monitor 注入按键，断言 IRQ1 回显 + PIT 心跳
make test-scheduler  # v0.5：断言 ≥3 个内核任务在 PIT 时间片下轮转
make test-all        # 全部集成测试
make go-test         # v0.4 H-04/H-05：Go 单元测试（串口分析 + 并行编排器）
make test-orch       # v0.4 H-04：Go 编排器并行跑 6 个集成测试场景（tests/orchestrate.json）
```
