# 软件开发文档（构建 / 测试 / 调试 / 协作规范）

- 阶段：第八阶段（v0.9 进行中）
- 最后更新：2026-10-11
- 状态：🟡 持续更新 —— 面向**在 Cerlesse 上继续开发的人**：环境、构建、测试、调试、
  代码与文档规范、协作流程、质量门禁，以及「新增一类东西」的步骤。

> 本文档与其它文档的分工：
> [../README.md](../README.md) 是项目入口，[../DEVELOPMENT.md](../DEVELOPMENT.md) 是需求与版本任务清单，
> [api.md](./api.md) 是对外接口参考，各主题背景在 boot/memory/drivers/process/syscall/filesystem 等文档。

> 本文档顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。

## 1. 目的与适用范围

本项目是用 Rust 从零实现的类 Linux 宏内核（代号 Cerlesse），
开发语言分工：**内核与用户态运行时用 Rust**，**构建/测试/镜像工具用 Python**，
**高并发主机侧服务用 Go**（见 [architecture.md](./architecture.md) 第 2 节）。
本文回答「拿到仓库后怎么开发」这个问题，适用于 Linux 与 Windows 开发主机（x86_64 优先）。

## 2. 开发环境与工具链

| 组件 | 用途 | 说明 |
| ---- | ---- | ---- |
| Rust（stable） | 内核 / boot / libc / user | 需 `rustup`；目标 `x86_64-unknown-none` 与 `x86_64-unknown-uefi` |
| GNU Make | 统一入口 | 所有构建/测试命令都包在 `Makefile` 目标里 |
| QEMU（`qemu-system-x86_64`） | 运行/测试 | 机器 `q35`，512M；启动盘为 virtio-blk |
| OVMF | UEFI 固件 | 默认 `/usr/share/OVMF/OVMF_CODE.fd`、`OVMF_VARS.fd`（可用变量覆盖） |
| Python 3 | 镜像构建、测试驱动、`mkfs`、`ckc` | **仅标准库**，不需要 pip 安装 |
| mtools（`mdir`/`mtype`） | 独立复核 FAT 镜像 | 只在 `test-fat` / `fat-image` 复核时需要 |
| Go | `testorch` / `serialmon` | **仅标准库**，模块 `cerlesse/tools` |

一次性准备（以 Linux 为例）：

```bash
rustup target add x86_64-unknown-none x86_64-unknown-uefi
# Debian/Ubuntu: sudo apt-get install qemu-system-x86 ovmf mtools  # 及 make / python3 / golang
```

Makefile 顶部是可覆盖变量，Windows/自定义路径下可这样传：

```bash
make run OVMF_CODE=/path/OVMF_CODE.fd OVMF_VARS=/path/OVMF_VARS.fd QEMU=/path/qemu-system-x86_64
```

## 3. 构建

```bash
make build     # = make image：构建 boot + kernel + 磁盘镜像（build/disk.img）
make run       # 启动 QEMU（-serial stdio，串口输出到终端）
make clean     # 清理构建产物
```

- 内核目标 `x86_64-unknown-none` 由 `.cargo/config.toml` 强制
  `relocation-model=static`（静态非 PIE，避免 bootloader 处理动态重定位，ADR-008）。
- `make kernel` / `make boot` 只构建单个 crate；`cargo build`（不带 `--target`）只构建
  `default-members`（`shared`/`boot`/`kernel`），不会连带构建用户程序。
- **用户程序由 `kernel/build.rs` 在内核构建时自动重建**（`user/*`，large 代码模型 +
  `user-release` profile），因此测试目标之间没有额外先后顺序；`make user` 只用于**单独验证脚手架**。

## 4. 测试

### 4.1 原则

- **串口是唯一权威日志通道**：所有测试断言只依赖串口输出（`println!` 全部重定向到串口）。
- **每个版本任务清单里的「验收」必须落成自动化测试**；设计先写进文档，再落代码（文档即规格）。
- 失败要能定位到**原因**而不是措辞：断言优先引用可观测的结构化标记（如 `driver: match 00:03.0 -> virtio-blk-pci`）。

### 4.2 常用目标

```bash
make test            # 冒烟：断言串口出现 "Kernel started!"
make test-all        # 全部集成测试（见下）
make user            # 单独构建 + 校验用户程序 ELF（不启 QEMU）
make test-pkg        # .ckc 包容器测试（主机侧，不启 QEMU）
make go-test         # Go 单元测试（go vet + go test）
make test-orch       # Go 编排器并行跑 orchestrate.json 全部场景 + 串口二次复核
```

