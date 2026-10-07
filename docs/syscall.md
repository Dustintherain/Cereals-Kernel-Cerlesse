# 系统调用 / 用户空间 / ELF（阶段文档）

- 阶段：第五阶段（v0.6–v0.7）
- 最后更新：2026-10-06
- 状态：⬜ 待编写 — 进入 v0.6 开发时填充
- 依赖：process.md（进程模型）、memory.md（用户地址空间）

## 计划大纲（进入该阶段时展开）

1. 用户态/内核态隔离：内核 `0xFFFF....`，用户程序 `0x0000....`
2. 第一批系统调用：`exit` `write` `read` `open` `close` `fork` `exec` `wait` `getpid` `sleep`
3. ELF Loader、User Stack、User Heap、Program Loader
4. 最小 libc / 系统调用封装（`libc/`）
5. 调用链示例：`write()` → `sys_write` → VFS → Console Driver → Screen

## 验收标准（占位）

- [ ] 用户态 `hello` 程序通过 ELF Loader 加载并运行
- [ ] `fork`/`exec`/`wait` 基本流程可用

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
