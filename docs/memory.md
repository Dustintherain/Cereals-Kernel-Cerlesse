# 内存管理（阶段文档）

- 阶段：第三阶段（v0.3）
- 最后更新：2026-10-07
- 状态：✅ v0.3 已完成（2026-10-07 QEMU 验收通过）

> 本文档顶部已含阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：boot.md（需要 BootInfo 内存映射）

## 1. 实现总览

| 需求 | 实现 | 位置 |
| ---- | ---- | ---- |
| 解析 BootInfo MemoryMap | 遍历 UEFI 描述符数组（`desc_size` 实测 0x30），仅 `EfiConventionalMemory`（type 7）可用 | `kernel/src/memory/frame.rs` |
| FrameAllocator | 位图（1 bit = 1 帧 = 4KiB），128KiB 静态位图，管理上限 4GiB | 同上 |
| Page / 地址换算 | `Page` / `PageRange` / `align_up` / `align_down` / 4K-2M-1G 页常量 | `kernel/src/memory/page.rs` |
| 页表 Mapper | 4 级页表 translate、map_4k / unmap_4k、TLB `invlpg`、`AddressSpace` | `kernel/src/arch/x86_64/paging.rs` |
| 内核堆 | 内核映像末尾预留 1MiB + 首次适配空闲链表（带合并） | `kernel/src/memory/heap.rs` |
| 全局分配器 | `#[global_allocator]`，打通 `Box` / `Vec` / `String` | `kernel/src/memory/allocator.rs` |
| 页错误诊断 | #PF 向量名修正（B-06）+ CR2 转储 + feature 门控注入测试 | `kernel/src/arch/x86_64/interrupt.rs` |

## 2. 设计要点

### 2.1 地址空间：延续恒等映射（ADR-009）

v0.3 内核继续运行在 bootloader/固件建立的恒等映射上（`phys == virt`）：

- 页表自身可直接用物理地址访问，`translate/map/unmap` 实现最简；
- 内核堆只需保证"恒等映射存在"即可直接使用物理帧地址；
- 高半区（`0xFFFF_FFFF_8000_0000` 一带）迁移推迟到 **v0.6 用户态隔离**：重定基需要 bootloader 建表并切 CR3（link.ld 的 VMA/LMA 分离），与"内核 0xFFFF…/用户 0x0000…"一并完成，避免两次搬迁（见 docs/syscall.md）。

### 2.2 帧分配器（ADR-010）

- 位图语义：`1` = 占用、`0` = 空闲；初始化全部占用，只释放 ConventionalMemory。
- 显式保留：低 1 MiB（IVT/BDA/EBDA）、内核映像 `[__kernel_start, __kernel_end)`（链接脚本导出）、BootInfo 与内存映射缓冲、内核堆预留区。
- BootServicesCode/Data 等类型**保守保留**：ExitBootServices 后它们其实可回收，但第一版先保证正确性。
- 分配：自游标向后扫描（整字节 `0xFF` 直接跳过）；尾部耗尽后回头扫描前部；释放后游标可取回，便于复用。
- 实测（QEMU `-m 512M`）：total = 130801 帧、free ≈ 119.5k 帧（≈ 466 MiB 可用）。

### 2.3 内核堆（ADR-011）

- 位置：`__kernel_end` 起 1 MiB（链接脚本符号 + 帧分配器保留，保证连续且独占）。
- 块头 16 字节：`size`（整块大小，含块头）+ `next_free`；块起点与块大小恒为 16 字节倍数。
- 分配：首次适配；对齐前导 padding 单独留作一个空闲块（≥ 块头才合法）；尾部剩余 ≥ 32B 才切分，否则并入本次分配。
- 释放：按地址有序插入空闲链表，并与前驱/后继相邻块合并；全部释放后应合并回单一整块（自测断言）。
- 单核 + 关中断（v0.4 之前）不引入锁；v0.4 起中断上下文需要分配时再补自旋锁。

### 2.4 页表 Mapper

- `translate()` 支持固件留下的 **1 GiB / 2 MiB 大页**（OVMF 用大页铺 RAM）。
- `map_4k()` 按需从帧分配器申请中间级页表；已存在映射不覆盖，返回 `AlreadyMapped`（遇大页亦不拆分）。
- `AddressSpace`：新 PML4 根 + `map/unmap/translate`；`Drop` 递归回收各级页表与叶子映射页，因此自测在 drop 后要求帧计数完全还原。
- `ensure_identity_range()`：堆区域若缺映射就补 4 KiB 恒等映射（v0.3 实测固件已铺好，补齐页数为 0）。