`make test-all` 组成：`test` `test-exception` `test-memory` `test-pagefault` `test-keyboard`
`test-shell` `test-proc` `test-scheduler` `test-ipc` `test-fs` `test-user` `test-driver` `test-fat`。
逐目标断言内容与项数见 [api.md](./api.md) 第 7 节。

场景定义在 `tests/orchestrate.json`（每场景独占 OVMF_VARS / 串口日志 / monitor socket，
磁盘镜像以 QEMU `snapshot=on` 共享，互不抢锁、不污染原镜像）。

### 4.3 结构

| 层级 | 内容 | 位置 |
| ---- | ---- | ---- |
| 内核内自测 | 内存/页表/上下文/调度/IPC 等在启动时打印 `... PASS` | `kernel/src/**` |
| 集成测试 | Python 驱动 QEMU，断言串口日志 | `tests/test_*.py` |
| 主机侧单元测试 | CKC 容器、Go 工具 | `userland/pkgmgr/tests/test_ckc.py`（阶段五迁至用户态 workspace）、`tools-go` |
| 编排/回归 | Go 编排器并行 + 日志二次复核 | `tools-go/cmd/testorch` |

## 5. 调试

```bash
make run                         # 串口到终端，观察正常启动
qemu ... -s -S                   # 附加 GDB 服务端并暂停，主机 gdb -> target remote :1234
qemu ... -no-reboot -d int,cpu_reset   # 崩溃/triple fault 时转储中断与 CPU 状态
```

要点：

1. **串口日志**是开发期第一手证据；异常 handler 会打印向量名、错误码、`RIP/CS/RFLAGS`、
   通用寄存器，#PF 时附带 `CR2`。
2. 内核 ELF 保留符号：`add-symbol-file kernel.elf` 后可按函数名下断点。
3. 条件编译触发异常：`--features exception-test`（#DE）、`--features pagefault-test`（#PF）。
4. Windows 主机使用同一套 QEMU 命令与 `gdb-multiarch`；Makefile 目标保持跨平台一致。

## 6. 代码规范

1. **`no_std`**：内核、`shared`、`libc` 均为 `no_std`；`shared` 只放 boot/kernel 共享的纯数据结构。
2. **错误一律负数 errno**（内核与 VFS 一致，中间层不翻译）；用户 ABI 见 [api.md](./api.md) 第 3 节。
3. **固定容量注册表**：驱动/块设备等注册表用定长数组 + 启动期写入、之后只读，让借用关系一眼看清。
4. **安全默认**：用户指针按页校验（越界 `EFAULT`）；`W^X` 强制（拒绝 `PROT_WRITE|PROT_EXEC`）；
   新映射页保证全零；清页表项必须 `invlpg`。
5. **注释解释「为什么」**：关键取舍、踩坑原因、与 Linux 的差异都写在模块头与函数上（中文）。
6. **ADR**：重大设计决策追加到 [architecture.md](./architecture.md) 第 6 节并同步 DEVELOPMENT。
7. **链接约束**：内核固定装载 32MiB、静态非 PIE；用户程序 large 代码模型 + 段页对齐 +
   `.lbss.*` 归入 `.bss`（见 [../libc/README.md](../userland/libc/README.md) 第 3、4 节）。
8. **命名与格式**：模块用 `snake_case`、类型用 `CamelCase`；提交前跑 `cargo fmt` / `cargo clippy`
   （如本机已安装）并保持与既有风格一致。

## 7. 协作流程

1. **分支**：功能/修复各开分支（本仓库由 Freebuff 使用的形如 `freebuff/<id>`），
   **不要 force push 已检出的分支**，不同开发者不要改动同一棵工作树。
2. **提交信息**沿用仓库风格：`<type>(<scope>): <简述>`，例如
   `feat(syscall v0.6): land user->kernel trampoline entry + syscall dispatch`、
   `docs(v0.4): convert interrupt known_state.md to aligned dev-doc format`。
3. **一个提交一件事**：实现 + 对应测试 + 文档同步；**不要提交构建产物/密钥**。
4. **保留锁文件**：`Cargo.lock` 不因恢复依赖而升级；不要为通过构建而随意升级依赖。
5. **PR/合并前**：确认第 9 节质量门禁通过，并在 PR 描述里贴出实测命令与结果。

