# 进程 / 线程 / 调度（阶段文档）

- 阶段：第四阶段（v0.5，上下文切换在 v0.4 末启动）
- 最后更新：2026-10-06
- 状态：⬜ 待编写 — 进入 v0.5 开发时填充

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：memory.md（内核堆）、interrupt（Timer 驱动调度）

## 计划大纲（进入该阶段时展开）

1. Process 结构：Address Space、File Table、Signal、Threads
2. Thread 结构：Registers、Stack、State、Scheduling Info
3. 状态机：NEW → READY → RUNNING → WAITING → READY → TERMINATED
4. 上下文切换：save registers → Scheduler → restore registers
5. 第一版调度器：Round Robin（10ms 时间片）；后续 Priority / CFS-like / Real-time
6. IPC：pipe、channel、shared_memory

## 验收标准（占位）

- [ ] 两个内核任务可来回切换且不崩溃
- [ ] Timer 中断驱动的时间片轮转可观察到

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
