# 内存管理（阶段文档）

- 阶段：第三阶段（v0.3）
- 最后更新：2026-10-06
- 状态：⬜ 待编写 — 进入 v0.3 开发时填充
- 依赖：boot.md（需要 BootInfo 内存映射）

## 计划大纲（进入该阶段时展开）

1. 物理内存：Frame、FrameAllocator、MemoryMap、`allocate_frame()` / `free_frame()`
2. 虚拟内存：Page、PageTable、Mapper、AddressSpace
3. 内核堆：Kernel Heap → Global Allocator → Rust alloc（`Box`/`Vec`/`String`/`Arc`/`Mutex`）
4. 页错误处理与内核堆调试（边界标记、分配追踪）

## 验收标准（占位）

- [ ] 帧分配器通过压力测试
- [ ] 内核中可正常使用 `Vec`/`Box`
- [ ] 页错误能输出准确诊断

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
