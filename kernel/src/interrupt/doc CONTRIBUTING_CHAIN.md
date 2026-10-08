# 中断模块集成上下文（开发文档）

- 阶段：v0.4 中断控制器（已验收）
- 最后更新：2026-10-08
- 状态：✅ 已落地（做为中断模块集成上下文快照与后续接入说明，不覆盖 DEVELOPMENT.md 的阶段定义）

> 本文件顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。

## 1. 目的

本文件是为了把中断子系统的代码组织、已验证状态、已知限制与后续接入顺序 안정的记录下来，
避免后续在 drivers / process / syscall 阶段反复确认中断入口、PIC 映射、IRQ 分发、
IRQ stub 保存寄存器等已固定住的细节。

## 2. 当前已有模块（v0.4）

- `kernel/src/arch/x86_64/interrupt.rs` — IRQ 汇编入口与 `irq_common`
- `kernel/src/interrupt/mod.rs` — 子模块绑定
- `kernel/src/interrupt/controller.rs` — PIC 第一版（初始化 / mask / enable / disable / EOI / 向量映射）
- `kernel/src/interrupt/controller_test.rs` — 已有/计划中的控制器侧对照测试锚点
- `kernel/src/interrupt/exception.rs` — 异常分发与向量名表
- `kernel/src/interrupt/irq.rs` — IRQ 注册表与 `irq_dispatch`
- `kernel/src/interrupt/known_state.md` — 当前状态记录
- `kernel/src/interrupt/pic_policy.md` — 选型策略记录

## 3. 当前集成状态

- 中断控制器 PIC 第一版已接入 `kernel_main`。
- 当前编译检查通过：`cargo check --workspace`。
- 回归测试路径已在本环境跑通：
  `make test`、`make test-exception`、`make test-memory`、`make test-pagefault`、`make test-all`。
- v0.4 的额外集成验收也已通过：
  `make test-keyboard`、`make test-orch`、`make go-test`。

## 4. 集成顺序（接下来进入 process / syscall 时要点）

### 4.1 启动链（v0.4 固定序列）

1. `init_pic()` — 初始化 PIC，默认全屏蔽，不使能外设 IRQ。
2. PIT 通道 0 设置为 100Hz 重装载值。
3. `enable_irq(0)` — IRQ0（PIT）使能。
4. `keyboard::init()` — 8042 初始化、排空输出缓冲、打开 IRQ1 使能位。
5. `enable_irq(1)` — IRQ1（键盘）使能。
6. IRQ0/1 回调注册。
7. `enable_irqs()`（`sti`）后进入空闲循环。

### 4.2 外设中断入口与返回顺序

- IRQ stub（向量 32..47）统一走 `irq_common`。
- `irq_common` 保存/恢复 15 个通用寄存器，并在 `call` 前强制 16 字节栈对齐。
- IRQ 入口**不保存** XMM/FPU 状态（当前内核不用浮点）。
- `irq_dispatch` 采用「先 EOI，后分发」语义（B-11 修复）。

### 4.3 调度器接入要点（v0.5）

- PIT 100Hz → IRQ0 → `scheduler::on_tick()` → 时间片到期 → `switch` → `context::switch_to`。
- 上下文中必须保存/恢复 RFLAGS（B-10）。
- 调度切换发生在 IRQ0 中断上下文内，切换前已 EOI（B-11）。
- 启动上下文（pid 0）也要回到就绪队列（B-12）。

## 5. 回归测试覆盖现状

| 测试 | 断言 | 状态 |
| ---- | ---- | ---- |
| `make test` | 启动回归（`Kernel started!`） | ✅ |
| `make test-exception` | `#DE` 诊断 | ✅ |
| `make test-memory` | 堆 / 帧 / 页表自测 + `context: switch PASS` | ✅ |
| `make test-pagefault` | `#PF` 诊断 + CR2 | ✅ |
| `make test-keyboard` | QEMU monitor 注入按键 → `KBIRQ ENTRY / KB a / KBIRQ EXIT` + 心跳 | ✅ |
| `make test-orch` | Go 编排器并行跑 6 场景 + 串口二次复核 | ✅ |
| `make go-test` | Go 宿主侧单元测试 | ✅ |

## 6. 限制（当前阶段不做的部分）

- 串口驱动已隐含使用于开发期串口日志通道，但本阶段不单独抽出通用串口驱动框架。
- IRQ 入口不保存 XMM/FPU 状态（当前内核不用浮点）；v0.5 若引入浮点再补。
- APIC（LAPIC/IOAPIC/redirection）尚未实现，仅保留枚举与 no-op 语义占位。
- 键盘驱动尚未提供环形缓冲，主循环无法读取按键（v0.5/v0.9 控制台阶段再补）。
- 中断子模块宿主侧单元测试占位尚未作为可独立运行的测试投入使用，真实中断路径验证仍依赖 QEMU 集成测试。

## 7. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-08 | 创建本文件，作为中断模块集成上下文快照（v0.4）；不覆盖 DEVELOPMENT.md 阶段定义 |
