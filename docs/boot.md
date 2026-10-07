# Bootloader / 启动（阶段文档）

- 阶段：第二阶段（v0.1 已完成 ✅；v0.2 GDT/IDT/异常 待编写）
- 最后更新：2026-10-06
- 状态：✅ v0.1 + v0.2 已完成

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：无（起点）

## 1. v0.1 已实现内容

### 1.1 启动链（已验证）

```text
OVMF (UEFI)
 → \EFI\BOOT\BOOTX64.EFI（boot crate, x86_64-unknown-uefi）
 → 串口 COM1 初始化，打印 "Cerlesse bootloader v0.1"
 → 打开 FAT 卷 \KERNEL.ELF → 解析 ELF64 程序头
 → AllocateAddress 装载 PT_LOAD 段到链接地址（32MiB）
 → GetMemoryMap → ExitBootServices（键变化自动重试）
 → 填充 shared::BootInfo（magic/memmap），以 sysv64 ABI 跳转内核入口
 → kernel _start（自建栈）→ kernel_main(RDI = &BootInfo)
 → 校验 magic → "Kernel started!"
```

### 1.2 关键实现记录

| 项 | 值 |
| ---- | ---- |
| 内核链接地址 | **32MiB（0x2000000）** — 实测 OVMF 将 0x900000–0x1500000 占为 BootServicesData，空闲常规内存从 0x1500000 起 |
| BootInfo | `shared::BootInfo`，magic = `CERLESSE`（小端 u64），位于 bootloader `.bss` |
| 内存映射缓冲 | bootloader 静态 256KiB 缓冲，desc_size 实测 0x30 |
| 验收输出 | QEMU 串口出现 `Kernel started!`（`make test` 自动断言） |

### 1.3 踩坑记录（重要）

1. **跨目标调用约定陷阱**：`boot` 编译于 `x86_64-unknown-uefi`，其 `extern "C"` 是
   **Win64 ABI（参数走 RCX）**；内核（`x86_64-unknown-none`）期望 **SysV（RDI）**。
   跳转内核必须写 `extern "sysv64" fn(*const BootInfo) -> !`。
2. **AllocateAddress(EFI_NOT_FOUND)**：固定链接地址落在固件 BootServicesData 内会分配失败，
   用内存映射诊断（打印 <64MiB 区域类型）后改为 32MiB。
3. **static mut 引用**：一律用 `addr_of!` / `addr_of_mut!`，避免 `&mut static mut` 警告。
4. **PIE 重定位陷阱（B-05，高危）**：`x86_64-unknown-none` 默认链接为 PIE，`.data` 中的
   函数地址表在文件中为 0、靠 `.rela.dyn` 加载时重定位——bootloader 不处理动态重定位段，
   导致 IDT handler 地址错误。修复：`.cargo/config.toml` 强制 `-C relocation-model=static`，
   验证方法：`readelf -h` 应为 `EXEC` 且 `readelf -r` 无任何重定位。
5. **汇编 `.quad` 绝对表在 PIE 下被 lld 拒绝**（R_X86_64_64）：函数地址表改在 Rust 侧
   由 `extern "C"` 函数项构建（RIP 相对）。

## 2. 验证方式

```bash
make image   # cargo 构建 boot+kernel → Python 组装 64MiB FAT 镜像（mtools）
make test    # QEMU+OVMF 启动，断言串口出现 "Kernel started!"
make run     # 手动启动，串口输出到 stdio
```

## 3. v0.2 已完成（GDT / IDT / Exception，2026-10-06）

- [x] 自建 GDT（0x00 null / 0x08 代码 / 0x10 数据 / 0x18 TSS）+ 远返回重载 CS + ltr
- [x] TSS：IST1 = 16KiB 双错专用栈；RSP0 = 内核栈顶（v0.4/v0.6 备用）
- [x] IDT 0..32 异常向量；stub 区分有错误码（8/10/11/12/13/14/16）；`exception_dispatch` 统一转储
- [x] 验收：`make test` 回归 PASS；`make test-exception`（feature 门控注入 `div eax`）PASS

实现位置：`kernel/src/arch/x86_64/{gdt,idt,interrupt}.rs`。

## 4. v0.3 对异常子系统的修正（B-06）

v0.3 的 #PF 诊断测试（`make test-pagefault`）发现异常**名表**自向量 9 起错位一位，
导致 #PF 被打印成 `reserved (#14)`、#GP 被打印成 `page fault`（向量号本身与栈帧都是对的，
仅名字错位；v0.2 的 #DE 测试恰好在错位之前，因此未暴露）。
已按 Intel SDM Vol.3 Table 6-1 重建 32 项名表，详见 [memory.md](memory.md) 第 4 节。

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
| 2026-10-06 | v0.1 实现完成：UEFI boot + ELF 装载 + ExitBootServices + 内核串口，QEMU 验收通过 |
| 2026-10-06 | v0.2 实现完成：GDT/TSS + IDT + 异常诊断；修复 B-01（cli 窗口）与 B-05（PIE 重定位） |
| 2026-10-07 | v0.3 复查：修正异常名表错位（B-06），新增 `make test-pagefault` 覆盖 #PF 诊断 |
