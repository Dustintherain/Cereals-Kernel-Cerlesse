# 设备驱动 / PCI / 网络（阶段文档）

- 阶段：第六阶段（v0.4 部分基础驱动；v0.8–v0.9 完整驱动与网络）
- 最后更新：2026-10-06
- 状态：⬜ 待编写 — 进入 v0.4/v0.8 开发时填充

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：interrupt（IRQ 分发）、memory（DMA 缓冲）、process（网络可后置）

## 计划大纲（进入该阶段时展开）

1. 驱动框架：Driver Manager（Block / Input / Network 分类）
2. 第一批驱动：Serial、Keyboard、Framebuffer、Timer、Disk、PCI
3. PCI 枚举：Vendor/Device ID → Driver Match
4. 后续驱动：USB、NVMe、AHCI、VirtIO、Audio、GPU
5. 网络栈顺序：Network Driver → Ethernet → ARP → IPv4 → ICMP → UDP → TCP → Socket
6. Socket API：`socket/bind/listen/accept/connect/send/recv`

## 验收标准（占位）

- [ ] PCI 设备可枚举并打印 Vendor/Device ID
- [ ] 可通过网络栈运行简易 HTTP Server

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
