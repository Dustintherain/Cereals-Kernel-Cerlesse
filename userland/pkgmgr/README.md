# pkgmgr —— Cerlesse 用户态包管理器（`ckc` + `cpm`）

这个目录是**纯内核化阶段五**从内核仓库 `tools/pkgmgr/` 迁过来的：

> 安装、卸载、依赖处理、事务编排属于**用户态管理服务**，不属于内核。
> 内核只保留两样东西：`.ckc` 的**格式规范**（[`docs/package-manager.md`](../../docs/package-manager.md)，
> 留在内核仓库）与安装期的**重新校验**（`cpm` 调 `ckc.manifest_from_data`，
> 因此「打包期检查过了」不是安装期的前提）。

| 文件 | 角色 | 规格 |
| ---- | ---- | ---- |
| `ckc.py` | **P0a**：`.ckc` 容器（`CKC\x01` 头 + 校验过的 tar payload）—— 打包、校验、`info` | `docs/package-manager.md` 3.2 / 5 节 |
| `cpm.py` | **P0b = CKM-3**：事务引擎与 CLI（`init`/`add`/`remove`/`rollback`/`status`/`ls`/`verify`/`plan`/`apply`/`recover`/`gc`） | `docs/package-manager.md` 3.5/3.6/4/7/8/9 节、`docs/ckm.md` 7 节 |
| `tests/test_ckc.py` | `.ckc` 容器的产物事实验收（确定性、头部逐字段、ustar 结构、打包期规则、系统 `tar` 独立复核） | — |
| `tests/test_cpm.py` | 事务验收（成功/原子失败/掉电恢复/回退/篡改/去重/并发锁/退出码档位） | — |

## 怎么跑

```bash
make -C userland test-pkg     # = test-ckc + test-cpm（主机侧，纯标准库，不启 QEMU）
make -C userland test-ckc
make -C userland test-cpm
```

**不需要内核源码，也不需要 QEMU**：这两份套件只操作临时目录里的 `.ckc` 与 scope。
内核仓库侧的 `make test-pkg` 只是转发到这里（找不到用户态目录时明确报错，不静默跳过）。

## 与内核的接触面

- `.ckc` 的**格式规范**与「安装期必须再校验一遍」的规则在内核仓库（`docs/package-manager.md`）；
- 内核的 CKM 只提供**机制**（能力位判定、审计记录、状态区），不解析包、不做依赖求解；
- 事务阶段（`stage → verify → activate → commit`，可回滚）与回执格式见 `docs/package-manager.md` 7 节。

## 仍未做（不在本目录内）

- `catalog sign/verify` 与 `trust add`：阻塞于开放问题 13.1（纯标准库没有 Ed25519）的定夺；
- `[requires]` 的依赖求解与镜像基线对接（P1）；
- 长期的形态是并进 `ckmd`（`ckm pkg` 子命令，见 `docs/ckm.md`），届时这里会变成它的库。
