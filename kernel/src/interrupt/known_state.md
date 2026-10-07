# 中断模块当前状态（2026-10-07，v0.4 初期）

## 已达成
- 新建 `kernel/src/interrupt/mod.rs` 并绑定 `controller` / `exception` / `irq`。
- 新建 `kernel/src/interrupt/controller.rs`（ControllerKind::Pic/Apic、Controller、toggle_irq、
  masked_bits、IRQ_BASE_VECTOR、irq_to_vector、单元测试占位块）。
- `kernel/src/main.rs` 已新增 `mod interrupt;`，交叉编译链路干净。
- `cargo check --workspace` 在当前环境下通过。
- 集成回归测试（QEMU + 串口断言）在当前环境下重新跑通：
  - `make test`
  - `make test-exception`
  - `make test-memory`
  - `make test-pagefault`
  - `make test-all`

## 已知限制（当前环境实测）
- `kernel` 是 `no_std` + 自定义 `panic_handler` 的二进制目标。
- 因此 `cargo test -p kernel` 会引入 `std` 并与自定义 `panic_impl` 冲突（E0152），
  不能直接在 kernel crate 上跑 `cargo test`。
- 本次新增的中断控制器单元测试块目前仅作为设计占位存在；真正的中断路径验证
  仍依赖 QEMU 集成测试（串口）。

## 为避免混淆的结论

- 本环境中 `Makefile` 存在，且 `make test-all` 在本次会话中确实通过。
- 此前会话中“`make test-all` 已过”的说法在本环境中是可以复核的，但复核结果
  是基于当前磁盘/Makefile/工具链状态，不能自动继承以前会话的中间产物。
- 本环境当前有 `.cargo/config.toml`（x86_64-unknown-none 强制静态重定位），
  且已安装目标 `x86_64-unknown-none` 与 `x86_64-unknown-uefi`，因此内核与 boot
  的交叉编译链路在本次检查中是可用的。


## 下一步建议
- v0.4 真实代码优先级：PIC 初始化 → IRQ 向量映射 → 使能/屏蔽 → EOI →
  Timer(PIT) → 键盘(PS/2)。
- 中断子模块的宿主侧单元测试若要跑通，需要单独新增可 std 的测试 crate，
  或通过主机侧工具（Python/Go）来覆盖同一断言语义。
