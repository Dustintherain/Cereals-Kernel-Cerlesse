# 项目目录结构（第一阶段）

- 阶段：第一阶段 / 结构规划
- 最后更新：2026-10-06
- 状态：✅ 已完成（代码框架已按本结构创建占位文件；占位文件内标注了计划实现的版本号）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。

## 1. 完整目录树

```text
Cerlesse/
│
├── shared/                      # boot/kernel 共享数据结构（v0.1 ✅）
│   └── src/lib.rs               # shared::BootInfo、BOOT_MAGIC
│
├── boot/                        # Bootloader crate（v0.1 ✅）
│   ├── src/                     # main.rs（UEFI 入口+ELF 装载）、serial.rs、uefi.rs
│   └── Cargo.toml
│
├── kernel/                      # 内核主体（Rust, no_std）
│   ├── src/
│   │   ├── arch/x86_64/         # 架构相关：boot/cpu/gdt/idt/interrupt/paging/context
│   │   ├── memory/              # frame/page/heap/allocator
│   │   ├── interrupt/           # exception/irq/controller
│   │   ├── process/             # process/thread/context/pid
│   │   ├── scheduler/           # scheduler/queue/round_robin
│   │   ├── syscall/             # syscall/process/file/memory
│   │   ├── fs/                  # vfs/inode/file/ramfs/mount
│   │   ├── driver/              # driver/serial（v0.1 ✅）/keyboard/framebuffer/pci/block/network
│   │   ├── time/                # timer/clock
│   │   ├── ipc/                 # pipe/channel/shared_memory
│   │   ├── network/             # ethernet/arp/ipv4/udp/tcp/socket
│   │   ├── security/            # permission/credential/capability
│   │   └── main.rs              # 内核入口（v0.1 ✅：_start + kernel_main）
│   ├── link.ld                  # 链接脚本：装载到 32MiB（ADR-007）
│   ├── build.rs                 # 注入链接脚本
│   └── Cargo.toml
│
├── user/                        # 用户态程序（v0.6 起）
│   ├── init/  shell/  ls/  cat/  echo/  ps/
│
├── libc/                        # 最小 libc / 系统调用封装（v0.6 起）
│
├── tools/                       # 主机侧工具
│   ├── image-builder/           # Python：磁盘镜像组装
│   └── mkfs/                    # Python：文件系统镜像生成
│
├── tools-go/                    # Go：测试编排 / 串口日志分析等主机服务
│
├── tests/                       # 集成测试（Python 驱动 QEMU）
│
├── docs/                        # 全部开发文档（见 docs/README.md）
│
├── Cargo.toml                   # Cargo Workspace（shared/boot/kernel）
├── Makefile                     # 构建/QEMU/测试入口（v0.1 ✅）
├── DEVELOPMENT.md               # 主开发文档
└── README.md
```

## 2. 模块职责速查

| 目录 | 职责 | 计划版本 |
| ---- | ---- | ---- |
| `shared/` | BootInfo 等跨 boot/kernel 纯数据结构 | v0.1 ✅ |
| `boot/` | UEFI 引导、内存检测、装载内核、传递 BootInfo | v0.1 ✅ |
| `kernel/src/arch/x86_64/` | GDT、IDT、控制寄存器（`cpu.rs`）、页表操作（`paging.rs`）、上下文切换 | v0.1–v0.3 ✅ |
| `kernel/src/memory/` | 物理帧分配、虚拟内存映射、内核堆、全局分配器 | v0.3 ✅ |
| `kernel/src/interrupt/` | 异常处理、IRQ 分发、PIC/APIC | v0.2 / v0.4 |
| `kernel/src/time/` | PIT/APIC Timer、系统时钟、sleep/yield | v0.4 |
| `kernel/src/process/` | 进程/线程模型、PID、状态机 | v0.5 |
| `kernel/src/scheduler/` | 调度队列、Round Robin | v0.5 |
| `kernel/src/syscall/` | 系统调用分发与各子系统实现 | v0.6 |
| `kernel/src/fs/` | VFS 抽象、RAMFS、挂载 | v0.7 |
| `kernel/src/driver/` | 驱动框架、串口、键盘、帧缓冲、PCI、块设备 | v0.4 / v0.8 |
| `kernel/src/ipc/` | 管道、通道、共享内存 | v0.5+ |
| `kernel/src/network/` | 以太网 → IP → TCP/UDP → Socket | v0.9 |
| `kernel/src/security/` | 权限、凭证、Capability | v1.0 |
| `user/` | init、shell 及基础用户程序 | v0.6–v0.9 |
| `tools/`（Python） | 镜像构建、mkfs、自动化脚本 | v0.1 起 |
| `tools-go/`（Go） | 测试编排、串口日志服务 | v0.4 起 |
| `tests/` | QEMU 集成测试 | v0.1 起 |

## 3. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段初稿，同步创建代码文件框架占位 |
| 2026-10-06 | v0.1 落地：新增 `shared/` crate、`link.ld`、`build.rs`，boot/kernel 标记完成 |
| 2026-10-07 | v0.3 落地：`memory/` 与 `arch/x86_64/{paging,cpu}.rs` 实现完成；link.ld 增加 `__kernel_start/__kernel_end` |
