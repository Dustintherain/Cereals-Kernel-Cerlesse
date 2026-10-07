# 文件系统（阶段文档）

- 阶段：第五阶段（v0.7）
- 最后更新：2026-10-06
- 状态：⬜ 待编写 — 进入 v0.7 开发时填充

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。
- 依赖：syscall.md（open/read/write）、memory.md（缓存/页缓存）

## 计划大纲（进入该阶段时展开）

1. VFS 抽象：`trait FileSystem` / `trait File` / `trait Directory`、Inode、Mount
2. 第一版后端：RAMFS；目录骨架 `/bin /dev /etc /home /proc /tmp /usr`
3. 后续后端：FAT32 → EXT2（不把具体 FS 写死进内核）
4. 文件描述符表与 syscall 对接

## 验收标准（占位）

- [ ] Shell 中可 `cd` / `ls` / `cat` RAMFS 内文件
- [ ] VFS 接口可在不改 syscall 层的前提下替换 FS 后端

## 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
