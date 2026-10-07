# Cerlesse OS 主开发文档（DEVELOPMENT）

- 阶段：第一阶段
- 最后更新：2026-10-06
- 状态：第一阶段完成；随开发反复修订
- 关联文档：[docs/architecture.md](docs/architecture.md) · [docs/roadmap.md](docs/roadmap.md) · [docs/project-structure.md](docs/project-structure.md)

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

### v0.2 — GDT + IDT + Exception ✅（2026-10-06 完成）
- [x] 自建 GDT（null/kcode/kdata）+ TSS（IST1 专用双错栈、RSP0 预留）+ ltr
- [x] IDT 32 个异常向量，汇编 stub（区分有/无错误码）+ 统一保存/恢复（`exception_dispatch`）
- [x] 异常 handler 诊断输出：向量名、错误码、RIP/CS/RFLAGS/RSP/SS、通用寄存器、#PF 时 CR2
- [x] 验收（自动化）：`make test` 回归 PASS；`make test-exception` PASS（串口含 `EXCEPTION: divide error` + 完整寄存器转储 + `KERNEL PANIC: unhandled exception`）

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

### v0.3 — Physical/Virtual Memory + Heap
- [ ] 解析 BootInfo MemoryMap
- [ ] FrameAllocator（bitmap 或 freelist）
- [ ] 页表 Mapper、内核高半区映射
- [ ] 内核堆 + `#[global_allocator]`
- [ ] 验收：内核中 `Vec`/`Box` 可用，帧分配压力测试通过

### v0.4 — Interrupt + Timer + Keyboard
- [ ] PIC（后 APIC）初始化与 IRQ 分发
- [ ] PIT Timer tick + 系统计时
- [ ] PS/2 键盘中断输入
- [ ] 上下文切换原语（为 v0.5 铺路）
- [ ] Go 测试编排/串口日志工具（H-04/H-05）
- [ ] 验收：tick 递增、按键回显、10ms 心跳日志

### v0.5 — Process + Thread + Scheduler
- [ ] Task/Thread 结构与状态机
- [ ] 内核级上下文切换
- [ ] Round Robin 调度器 + 时间片
- [ ] 基础 IPC（pipe）
- [ ] 验收：≥3 个内核任务轮转且不崩溃

### v0.6 — Syscall + User Space
- [ ] syscall 指令入口（`syscall`/`sysret` 或 `int 0x80`）
- [ ] `exit/write/read/open/close/fork/exec/wait/getpid/sleep`
- [ ] 用户地址空间隔离 + 最小 libc（`libc/`）
- [ ] 验收：用户态 `hello` 运行、退出码回收

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
| 单元测试 | 纯逻辑模块（分配器算法、ELF 解析、路径解析）在 host 上 `cargo test` | Rust | v0.3 |
| 集成测试 | QEMU 启动 → 断言串口日志包含预期输出 | Python (H-02) | v0.1 |
| 压力测试 | 帧分配、任务轮转、fork 风暴 | Python + Go 编排 | v0.3/v0.5 |
| 回归测试 | 每个里程碑的验收标准固化为测试用例 | Go 编排服务 | v0.4 |
| 异常测试 | 故意触发 #PF/#GP，校验诊断输出 | Python | v0.2 |

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
