# 总体开发路线图（第一阶段）

- 阶段：第一阶段 / 路线规划
- 最后更新：2026-10-06

## 1. 总体开发路线

```text
Boot / 启动（Bootloader → CPU初始化 → Kernel入口）
        ↓
Kernel 基础层（GDT / IDT / Exception / Panic）
        ↓
内存管理（Physical → Virtual → Heap → Allocator）
        ↓
中断与时钟（PIC/APIC → Timer → Keyboard）
        ↓
进程 / 线程（Task → Context → Scheduler → IPC）
        ↓
系统调用（syscall → Process → File → Memory）
        ↓
文件系统（VFS → FAT/EXT → File → Directory）
        ↓
驱动系统（PCI → Block → USB → Input → Display）
        ↓
用户空间（ELF → libc → init → shell）
        ↓
网络系统（Ethernet → IP → TCP/UDP → Socket）
        ↓
完整系统（Shell + Utilities + Services + Apps）
```

## 2. 实际开发顺序（严格按依赖关系）

```text
① Bootloader → ② Kernel Entry → ③ Serial Console → ④ GDT/IDT → ⑤ Exception
→ ⑥ Physical Memory → ⑦ Virtual Memory → ⑧ Kernel Heap → ⑨ Interrupt
→ ⑩ Timer → ⑪ Keyboard → ⑫ Task → ⑬ Context Switch → ⑭ Scheduler
→ ⑮ Process → ⑯ Syscall → ⑰ User Mode → ⑱ ELF Loader → ⑲ VFS
→ ⑳ RAMFS → ㉑ Disk Driver → ㉒ PCI → ㉓ Network → ㉔ Shell → ㉕ User Applications
```

**禁止提前做的内容**：GUI、网络、USB、GPU —— 这些都建立在前面的内核基础设施之上。

## 3. 里程碑（v0.1 – v1.0）

| 版本 | 目标 | 对应文档 |
| ---- | ---- | ---- |
| **v0.1 ✅** | Bootloader + Rust Kernel（2026-10-06 QEMU 验收通过） | boot.md |
| **v0.2 ✅** | GDT + IDT + Exception（2026-10-06 验收通过） | boot.md / memory.md |
| **v0.3** | Physical/Virtual Memory + Heap | memory.md |
| **v0.4** | Interrupt + Timer + Keyboard | drivers.md |
| **v0.5** | Process + Thread + Scheduler | process.md |
| **v0.6** | Syscall + User Space | syscall.md |
| **v0.7** | VFS + RAMFS + ELF | filesystem.md / syscall.md |
| **v0.8** | Driver + PCI + Disk | drivers.md |
| **v0.9** | Shell + User Programs + Network | drivers.md / filesystem.md |
| **v1.0** | 完整可启动的类 Linux 小型 OS | — |

每个版本的详细任务清单见 [../DEVELOPMENT.md](../DEVELOPMENT.md)。

## 4. 阶段划分（与文档对应）

1. **第一阶段（已完成）**：架构 + 路线 + 结构 + 文件框架。
2. **第二阶段（v0.1–v0.2）**：Boot、GDT/IDT、异常处理 —— 编写 boot.md。
3. **第三阶段（v0.3–v0.4）**：内存与中断时钟 —— 编写 memory.md。
4. **第四阶段（v0.5）**：进程/调度 —— 编写 process.md。
5. **第五阶段（v0.6–v0.7）**：系统调用/用户态/文件系统 —— 编写 syscall.md、filesystem.md。
6. **第六阶段（v0.8–v0.9）**：驱动/PCI/网络/Shell —— 编写 drivers.md。
7. **第七阶段（v1.0）**：整合、安全机制、发布。

## 5. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段初稿：10 个里程碑 + 25 步依赖顺序 |
| 2026-10-06 | v0.1 完成，里程碑表标记 ✅ |
| 2026-10-06 | v0.2 完成（GDT/IDT/异常 + B-01/B-05 修复），里程碑表标记 ✅ |
