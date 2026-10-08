# 设备驱动 / PCI / 网络（阶段文档）

- 阶段：第六阶段（v0.4 基础驱动已落地；v0.8–v0.9 完整驱动与网络待编写）
- 最后更新：2026-10-08
- 状态：🟡 部分完成（v0.4 中断控制器 + PIT Timer + PS/2 键盘 + 上下文切换原语 已验收，见 `kernel/src/interrupt/`、`kernel/src/time/`、`kernel/src/driver/keyboard.rs`、`kernel/src/arch/x86_64/context.rs`；v0.8–v0.9 驱动/PCI/磁盘/网络 尚未编写）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：interrupt（IRQ 分发）、memory（DMA 缓冲）、process（网络可后置）

## 1. 当前状态（2026-10-08）

### 1.1 已落地部分（v0.4 完成：中断控制器 + PIT 100Hz + PS/2 键盘 + 上下文切换原语）

- [x] 中断控制器骨架与 PIC 第一版（初始化、IRQ→向量映射、mask/EOI 原语）
- [x] `kernel/src/interrupt/` 绑定为 `controller` / `exception` / `irq` 三子模块
- [x] `kernel_main` 中接入 PIC 初始化（默认全屏蔽，不使能外设 IRQ）
- [x] 外部流程快照：`kernel/src/interrupt/doc CONTRIBUTING_CHAIN.md`（当前仅作本模块集成上下文，不覆盖 DEVELOPMENT.md 的阶段定义）
- [x] 选型策略记录：`kernel/src/interrupt/pic_policy.md`
- [x] v0.4 中断控制器回归测试已跑通（`make test` / `make test-exception` / `make test-memory` / `make test-pagefault` / `make test-all`）
- [x] PIT/Timer 子系统雏形已接入（`kernel/src/time/`：timer 设置通道 0 重装载值、clock 提供 tick 计数）
- [x] IRQ 分发入口与 16 槽 IRQ 注册表已实现（`kernel/src/interrupt/irq.rs`），`irq_dispatch` 接收完整中断帧并按 `frame.vector` 分发
- [x] IRQ0（PIT）回调已注册并在串口观测心跳（100Hz 下每 100 tick = 1 秒打印一次 `IRQ0_heartbeat tick=<n>`，串口打印期间有重入保护）
- [x] PIT 通道 0 频率校正为 100Hz（10ms/tick）：`time::timer::TICK_RELOAD_100HZ`
- [x] PS/2 键盘中断输入（`kernel/src/driver/keyboard.rs`）：8042 初始化（排空输出缓冲、设置 IRQ1 使能位）、IRQ1 扫描码接收、
      set 1 解码（字母/数字/Space/Enter/Backspace/符号 + Shift + `0xE0` 扩展前缀）、串口回显 `KB <字符>`
- [x] 上下文切换原语（`kernel/src/arch/x86_64/context.rs`）：callee-saved + 栈切换的 `switch_to` / `init_context`，
      并提供内核自测（主上下文 ↔ 备用上下文往返）输出 `context: switch PASS`
- [x] 新增自动化验收：`make test-keyboard`（QEMU monitor `sendkey` 注入真实按键）—— 断言 `KBIRQ ENTRY` / `KB a` / `KBIRQ EXIT` + PIT 心跳

### 1.2 已知状态与限制