## 3. 验收与测试

| 命令 | 断言（串口） | 结果 |
| ---- | ---- | ---- |
| `make test` | `Kernel started!`（v0.1 回归） | ✅ |
| `make test-exception` | `EXCEPTION: divide error`（v0.2 回归） | ✅ |
| `make test-memory` | `heap: Box/Vec/String PASS`、`frame: stress PASS`、`paging: map/unmap PASS` | ✅ |
| `make test-pagefault` | `EXCEPTION: page fault`、`CR2=0x500000000000`（feature `pagefault-test`） | ✅ |
| `make test-all` | 上述全部 | ✅ |

自测都在内核内运行，任何一项失败都不会打印 `Kernel started!`（即集成测试失败）：

1. **堆**：Box 1KiB / Vec 10000×u64 / String 800B / 64KiB 大块；再以 256 次混合大小（8–519B）与混合对齐（8–64B）分配、交错释放、逐字节填充模式校验（用于抓重叠与越界）；最后断言分配字节归零、存活块为 0、空闲链表合并回 1 块、字节账目平衡。
2. **帧**：连续分配 1024 帧 → 计数、唯一性、4KiB 对齐、范围、首尾真实读写校验 → 全部释放 → 计数还原 → 复分配得到同一组帧。
3. **页表**：新建地址空间映射 3 页 → `translate` 校验物理地址/页大小/标志 → 重复 `map` 报错 → `unmap` 返回正确帧、重复 `unmap` 报错 → 重映射 → `drop` 后帧计数还原。

串口样例（QEMU 512M）：

```text
Cerlesse kernel v0.3
GDT/TSS + IDT loaded
memory map: 0x1680 bytes, desc size 0x30
mem: frames total=130801 free=119517 (466 MiB usable)
mem: heap 0x204c000-0x214c000 (1024 KiB)
heap: Box/Vec/String PASS
frame: stress PASS (1024 frames alloc/unique/free)
paging: map/unmap PASS (fresh address space, 3 pages)
Kernel started!
```

页错误诊断样例（`make test-pagefault`）：

```text
test: touching unmapped address
EXCEPTION: page fault
vector=0xe error=0x2
RIP=0x2005319 CS=0x8 RFLAGS=0x6
RSP=0x2047be0 SS=0x10
...
CR2=0x500000000000
KERNEL PANIC: unhandled exception
```

## 4. v0.3 发现并修复的缺陷

| # | 级别 | 问题 | 修复 |
| ---- | ---- | ---- | ---- |
| B-06 | 中 | v0.2 异常名表自向量 9 起整体错位一位（漏记 #9 coprocessor segment overrun），于是 #PF 打印成 `reserved (#14)`、#GP 打印成 `page fault`；v0.2 的 #DE 测试恰好位于错位之前，未能暴露 | 按 Intel SDM Vol.3 Table 6-1 重建 32 项名表；新增 `make test-pagefault` 同时校验 #PF 名称与 CR2 |
| B-07 | 中 | v0.3 首版堆分配器把对齐前导 padding 计入已分配块大小，块尾越过自身边界覆盖后继空闲块头，表现为空闲链表自环 + 分配死循环（串口停在堆压力测试前） | 块大小改为 `want - pad`；压力测试增加填充模式校验以抓住重叠，已回归 |

## 5. 非目标 / 后续

- 内核高半区映射：推迟到 v0.6（ADR-009）。
- 大页拆分（2 MiB/1 GiB → 4 KiB）：v0.3 遇已有大页直接返回 `AlreadyMapped`，不拆分。
- 页帧换出/交换、页缓存、写时复制：非第一版目标。
- host 侧 `cargo test`，内核 crate 是 `no_std` 二进制且自带 `#[panic_handler]`，无法直接跑 host 测试框架；v0.3 验收改为**内核内自测 + QEMU 串口断言**（更贴近真实运行环境）。把纯算法抽离成 host 可构建 crate 的单测排到后续版本。

## 6. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
| 2026-10-07 | v0.3 完成：位图帧分配器、页表 Mapper/AddressSpace、内核堆与全局分配器、#PF 诊断；补记 B-06/B-07 |