## 8. 文档维护约定

1. 每份文档顶部保留 `阶段` / `最后更新` / `状态` 三行。
2. **设计变更先改文档再改代码**（文档即规格）；未开始的阶段保留占位大纲，
   但阶段内**已落地**部分要及时补记，避免文档与代码脱节。
3. 新增/调整文档时同步 [docs/README.md](./README.md) 索引与相关文档的交叉链接。
4. 文档会被反复修改：每次修订在**该文档的「修订记录」**里补一行（日期 + 变更点）。
5. 对外接口变化必须同步 [api.md](./api.md)（系统调用号/语义/errno/libc 函数/内核 trait）。

## 9. 质量门禁（合并前最低要求）

- 受影响模块对应的 `make test-*` 目标**通过**；
- 涉及用户态 / 进程 / 文件系统的改动，`make test-user`（必要时 `test-shell` / `test-proc` / `test-fs`）通过；
- 版本里程碑收官时，`make test-all` + `make go-test` + `make test-orch` 全通过；
- 文档（主题文档 + DEVELOPMENT.md 修订记录 + [api.md](./api.md)）已同步；
- 不引入会被构建产物/密钥污染仓库的改动。

> 说明：本仓库当前以 **QEMU 串口断言**作为验收证据，尚未引入 host 侧 `cargo test`
> 的纯逻辑单测（见 [../DEVELOPMENT.md](../DEVELOPMENT.md) 第 6 节计划）。

## 10. 扩展指南

### 10.1 新增一个系统调用

1. **先往 `abi/src/syscall.rs` 加号**（这是 ABI 的唯一定义点，阶段三起）：写号与名字，需要时加减进 `UNIMPLEMENTED`、
   并扩 `SYS_MAX`（沿用 Linux x86_64 语义；`0..=255` 只按 Linux 语义新增，`>= 256` 是 Cerlesse 扩展段）；
2. 在对应子模块（`file.rs`/`memory.rs`/`process.rs`…）实现 handler，返回负数 errno（用 `abi::errno::*`，不写字面量）；
3. 把它挂进 `kernel/src/syscall/syscall_table.rs` 的 `SYS_TABLE`，并从 `abi` 的 `UNIMPLEMENTED` 里去掉那条；
4. 在 `libc/src/syscall.rs` 增加封装（号/常量从 `abi` 取，**不要写第二份数字**）；
5. 加断言（`tests/test_*.py` / `--expect-re`），并同步 [api.md](./api.md) 与 [syscall.md](./syscall.md)；
6. `make abi-check`（号唯一 / 号段不重叠 / 记录尺寸）+ `make check-boundary` 必须通过 —— 它们就是 ABI 的防漂移门禁。

### 10.2 新增一个用户程序

复制 `user/hello/` 改包名 → 加进根 `Cargo.toml` 的 `members` 与 `kernel/build.rs` 的
`USER_PROGRAMS` → 在 `kernel/src/syscall/user.rs` 用 `include_bytes!` 取回并写入 RAMFS → `make user`。
细节见 [../libc/README.md](../userland/libc/README.md) 第 3 节。

### 10.3 新增一个文件系统后端

实现 `fs::vfs::FileSystem` trait（见 [api.md](./api.md) 5.1）→ 在 `fs::init` / `fs::mount_disks`
里挂到挂载表 → 加 `make test-*` 断言路由与读写 → 同步 [filesystem.md](./filesystem.md)。

### 10.4 新增一个驱动

实现 `driver::Driver` trait（`name`/`matches`/`attach`，见 [api.md](./api.md) 5.2）→
在 `register_builtin()` 注册 → 若提供块设备则再实现 `BlockDevice` 并 `block::register` →
加断言并同步 [drivers.md](./drivers.md)。

### 10.5 新增一个集成测试

在 `tests/` 加 `test_*.py`（断言串口）→ 在 `Makefile` 增加目标并加入 `test-all`
→ 在 `tests/orchestrate.json` 增加场景（供 `make test-orch`）。

## 11. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-11 | 创建本文档：环境与工具链、构建/测试/调试流程、代码与文档规范、协作流程、
质量门禁、五类扩展步骤（syscall / 用户程序 / 文件系统后端 / 驱动 / 集成测试） |