- PIC 第一版的映射固定为常见形式：**master/slave + IRQ0..15 → 向量 32..47**，后续可通过映射函数与控制器枚举（`ControllerKind::Pic` / `ControllerKind::Apic`）扩展。
- APIC（LAPIC/IOAPIC/redirection）尚未实现，目前仅保留枚举与 no-op 语义占位。本阶段不尝试同时实现 PIC 与 APIC 两条路线。
- 串口驱动已在 v0.1/v0.2 的串口日志通道中隐含使用；本阶段不单独抽出通用串口驱动框架，直到需要可复用的串口输入/输出接口时再补（预期在 v0.4 键盘/控制台交互或 v0.8 串口驱动框架时）。
- 当前验收仍以 QEMU + 串口断言为主；中断控制器子模块的宿主侧单元测试占位尚未作为可独立运行的测试投入使用，真实中断路径验证仍依赖 QEMU 集成测试。
- IRQ 汇编入口（`irq_common`）保存/恢复 15 个通用寄存器并在 `call` 前强制 16 字节对齐；**不保存** XMM/FPU 状态（当前内核不用浮点），v0.5 若引入浮点再补。
- 键盘初始化末尾会排空输出缓冲，但启用 IRQ1 后仍可能立即收到一个（硬件自发的）扫描码中断，回调会正常处理并打印空的 `KBIRQ ENTRY/EXIT`。
- 键盘驱动当前在中断上下文读扫描码后再打印；未实现键盘环形缓冲，故主循环无法“读取”按键（v0.5/v0.9 控制台时再补）。

### 1.3 为何 drivers.md 现在看起来“只有中断控制器”

- v0.4 目标由三部分组成：Timer tick、PS/2 键盘输入、上下文切换原语（为 v0.5 铺路）。**三者均已落地并自动化验收。**
- 已验证输出样例（串口，节选；`make test-keyboard`）：
  ```
  Cerlesse kernel v0.4
  PIC initialized
  keyboard initialized
  KBIRQ enabled
  PIT 100Hz + PIC IRQ0/IRQ1 unmasked; IF enabled
  ...
  heap: Box/Vec/String PASS
  frame: stress PASS (1024 frames alloc/unique/free)
  paging: map/unmap PASS (fresh address space, 3 pages)
  context: switch PASS
  Kernel started!
  IF=1
  KBIRQ ENTRY
  KB a
  KBIRQ EXIT
  ...
  IRQ0_heartbeat tick=100
  ```
- v0.8 起的驱动框架、PCI 枚举、块设备、网络栈尚未编写；本文件在 v0.8 开发时再补具体实现记录。

## 2. 开发路线（按依赖展开）

### 2.1 v0.4（中断 → Timer → 键盘 → 上下文切换原语）

完成顺序（粗粒度）：

1. PIC 初始化完成后，建立 IRQ→向量映射与 EOI 语义并接入外部中断入口。
2. PIT Timer tick：初始化 PIT（通道 0）、提供系统计时基准、在串口观察到 tick 递增。
3. 建立 IRQ 分发注册表骨架，使后续 IRQ 回调可按向量号注册与分发；当前阶段以 IRQ0（PIT）心跳为最小验证闭环。
4. PS/2 键盘中断输入：键盘控制器初始化、扫描码接收、扫描码解码、按键回显/基本回显逻辑。
5. 上下文切换原语（为 v0.5 的 Task/Thread 切换铺路）：至少让调度入口与切换桩可在中断时钟驱动下观察到。

验收（v0.4）：

- [x] tick 递增可观察（100Hz → 每 100 tick 打印一次 `IRQ0_heartbeat tick=<n>`，经 `make test` / `make test-keyboard` 确认）
- [x] 按键回显可观察（QEMU monitor `sendkey a` → 串口输出 `KB a`，经 `make test-keyboard` 确认）
- [x] 上下文切换原语可用且回归测试不崩溃（`context: switch PASS`，随 `make test-memory` 断言）
- [x] Go 测试编排/串口日志工具（H-04/H-05）已落地（2026-10-08）：
      `tools-go/cmd/testorch`（并行调度 6 个 QEMU 场景 + 串口日志二次复核，`make test-orch`）、
      `tools-go/cmd/serialmon`（tail -f 式实时分类/心跳异常检测/`-expect` 核对）；
      `make go-test` 为宿主侧单元测试入口（详见 tools-go/README.md）

### 2.2 v0.8（Driver Manager → PCI → 块设备 → 磁盘 FS 挂载）

