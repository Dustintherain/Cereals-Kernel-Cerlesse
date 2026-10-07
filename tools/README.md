# tools/ — 主机侧 Python 工具

语言分工见 docs/architecture.md 第 2 节：Python 只运行在开发主机，不进入内核。

| 工具 | 用途 | 计划版本 |
| ---- | ---- | ---- |
| `image-builder/` | 组装 boot + kernel + 文件系统磁盘镜像 | v0.1 |
| `mkfs/` | RAMFS/FAT 镜像生成 | v0.7 |
| （tests/ 驱动） | QEMU 启动与串口日志断言 | v0.1 |

状态：⬜ 占位 — 各脚本在对应版本创建。
