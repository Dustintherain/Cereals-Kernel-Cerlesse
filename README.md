# Cereals-Kernel-Cerlesse

> 代号 **Cerlesse OS** — 使用 Rust 从零开发的类 Linux 高性能宏内核。
> 内核核心：**Rust**；主机侧工具：**Python**（构建/测试）+ **Go**（编排/日志服务）。
> 支持 Linux / Windows 主机交叉开发，目标平台 x86_64（UEFI + QEMU 优先）。

## 当前进度

**v0.3 已完成（2026-10-07）**：物理内存 + 虚拟内存 + 内核堆。
解析 UEFI 内存映射 → 位图帧分配器（4KiB/帧，覆盖 4GiB）→ 4 级页表 Mapper 与 AddressSpace →
内核堆（内核映像后预留 1MiB，首次适配空闲链表）→ `#[global_allocator]`（内核中可直接用 `Box`/`Vec`/`String`）。
顺带修复 v0.2 遗留缺陷：异常名表错位（#PF 被打印成 `reserved (#14)`，B-06）。
自动化验证：`make test` 回归 PASS；`make test-exception`（#DE）PASS；`make test-memory`（堆/帧/页表自测）PASS；
`make test-pagefault`（#PF 诊断 + CR2）PASS。
下一步：v0.4 中断控制器 + Timer + 键盘（见 [DEVELOPMENT.md](DEVELOPMENT.md) 任务清单）。

## 文档入口

- [docs/README.md](docs/README.md) — 文档中心与阶段状态
- [docs/architecture.md](docs/architecture.md) — 内核架构与语言分工
- [docs/roadmap.md](docs/roadmap.md) — 开发路线与 v0.1–v1.0 里程碑
- [docs/project-structure.md](docs/project-structure.md) — 目录结构与模块职责
- [DEVELOPMENT.md](DEVELOPMENT.md) — 主开发文档（需求/任务清单/测试/QEMU/Issue 规划）

## 项目结构（概览）

```text
boot/      UEFI 引导（v0.1 ✅）   kernel/    Rust 内核主体（v0.3 ✅，含内存管理）
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
make test-memory     # v0.3：断言帧分配器 / 内核堆 / 页表 Mapper 自测输出
make test-pagefault  # v0.3：访问未映射地址，断言 #PF 诊断 + CR2
make test-all        # 全部集成测试
```