1. 驱动框架：Driver Manager，按 Block / Input / Network 分类注册与匹配。
2. PCI 枚举：遍历 PCI 总线，按 Vendor/Device ID 匹配驱动。
3. 块设备驱动：优先 virtio-blk 或 ATA（QEMU 环境下更易验证）。
4. 磁盘文件系统后端：FAT32 挂载进入 VFS（见 docs/filesystem.md）。

验收（v0.8）：

- [ ] PCI 列表输出，包含 Vendor/Device ID
- [ ] 磁盘读写可验证
- [ ] FAT32 挂载完成，可在 VFS 层访问磁盘文件

### 2.3 v0.9（网络栈顺序 + Socket syscall 最小集）

网络栈分层顺序（自底向上）：

1. Network Driver（例如 virtio-net）
2. Ethernet
3. ARP
4. IPv4
5. ICMP
6. UDP
7. TCP（尽力，非完整工业栈）
8. Socket API：`socket/bind/listen/accept/connect/send/recv`

验收（v0.9）：

- [ ] Shell 交互可用
- [ ] ping/HTTP 简例可跑

## 3. 驱动列表（计划）

### 3.1 优先级高的第一批驱动

| 驱动 | 计划版本 | 说明 |
| ---- | ---- | ---- |
| Serial（串口） | v0.1（串口日志通道） / 框架化待定 | 当前为开发期唯一输出通道；通用驱动框架化晚于基本输入输出需求出现时补 |
| Keyboard（PS/2） | v0.4 | 在中断入口之上实现键盘输入 |
| Timer（PIT/APIC Timer） | v0.4 | 调度与系统计时基础 |
| Framebuffer | v0.4/v0.8 视需要 | 输出显示能力；本阶段不作为强约束，串口优先 |
| PCI | v0.8 | 枚举与匹配 |
| Disk（virtio-blk/ATA） | v0.8 | 块设备，为磁盘 FS 提供后端 |

### 3.2 后续驱动（v1.0 之前按需补）

- USB、NVMe、AHCI、VirtIO（更多设备类型）、Audio、GPU 等排在后续，非第一版目标。

## 4. 网络栈顺序与验收（v0.9）

网络栈顺序（再次强调）：

1. Network Driver → Ethernet → ARP → IPv4 → ICMP → UDP → TCP → Socket

Socket API（最小集，v0.9）：

- `socket`、`bind`、`listen`、`accept`、`connect`、`send`、`recv`

验收（重复，为了与文档索引和 DEVELOPMENT.md 保持一致）：

- [ ] PCI 设备可枚举并打印 Vendor/Device ID
- [ ] 可通过网络栈运行简易 HTTP Server（v0.9 简化网络可达时再验收）

## 5. 非目标 / 后续

- SMP 多核下的中断均衡与高级 APIC 语义：v1.0 之后。
- USB、Wi-Fi、GPU、完整 TCP/IP 工业栈：非第一版目标。
- DMA 缓冲管理细节：在实际驱动（尤其是块设备/网络）用到时再补，把 DMA 缓冲依赖记到 memory.md 对应位置。

## 6. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
| 2026-10-07 | 补记当前状态：PIC 第一版已接入 kernel_main，中断控制器子模块位置与选型策略已就位，v0.4 剩余（Timer/键盘/切换原语）与 v0.8/v0.9 仍为待编写 |
| 2026-10-08 | v0.4 完成：IRQ stub 修正（B-08）、IRQ 入口保存寄存器（B-09）、PIT 100Hz、PS/2 键盘回显、上下文切换原语；新增 `make test-keyboard`；更新本文件当前状态与验收清单 |
| 2026-10-08 | v0.4 最后一项 H-04/H-05 落地：Go 编排器 `testorch` + 串口分析器 `serialmon`；测试脚本支持并行隔离参数与磁盘 `snapshot=on`；新增 `make go-test` / `make test-orch`，验收清单全部勾选 |
| 2026-10-08 | 本次 docs 迭代统一文档顶部元信息（阶段 / 最后更新 / 状态）与修订记录，保持与 DEVELOPMENT.md 与索引文档描述一致 |
