# 中断模块当前状态（2026-10-08，v0.4 完成）

## 已达成

- `kernel/src/interrupt/mod.rs` 绑定 `controller` / `exception` / `irq` 三个子模块。
- `controller.rs`：`ControllerKind::{Pic,Apic}`、`Controller`（初始化 / mask 位图 / enable / disable /
  带向量 EOI / `eoi_with_vector_static` 自由函数）、`IRQ_BASE_VECTOR = 32`、
  `irq_to_vector` / `vector_to_irq` 双向映射；PIC 端口操作内联，不依赖串口驱动。
- `irq.rs`：16 槽 IRQ 注册表（`IRQ_REGISTRY`）、`irq_dispatch(frame)` 分发入口、
  IRQ0（PIT）与 IRQ1（键盘）回调注册、串口重入保护（`serial_debug_enter/exit`）。
- `arch/x86_64/interrupt.rs`：IRQ 向量 32..47 的汇编 stub 全部跳入 `irq_common`；
  `irq_common` 保存/恢复 15 个通用寄存器、`call` 前强制 16 字节栈对齐，然后 `iretq`。
- `arch/x86_64/idt.rs`：IDT 容量 48（异常 0..31 + IRQ 32..47），IRQ 槽位安装对应 stub。
- `kernel_main` 启动链：`init_pic()` → PIT 通道 0 设 100Hz → `enable_irq(0)` →
  `keyboard::init()` → `enable_irq(1)` → 注册回调 → `enable_irqs()`（sti）→ 进入 `hlt` 空闲循环。

## 本环境已验证（QEMU + 串口断言）

- `make test`：启动回归（`Kernel started!`）
- `make test-exception`：`#DE` 诊断
- `make test-memory`：堆 / 帧 / 页表 自测 + `context: switch PASS`
- `make test-pagefault`：`#PF` 诊断 + CR2
- `make test-keyboard`：QEMU monitor 注入真实按键 → `KBIRQ ENTRY` / `KB a` / `KBIRQ EXIT` + `IRQ0_heartbeat tick=`
- `cargo check --workspace`：无错误、无告警

## 已知限制

- `kernel` 是 `no_std` + 自定义 `panic_handler` 的二进制目标，`cargo test -p kernel` 会引入 `std`
  并与自定义 `panic_impl` 冲突（E0152），因此中断路径的真实验证依赖 QEMU 集成测试（串口）。
- IRQ 入口不保存 XMM/FPU 状态（当前内核不使用浮点）。
- PIC 映射固定为常见形式（master/slave，IRQ0..15 → 向量 32..47）；APIC 仍未实现，
  `ControllerKind::Apic` 保留枚举与 no-op 语义。
- 键盘驱动尚未提供环形缓冲，主循环无法读取按键（v0.5/v0.9 控制台阶段再补）。

## 已修复的缺陷

- **B-08**（高）：向量 33..47 曾 `jmp isr_common` → 落入 `exception_dispatch` → IRQ1 一触发即
  `EXCEPTION: irq(1)` + panic。现全部走 `irq_common`。
- **B-09**（高）：IRQ 路径曾直接 `call irq_dispatch` 而不保存通用寄存器，`iretq` 后被中断代码
  带着被破坏的 caller-saved 寄存器运行，偶发 `#GP`。现由 `irq_common` 统一保存/恢复。
