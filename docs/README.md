# Cerlesse OS 文档中心

- 阶段：第一阶段 / 文档中心
- 最后更新：2026-10-08
- 状态：✅ 已完成（2026-10-08 统一各阶段文档元信息后重新同步索引与修订记录）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。

> 项目代号：**Cerlesse**（仓库：Cereals-Kernel-Cerlesse）
> 定位：使用 Rust 从零开发的类 Linux 高性能宏内核，支持 Linux / Windows 主机交叉开发（x86_64 优先）。
> 语言策略：**Rust 编写内核核心**，Python 用于构建/测试工具链，Go 用于主机侧辅助服务。

## 文档索引

| 文档 | 内容 | 状态 |
| ---- | ---- | ---- |
| [architecture.md](./architecture.md) | 第一阶段：内核架构、语言分工、目标与非目标 | ✅ 已完成（会随开发反复修订） |
| [lang-selection.md](./lang-selection.md) | 按功能领域汇总的性能最佳语言对照（辅助选型参考） | ✅ 新增（2026-10-07） |
| [package-manager.md](./package-manager.md) | 面向内核/系统生态的高效、安全、轻量、灵活包管理器设计提案 | 设计中 |
| [project-structure.md](./project-structure.md) | 目录结构与模块职责划分 | ✅ 已完成 |
| [../DEVELOPMENT.md](../DEVELOPMENT.md) | 主开发文档：需求、依赖、任务清单、测试、QEMU、Issue 规划 | ✅ 已完成 |
| [boot.md](./boot.md) | 阶段：Bootloader / UEFI / Kernel Entry / GDT / IDT | ✅ v0.1 + v0.2 已完成 |
| [memory.md](./memory.md) | 阶段：物理/虚拟内存 / Heap / Allocator | ✅ v0.3 已完成 |
| [drivers.md](./drivers.md) | 阶段：驱动框架 / PCI / 网络（含 v0.4 中断/时钟/键盘/上下文切换） | 🟡 v0.4 已验收（PIC + PIT 100Hz + PS/2 键盘 + 上下文切换原语 + H-04/H-05 Go 工具，`make test-keyboard` / `make test-orch`）；v0.8–v0.9 驱动/PCI/磁盘/网络 尚未编写 |
| [process.md](./process.md) | 阶段：进程/线程 / 上下文切换 / 调度器 | 🟡 v0.5 内核侧核心完成（线程模型 + 上下文切换 + Round Robin 时间片调度，`make test-scheduler`）；IPC 待做 |
| [syscall.md](./syscall.md) | 阶段：系统调用 / 用户空间 / ELF | ⬜ 待编写（v0.6–v0.7）；High-half 映射按 ADR-009 推迟到本阶段 |
| [filesystem.md](./filesystem.md) | 阶段：VFS / RAMFS / 磁盘文件系统 | ⬜ 待编写（v0.7） |

## 阶段说明

- **第一阶段（已完成）**：确定架构方向、总体路线、里程碑、目录结构，并创建代码文件框架占位。
- **第二阶段（v0.1–v0.2）**：Boot ✅、GDT/IDT/异常 ✅（见 boot.md 实现记录）。
- **第三阶段（v0.3）**：物理/虚拟内存 + 内核堆 ✅（见 memory.md）。
- **第四阶段（v0.4）**：中断控制器 + Timer + 键盘 + 上下文切换原语 ✅（见 drivers.md 第 1 节），下一目标 v0.5 进程/线程/调度器（届时展开 process.md）。
- **后续**：每进入一个新阶段就编写/修订对应的主题文档；阶段内已落地部分也要及时补记当前状态（例如 v0.4 中断控制器）。
- 本套文档**会在开发过程中被反复修改**——修改前请在对应文档顶部记录修订日期与变更点。
- 本次（2026-10-08）修订重点：统一各阶段文档顶部元信息（阶段 / 最后更新 / 状态）、修订记录与 DEVELOPMENT.md 描述一致；
  并确认 v0.4（PIC / PIT 100Hz / PS/2 键盘 / 上下文切换原语 + H-04/H-05 Go 工具）与 v0.5（线程模型 + Round Robin 时间片调度）状态在索引中正确标记。

## 文档维护约定

1. 每份文档顶部保留 `最后更新` / `阶段` / `状态` 三行，便于快速辨识（已有文档逐步对齐）。
2. 设计发生变更时，先改文档再改代码（文档即规格）。
3. 未开始的阶段文档保留占位大纲，不提前编写细节；但阶段内已落地部分应及时补记当前状态，避免文档与代码库状态脱节。
4. 本项目文档会在开发过程中被反复修改，修改前请在对应文档顶部记录修订日期与变更点。
