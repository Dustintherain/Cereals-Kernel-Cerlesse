# Cerlesse OS 包管理器设计提案（H-06）

- 阶段：跨阶段 / 主机侧能力（H-06；不阻塞 v0.1–v0.8 内核主线）
- 最后更新：2026-10-12
- 状态：🟡 **提案 v1 + P0a/P0b 已落地** —— 设计规格（格式/安全模型/事务语义/CLI/验收）已定；
  **包容器 `.ckc`（P0a）与事务引擎（P0b = CKM-3）都已实现**：
  `userland/pkgmgr/ckc.py`（`pack`/`verify`/`info`）+ `userland/pkgmgr/cpm.py`
  （`init`/`add`/`remove`/`rollback`/`status`/`ls`/`verify`/`plan`/`apply`/`recover`/`gc`）；
  `make test-pkg` = `test-ckc`（37 项）+ `test-cpm`（**63 项**），两份都含独立复核；
  catalog 签名/`trust`（开放问题 13.1 未定夺）与内核侧接入（P1）仍待做，见第 11 节（P0b 落地清单见 11.1）
- 关联：**[ckm.md](./ckm.md)（CKM 设计提案：本文的 P0b 事务/回执/签名 = CKM 阶段 3，用户态 `cpm` 合并进 `ckm pkg`；内核侧消费点由 CKM 的 `ckm_component_state` + 状态树提供）** ·
  [DEVELOPMENT.md](../DEVELOPMENT.md)（H-06 需求行）· [docs/README.md](./README.md)（索引）·
  [docs/filesystem.md](./filesystem.md)（VFS/RAMFS/FAT 现状）· [docs/syscall.md](./syscall.md)（用户程序约束）·
  [docs/lang-selection.md](./lang-selection.md)（语言分工）· [docs/project-structure.md](./project-structure.md)（`tools/` 归属）

> 本文档顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。
> 约定：**文档即规格** —— 本文里带「必须」的条款是参考实现要照着做并被验收的条款，
> 其余是理由与开放问题。文中不写「大概」「可能」这类无法验收的表述。

## 1. 目标、验收面与非目标

### 1.1 目标（来自 H-06）

设计一个面向内核/系统生态的包管理器，同时满足四个词，且每个词都能落成可验收的条款：

| 目标 | 落成什么条款 | 在哪一节 |
| ---- | ---- | ---- |
| **高效** | 内容去重（文件粒度）、幂等（已激活修订零写入）、按需元数据、无全量依赖求解 | 4 / 6 |
| **安全** | 修订 = 内容哈希；目录（catalog）签名；**纯文件安装、不执行任何安装脚本**；scope 路径白名单；安装区互不越权 | 5 |
| **轻量** | 状态 = 回执（receipt）+ 真实文件，不设全局包数据库；元数据上限与解析预算；管理器本体只做 5 件事 | 3.5 / 6.1–6.3 |
| **灵活（Windows 式体验）** | 多来源（channel）共存、按 scope 安装、查询/安装/卸载/修补/回退全命令化，但每条意图映射到固定安全语义 | 1.2 / 7–9 |

用户视角的一句话：**像 Windows 包管理器那样「能查、能装、能卸、能修、能回退、多来源」，但不能因此把系统变成不可控。**
灵活性体现在「意图可以很多种」，而不是「同一种意图可以有不同的安全含义」。

### 1.2 「Windows 式灵活性」逐条落到本提案的哪里

| Windows 包管理器式的体验 | 本提案的落点 | 约束（灵活性不越过哪条线） |
| ---- | ---- | ---- |
| 同时管理多种不同类型的包 | `category`：`system` / `driver` / `userland` / `service` / `image-baseline`（3.1） | `driver`/`system` 必须声明 `[abi].kernel`，且只能装到 `system` scope |
| 「不管哪来的都能查到」 | 多渠道并行：`channels/*.toml` + 每渠道签名 catalog（3.3、9.1 `search`/`info`） | 每个渠道必须有显式信任的公钥，未信任就拒绝 |
| 「查任何东西」 | `search` / `info` / `status` / `ls` / `verify`，均可 `--json`（9.1） | 查询不写状态；`search` 只看已验签的 catalog |
| 「装任何东西」 | `cpm add`（命令式）+ `cpkg.toml`（声明式），可指定 scope（9.1） | 路径白名单（8.2）+ 纯文件安装（5.3）+ setuid 拒绝 |
| 「卸任何东西」 | `cpm remove`（按 receipt 删文件，保留用户改过的配置；`--purge` 才连配置一起删） | 只删 receipt 记录的文件，不碰未知文件 |
| 「修任何东西」/局部更新 | 换修订即修补（内容变 ⇒ rev 变 ⇒ 新事务）；`cpm verify` 检出自愈（4.3、6.3） | 不做同 rev 的就地打补丁；不做安装脚本 |
| 回退到旧版本 | `cpm rollback` 回到 `prev_rev`（第 8/9 节） | `prev_rev` 的 blob 必须在 store ⇒ 纯本地、不触网（5.2） |
| 多来源/开发版/自动更新统一检索 | 渠道优先级 + `cpkg.toml` 锁定 rev（3.3、3.4） | 不许静默降级（4.3）、不许隐式安装依赖（4.5） |
| 「能装给当前用户，也能装给整机」 | scope：`user` / `shared` / `system`（第 8 节） | 内核级内容与用户态内容分开描述、分开交付语义（5.2） |
| 语言/组件按功能领域混搭（Rust 核心 + Python 胶水等） | 包只描述「文件 + 元数据」，不关心谁编的（`docs/lang-selection.md`） | 仍要求可验证的内容修订；语言自由不等于来源自由 |

### 1.3 本提案不做什么（非目标）

- 不做图形化商店、不做系统设置中心、不做权限管理大脑（本体只做第 6.2 节的五件事）；
- **不做通用版本求解器**（SAT/回溯）。P0/P1 只支持「精确名字」与「精确修订或该名字的最新修订」，
  依赖只记录、只校验，不隐式拉取（第 4.5 节）；
- 不做安装脚本/钩子（`hooks` 必须为 `false`，见 5.3）——这是结构性安全保证，不是暂缺功能；
- 不做内核态包管理器。P0/P1 全在主机侧；内核侧只保留「未来的消费点」（第 10.2 节）；
- 不做 P2P/去中心分发、不做增量二进制补丁（P2 之后，见 11.3）；
- 不替换现有构建流程（`make image` / `tools/image-builder` / `tools/mkfs`），二者是并存的交付路径。

---

## 2. 术语与数据模型

术语定义到「能写进代码」的粒度。命名沿用本文与 CLI：**`cpm`** = Cerlesse Package Manager。

| 术语 | 定义（字段级） |
| ---- | ---- |
| **包（package）** | 由 `PKG.toml` 描述的一个可分发单元：`name` + `category` + 一组带目标路径的文件 + 元数据。 |
| **修订（revision）** | 包内容的一个不可变状态，标识为 `rev = "<algo>:<digest>"`（P0 固定 `sha256:` + 64 位小写十六进制）。**身份是内容，不是版本号**。 |
| **版本（version）** | 人对修订的别名（`1.2.3`），只存在于 catalog 里；同一个 `version` 可以指向不同 `rev`（例如重新构建过），此时以 `rev` 为准并告警。 |
| **来源 / 渠道（channel）** | 一个可验证的包提供方：本地目录、内网仓库、镜像附带区。渠道 = 「catalog 地址 + 公钥指纹 + 优先级」。 |
| **目录（catalog）** | 某渠道在某代（generation）上「有哪些包、哪些修订、多大」的签名清单。**按需拉取**，不是常驻数据库。 |
| **意图（intent）** | `cpkg.toml` 里声明的期望状态（想要哪些包、锁到哪些 rev、装到哪个 scope）。 |
| **安装区（scope）** | 交付物的落点与生命周期分类：`image-baseline` / `system` / `shared` / `user`（第 8 节）。 |
| **回执（receipt）** | 每个 scope 下每个包一个文件，记录「这个包在这个 scope 里激活了哪个 rev、写了哪些文件、每个文件的哈希与模式」。**这就是「已安装状态」**。 |
| **存储库（store）** | 内容寻址的文件仓库 `store/<algo>/<xx>/<digest>`。按内容命名 ⇒ 天然去重、天然可校验。 |
| **事务（transaction）** | 一次带日志（journal）的变更：准备 → 校验 → 激活 → 提交；失败可回放回滚（第 7 节）。 |
| **基线（baseline）** | 出厂/镜像自带、cpm 只读不写的内容（`image-baseline` scope）。 |

### 2.1 状态从哪来（「追踪优先于枚举」的落地）

cpm **不维护**「所有已安装包 + 所有依赖」的中心数据库。三处信息合起来就是全部状态，任一处都能独立回答自己的那部分问题：

1. `receipts/<scope>/<name>.toml` —— 谁被激活了、激活的是什么内容；
2. 磁盘上的文件本身（`stat` 即可判断存在/大小/mtime）；
3. `cpkg.toml` —— 人类声明的意图（可能与实际不一致，`cpm plan` 的职责就是算出这个差异）。

「已准备但未激活」的内容在 `txn/` 与 `store/` 里，**不算生效状态**（5.4）。

---

## 3. 制品与文件格式（全部给出字段）

根目录（主机侧原型里是一个目录，将来可以是系统根；`cpm --root <dir>` 指定）：

```text
<root>/
  cpkg.toml                    # 意图清单（可选；命令式用法下可以没有）
  channels/<name>.toml         # 渠道定义（来源 + 公钥指纹 + 优先级）
  trust/<keyid>.pub            # 信任的公钥（显式加入，见 5.1）
  catalog/<channel>.toml       # 缓存的 catalog（按需拉取）
  catalog/<channel>.toml.sig   # catalog 的分离签名
  store/sha256/<xx>/<digest>   # 内容寻址的 blob（xx = digest 前 2 位，分桶避免单目录过大）
  scopes/<scope>/...           # 每个安装区的实际交付物（P0 里是主机侧 staging 目录）
  receipts/<scope>/<name>.toml # 回执（= 该 scope 下该包的已激活状态）
  txn/<utc>-<pid>.jsonl        # 事务日志（append-only）
  cache/                       # 可清理的临时层（下载中转、catalog 原始字节）
```

### 3.1 `PKG.toml` —— 包定义（作者侧，`cpm pack` 的输入）

```toml
[package]
name        = "hello"
version     = "0.7.1"          # 人的别名，不参与身份
category    = "userland"       # system | driver | userland | service | image-baseline
scope       = "user"           # 该包允许被安装到的安装区（见 8.2 的白名单）
architecture = "x86_64-unknown-none"
license     = "MIT"

[abi]                          # category = "system" | "driver" 时**必须**有
kernel = ">=0.7.0"             # 声明它依赖的内核接口版本
user   = ">=0.6.0"             # 可选：只对 userland/service 有意义

[requires]                     # P0/P1：只记录、只校验，**不隐式安装**（4.5）
"libc-runtime" = { rev = "sha256:<hex>" }

[install]
hooks = false                  # 必须为 false（5.3）；为 true 的包直接被拒绝

[[files]]
source = "bin/hello"
target = "/usr/bin/hello"
mode   = 0o755

[[files]]
source = "README.md"
target = "/usr/share/doc/hello/README"
mode   = 0o644
```

字段规则（必须）：

- `files[].target` 必须是**绝对路径**、规范化后不含 `..`，且落在该 `scope` 的白名单前缀内（8.2），否则打包/安装都报 `EINVAL`；
- `files[].mode` 只允许 `0o600`–`0o755` 的常规权限位；**置位 `0o4000`(setuid)/`0o2000`(setgid)/`0o1000`(sticky) 直接拒绝**；
- `category = "driver"` 的包必须同时声明 `[abi].kernel`，且 `scope` 只能是 `system`；
- 同一 `target` 在一个包里出现两次 ⇒ `EINVAL`（不靠「后者覆盖」这种隐式行为）。

### 3.2 `.ckc` —— 包容器（CKC = Cerlesse Kernel Container）

**后缀 `.ckc` 是 Cerlesse 内核包的唯一合法后缀**（`cpm pack` / `userland/pkgmgr/ckc.py pack` 的输出）。
后缀 + 文件头魔数让一个包**一眼可辨、不会被当成普通 tar 随便解开**，同时把校验所需信息放在最前面：
读 64 字节头就能判断版本、是否被截断、以及 payload 的 SHA-256，不必先解析整个 tar。

```text
偏移  长度  字段             说明
0     4     magic            b"CKC\x01"（"CKC" + 容器版本 1）
4     2     header_len       LE u16 = 64（头长固定，方便以后扩展时仍能定位 payload）
6     2     flags            LE u16 = 0（v1 未定义任何位；非 0 = 未知特性 → 拒绝）
8     8     payload_len      LE u64（payload 字节数）
16    8     payload_off      LE u64 = 64（payload 起始偏移）
24    32    payload_sha256   payload 的 SHA-256（**32 字节原始摘要**）
56    8     reserved         必须全 0（写入 0，读取时非 0 → 拒绝）
```

整型一律**小端**；头长固定 64，`payload_off` 目前恒为 64（字段仍写出来，因为要能拒绝「头长不匹配」的伪造包）。

**身份 = payload 的哈希**，不是整个文件的哈希：同样的文件内容 ⇒ 同样的 `rev`，
与打包时间、机器、头部字节无关。头部字段本身在 `verify` 时被逐一检查，但不参与身份。

payload 是**确定性未压缩 ustar**。规则必须全部满足，否则同一目录树两次打包的字节不同、`rev` 就会漂移：

| 规则 | 取值 | 理由 |
| ---- | ---- | ---- |
| 归档名 | **安装目标路径去掉开头的 `/`**（如 `usr/bin/hello`） | 容器的含义自解释：`info` 不读 `PKG.toml` 也能看出文件会落到哪 |
| 条目顺序 | `PKG.toml` 第一，其余按归档名的**字节序**排序 | 消除文件系统迭代顺序的影响 |
| 格式 | POSIX ustar（`ustar\0` + `00`），单条路径 ≤ 100 字节 | 不用 PAX 扩展头，避开「扩展头字段顺序」这种确定性陷阱 |
| `mtime` / `uid` / `gid` / `uname` / `gname` | 全 `0` / 空 | 打包时间与构建者身份不进入产物 |
| 模式 | 只取 `PKG.toml` 显式给的权限位；`setuid`/`setgid`/`sticky` 一律拒绝 | 见 5.2；不受构建机 umask 影响 |
| 只允许常规文件 | 目录条目 / 符号链接 / 设备 / 稀疏文件 → 报错 | 目录由解包方按路径创建，避免目录条目的顺序问题 |
| 补零 | 数据块之后的补齐字节必须为 0；结尾恰好两个全零块 | 「逐字节相同」要能验收，就不能留下未定义字节 |
| 压缩 | v1 **不压缩**（`flags` 必须为 0） | 压缩算法与参数会成为身份的一部分；去重（4.4）比压缩更划算 |

与 `tools/mkfs/mkfs.py` 的做法一致（固定时间戳 ⇒ 同一目录树逐字节相同），
这样 `.ckc` 的 `rev` 在任何机器上可复现。

**命名演变**：提案 v1 里这个后缀叫 `.cpk`；实现时改用 **`.ckc`**
（Cerlesse Kernel Container）—— 与项目名同源、和现有 `.img`/`.elf` 产物不混，
而且「CKC」本身就说明它是内核包容器。`.cpk` 不再被识别（`ckc.py verify` 直接拒绝）。

**已落地的部分（P0a，2026-10-11）**：`userland/pkgmgr/ckc.py`（纯标准库）实现
`pack` / `verify` / `info`，退出码与 9.2 一致；`make test-pkg`（`userland/pkgmgr/tests/test_ckc.py`）**37 项断言**通过，
包括确定性、头部逐字段、payload 篡改、ustar 结构规则、打包期规则（超长路径/越界 target/setuid/重复 target/hooks），
以及**独立复核**：用系统 `tar` 读同一段 payload，条目与内容必须与 `ckc.py` 一致（不是自己验自己）。
**仍未做（P0b 的剩余面）**：catalog 签名与 `trust` 公钥管理（开放问题 13.1 未定夺，见 11.1）；
事务/回执/store/`recover` 已随 P0b 落地（`userland/pkgmgr/cpm.py`）。

### 3.3 `catalog.toml` + `catalog.toml.sig` —— 渠道清单

```toml
[catalog]
channel    = "local-dev"
generation = 12                # 单调递增；回退到更小 generation 时要求 --allow-downgrade
created_at = "2026-10-11T00:00:00Z"

[[package]]
name     = "hello"
version  = "0.7.1"
rev      = "sha256:<hex>"
size     = 20480
category = "userland"
scope    = "user"
```

- **签名是对 `catalog.toml` 的完整字节做 Ed25519 分离签名**（`catalog.toml.sig`，64 字节原始签名）；
- 校验顺序固定：**先验签，后解析**。签名不过 ⇒ 整个 catalog 不可用（`EVERIFY`），不进入「部分可用」状态；
- `catalog` 里缺失的字段（如 `scope`）表示「以 `PKG.toml` 为准」，与 `PKG.toml` 冲突时以 `PKG.toml` 为准并告警（catalog 只是索引，包自身的定义才权威）。

### 3.4 `cpkg.toml` —— 意图清单（声明式入口）

```toml
[settings]
channel = "local-dev"          # 默认渠道
scope   = "user"               # 默认安装区

[[want]]
name  = "hello"
rev   = "sha256:<hex>"         # 可选：锁定到精确修订（推荐，可复现）
scope = "user"                 # 可选：覆盖默认 scope
```

约束：`[[want]]` 之间不允许同名不同 `rev`（`EINVAL`）——「同时想要两个修订」不是一个有意义的意图，
需要多修订共存时按 scope 区分。

### 3.5 `receipts/<scope>/<name>.toml` —— 回执（已激活状态）

```toml
[receipt]
name       = "hello"
scope      = "user"
rev        = "sha256:<hex>"
prev_rev   = "sha256:<hex>"     # 可为空；rollback 的落点
channel    = "local-dev"
activated_at = "2026-10-11T00:00:00Z"

[[files]]
target = "/usr/bin/hello"
digest = "sha256:<hex>"         # 该文件的哈希（用于 verify / 篡改检测）
size   = 4096
mode   = 0o755
```

`cpm status` 只读这部分 + `stat` 文件，不读 catalog、不建索引（6.3 的预算就靠这一点）。

### 3.6 `txn/<utc>-<pid>.jsonl` —— 事务日志

append-only，每条一行 JSON，带 `seq` 单调递增（第 7 节给出状态机）：

```json
{"seq":1,"op":"begin","intent_digest":"sha256:<hex>"}
{"seq":2,"op":"stage","target":"/usr/bin/hello","blob":"sha256:<hex>"}
{"seq":3,"op":"verify","name":"hello","rev":"sha256:<hex>","result":"ok"}
{"seq":4,"op":"backup","target":"/usr/bin/hello","backup":"txn/<id>/0"}
{"seq":5,"op":"activate","target":"/usr/bin/hello","blob":"sha256:<hex>"}
{"seq":6,"op":"commit"}
```

---

## 4. 内容标识、校验与依赖

### 4.1 身份 = 内容哈希

- 标识写作 **带算法前缀的字符串**：`sha256:<64 hex>`。P0 只实现 `sha256`（Python 标准库 `hashlib` 直接可用，
  零第三方依赖 —— 与 `tools/mkfs/mkfs.py` 纯标准库的做法一致）；
- 算法可替换：所有比较都按前缀分派，将来加 `blake3:` 不需要改格式（BLAKE3 更快，但包元数据是 MiB 量级，
  SHA-256 的速度完全够，不值得为此引入依赖）；
- **任何**写进 receipt/store 的内容都必须先算出 rev/hash，不允许「先写后算」。

### 4.2 三级校验（缺一层就会出现「路径对了但内容是别的」）

| 层 | 校验对象 | 失败结果 |
| ---- | ---- | ---- |
| 1. 信任 | `catalog.toml.sig` 对 `catalog.toml` 字节 | `EVERIFY`（渠道整体不可用） |
| 2. 绑定 | catalog 里的 `name → rev`，以及 `.ckc` 自身的 `sha256` | `EVERIFY`（该包不可用，事务不开始） |
| 3. 落地 | 每个文件写进 scope 后，重读并与 receipt 记的 `digest` 比对 | `EINTEGRITY`（事务回滚） |

第 3 层必须**在激活之后、提交之前**做，这样「commit 成功」就意味着「磁盘内容等于期望内容」。

### 4.3 篡改与降级

- blob 进 store 前必须校验：不等 ⇒ 丢弃临时文件 + `EVERIFY`，**不写入 store**；
- catalog `generation` 回退（渠道被回滚/被换掉）默认拒绝，需显式 `--allow-downgrade`；
- 文件被就地篡改（在 scope 里改了已交付文件）：`cpm verify` 用 receipt 的 `digest` 检出，报 `EINTEGRITY` 并指出路径；
  `cpm apply` 会把它视为「未激活」并重新激活（自愈），但不自动删除未知文件。

### 4.4 去重（文件粒度）

store 按内容寻址 ⇒ 相同内容的文件在多个包/多个 scope 里只存一份。
scope 里的落地优先**硬链接**到 store，平台不支持时**回退为复制**（两者都必须在 receipt 里记 `digest`，
保证 `verify` 与硬链接/复制无关）。P0 不做跨包二进制差分，去重带来的收益先把「同一份内容不存两遍」拿满。

### 4.5 依赖：记录 + 校验，不隐式安装

- `[requires]` 写进 receipt（供 `status`/`verify` 展示）；
- `verify` 阶段检查「要求的名字在当前 scope 已激活且 rev 匹配」；不满足 ⇒ 事务失败并**打印缺哪个包、
  应该装哪个 rev**，由使用者显式再执行一次 `cpm add`；
- 理由：隐式拉取与递归求解是包管理器里最容易失去可预测性的部分，也是「巨型数据库」的起点。
  精确名字 + 精确 rev + 显式命令，系统状态永远等于「人写下的意图」；
- 版本区间求解明确推迟（非目标），需要时再看是否值得引入。

---

## 5. 安全模型

### 5.1 信任根：显式加入的公钥

- 渠道 = `catalog` 地址 + **Ed25519 公钥指纹**（`keyid` = 公钥哈希前 16 位十六进制）+ 优先级；
- 公钥只能通过 `cpm trust add <file>` 显式加入，命令会**打印指纹并要求确认**；不存在静默 TOFU；
- 未知 `keyid` 签的 catalog ⇒ 拒绝（`EVERIFY`），提示「用 `cpm trust add` 加入该渠道公钥」；
- 公钥交换不在本提案范围内（P0 用文件路径/命令行给出）。

### 5.2 域边界（内核级 vs 用户态 vs 共享）

| 边界 | 规则 |
| ---- | ---- |
| `category` 与 `scope` | `system`/`driver` ⇒ 只能 `system` scope；`userland`/`service` ⇒ `shared` 或 `user`；`image-baseline` 只能由镜像构建产生，cpm 不安装它 |
| 路径白名单 | 按 scope 定义（8.2）；越界 ⇒ `EINVAL`，**且不写任何文件**（先全量检查再开始事务） |
| ABI | `driver`/`system` 必须声明 `[abi].kernel`；`cpm add` 在本机内核接口版本不满足时拒绝（P0 里「本机内核版本」由 `--kernel-abi` 参数或 `channel` 元数据给出，显式输入而非猜测） |
| 特权 | P0/P1 一律拒绝 setuid/setgid/sticky 位；不装到系统目录之外的白名单路径 |
| 回退 | `prev_rev` 的 blob 必须仍在 store（引用计数保证）⇒ `rollback` **不需要网络** |

### 5.3 纯文件安装（最重要的一条）

- **不允许安装脚本/hook**：`[install].hooks = true` 的包被拒绝；P0/P1 不提供任何 hook 机制。
  安装 = 「把校验过的 blob 放到白名单路径上」。这是「依赖更新不会导致系统回归崩坏」的结构性保证 ——
  不执行第三方代码，就没有「安装时被换掉」的攻击面；
- 因此「修补」只能通过**换修订**表达（内容变了 ⇒ rev 变了 ⇒ 新事务），不做「同一 rev 的就地打补丁」；
- 需要配置文件的包，交付模板文件到 `/usr/share/...`，**不覆盖**用户已存在的配置文件
  （目标路径若属于「配置文件白名单」且在 scope 中已存在，则跳过并记 `skip (config exists)`）。

### 5.4 未激活 ≠ 生效

`store/` 里的一切都是「材料」，`scopes/` 里被 receipt 记录的文件才是「生效」。
查询、下载、校验失败都不会改变生效状态；`cpm status` 必须能明确区分「已准备」与「已激活」（第 7.4 节）。

### 5.5 威胁与对策（逐条对应到上面的机制）

| 威胁 | 对策 |
| ---- | ---- |
| 渠道被换/中间人改 catalog | catalog 签名 + 显式信任的公钥（5.1） |
| 内容被替换成别的包 | `name → rev` 绑定在签名内 + 内容哈希（4.2） |
| 传了一半/被截断的内容 | 长度与哈希都进 catalog；不符即 `EVERIFY` |
| 恶意安装脚本 | 不存在 hook 机制（5.3） |
| 包越权写系统目录 | scope 路径白名单 + 全量预检查（5.2） |
| setuid 提权 | 权限位直接拒绝（3.1） |
| 安装一半断电/失败 | 事务日志 + 回滚（7） |
| 已交付文件被就地篡改 | receipt 里逐文件 `digest`，`verify` 检出（4.3） |
| 渠道降级（回滚到旧 catalog） | generation 单调 + 显式 `--allow-downgrade`（4.3） |

---

## 6. 效率与轻量

### 6.1 不维护全局大库

- 不设「所有包 + 所有依赖 + 所有文件」的中心数据库；状态 = receipt + 磁盘（2.1）；
- catalog 按需拉取、可丢弃（`catalog/*` 与 `cache/` 随时可删，删掉只损失一次网络往返）；
- 不常驻内存的索引结构；解析上限见 6.3。

### 6.2 管理器本体（只做五件事）

```text
① 解析意图（命令 / cpkg.toml）
② 定位修订（channel → catalog → rev）
③ 校验内容（签名 → 绑定 → 落盘后逐文件）
④ 编排事务（stage → verify → activate → commit，可回滚）
⑤ 记录可观测结果（receipt / journal / 人类可读日志）
```

不在这五件事里的能力（商店 UI、账户、遥测、系统设置）都不属于本体。

### 6.3 预算（可测，验收时按此断言）

| 指标 | 预算 | 说明 |
| ---- | ---- | ---- |
| catalog 大小 | 单渠道 ≤ 1 MiB | 超出即拒绝解析（`EFBIG`），强制渠道分片 |
| `cpm status` | 不读 catalog、不扫 store；O(包数) 次 `stat` + 读 receipt | 冷启动耗时可测，量级与包数线性 |
| `cpm apply` 幂等 | 目标 rev 已激活 ⇒ **零写入**，日志 `skip (already active)` | 用文件 mtime/写入计数断言 |
| 去重 | store 文件数 = 唯一内容数（不随包数增长而重复） | `cpm gc` 前断言 |
| 事务失败 | 生效状态不变（`scopes/` 无半成品） | 注入失败后逐字节比对 |
| `rollback` | 纯本地（store 命中即不触网） | `--offline` 下必须成功 |

### 6.4 缓存与回收

- `cache/` 是临时层：下载中转、catalog 原始字节，可任意清理；
- `cpm gc` 回收「没有被任何 receipt 引用、且不在当前意图里」的 blob 与旧 `txn/` 记录；
  `gc --dry-run` 必须能先列出将删除的内容（默认不删）。

---

## 7. 事务模型

### 7.1 阶段（每个阶段失败都有确定的后果）

```text
解析意图 → 规划（plan）→ 准备（stage）→ 校验（verify）→ 激活（activate）→ 提交（commit）
                                                        ↘ 任意一步失败 → 回滚（rollback）
```

| 阶段 | 做什么 | 失败后果 |
| ---- | ---- | ---- |
| plan | 计算「意图 vs receipt」差异；决定目标 scope、目标 rev；**全量路径白名单检查** | 不写任何文件，`EINVAL`/`EVERIFY` |
| stage | 把 blob 放进 store（内容不落地到 scope） | `EVERIFY`（哈希不符）/ 磁盘错误 `EIO` |
| verify | 依赖校验（4.5）、空间/冲突校验、确定要写的文件清单 | `EINVAL`/`ENOSPC`，不改变生效状态 |
| activate | 把文件写进 scope；已存在的同路径文件先备份进 `txn/<id>/` | 回滚（恢复备份），`EINTEGRITY` |
| commit | 逐文件重读校验 → 写新的 receipt（含 `prev_rev`）→ 追加 `commit` 记录 | 回滚 |
| rollback | 按 journal 逆序恢复备份、删除本次新增的文件、丢弃半成品 receipt | 记录 `rollback`，返回事务失败码 |

### 7.2 原子性到哪一步

P0 的原子性边界是**每个文件**（`rename` 到目标路径），不是整个 scope。
理由：跨多个文件/多个 scope 的真正原子需要文件系统级事务，这会与「轻量」冲突。
补偿措施：journal 让**中断后可恢复**（`cpm recover` 读 `txn/` 里未 `commit` 的事务并回滚），
即「不会留下半成品被当作生效状态」。

### 7.3 并发

- 单写者：`<root>/cpm.lock`（`O_CREAT|O_EXCL` 创建，记录 pid 与时间）；拿不到锁 ⇒ `ELOCK`；
- 读者（`status`/`ls`/`info`）不加锁，只读 receipt，因此可以随时运行；
- 锁不能靠「文件存在与否」判断死活：锁文件里写 pid + 启动时间，`recover` 时若 pid 不存在 ⇒ 视为陈旧锁并提示（不自动删）。

### 7.4 可观测的三种状态（`status` 必须能分别回答）

| 状态 | 来源 | 含义 |
| ---- | ---- | ---- |
| **已激活** | `receipts/` | 已生效，且文件应与 receipt 一致（可用 `verify` 核对） |
| **已准备** | `store/` + 未 `commit` 的 `txn/` | 材料已就位，未生效 |
| **基线** | `image-baseline` scope（镜像/只读卷） | 出厂内容，cpm 不写、只报告 |

---

## 8. 安装区（scope）与到 Cerlesse 的映射

### 8.1 四个 scope

| scope | 生命周期 | 可写性 | 回退策略 |
| ---- | ---- | ---- | ---- |
| `image-baseline` | 与镜像同寿 | **只读**（cpm 只报告，不写） | 换镜像，不由 cpm 回退 |
| `system` | 整机 | 可写（依赖文件系统支持） | 严格：必须能回退，否则拒绝安装 |
| `shared` | 整机、多用户共享 | 可写 | 可回退 |
| `user` | 单用户 | 可写 | 可回退，允许用户自行删除（下次 `apply` 重新激活） |

### 8.2 scope 路径白名单（必须实现，且越界即拒绝）

```text
image-baseline : （只读占位，不接收写入）
system         : /bin /sbin /lib /usr/bin /usr/sbin /usr/lib /etc /usr/share
shared         : /usr/share /opt /var
user           : ~ /home/<user>
```

规则：target 规范化后必须以白名单前缀开头（按路径段比较，避免 `/etcmal` 命中 `/etc`），
否则 `EINVAL` 并**在开始事务前**整体拒绝（不是「逐个文件跳过」）。

### 8.3 与当前内核状态的映射（诚实说明依赖）

截至 v0.8，内核侧文件系统的现实是：

| 载体 | 现状 | 对 cpm 的含义 |
| ---- | ---- | ---- |
| RAMFS（`fs/ramfs.rs`） | 可读可写，内存后端，单文件上限 256 KiB | 唯一「可写但非持久」的安装区候选（将来 `system` 的原型落点） |
| FAT12/16/32（`fs/fat.rs`） | **只读**：写操作一律 `EROFS` | 启动盘 `/disk` 与测试盘 `/mnt/fat` 目前只能作为 `image-baseline`（只读基线） |
| VFS（`fs/vfs.rs` + 挂载表） | 最长前缀路由，`FileSystem` trait 为唯一入口 | cpm 将来要写盘时，**只能通过 VFS**，不新增旁路 |
| 用户程序（`docs/syscall.md` 3.4） | 映像窗口 64 KiB + 4 页栈、无堆、无 `argc/argv`、`fork/execve/wait4` 仍 `ENOSYS` | cpm 生成的用户态包在 P1 只能跑**单程序**，还不能做「装了就执行」的多程序体验 |

**结论（本提案的定位）**：P0 完全在主机侧（对一个目录树做事务），**不依赖内核写盘**；
内核侧接入点在 10.2 列出，且都必须排在 v0.9 的可写文件系统与进程对象之后。

---

## 9. CLI 接口（`cpm`）

命令、参数与退出码都固定下来，因为它们会被验收脚本断言。

### 9.1 命令表

| 命令 | 作用 | 关键参数 |
| ---- | ---- | ---- |
| `cpm init` | 在 `<root>` 建立目录骨架 | `--root`、`--channel` |
| `cpm trust add <pubkey>` | 显式信任一个渠道公钥（打印指纹 + 确认） | `--yes`（非交互） |
| `cpm catalog fetch/verify` | 拉取并验签渠道清单 | `--channel`、`--allow-downgrade` |
| `cpm search <glob>` | 在已验签的 catalog 里查包（不联网也只看本地缓存） | `--json` |
| `cpm info <name>` | 显示某个包/修订的元数据与依赖 | `--json` |
| `cpm plan` | 只算差异，输出将做什么 | `--json`、`-v` |
| `cpm apply` | 按 `cpkg.toml` 让实际状态收敛到意图 | `--dry-run`、`--offline` |
| `cpm add <name>[@rev] [--scope S]` | 命令式安装（P0 的入口） | `--rev`、`--scope`、`--dry-run` |
| `cpm remove <name> [--scope S]` | 卸载（按 receipt 删文件、保留用户改过的配置） | `--purge`（连配置一起删） |
| `cpm rollback <name> [--scope S]` | 回到 `prev_rev`（纯本地） | `--offline` |
| `cpm status [<name>]` | 已激活 / 已准备 / 基线（7.4） | `--json` |
| `cpm ls [--scope S]` | 列出 scope 里的包 | `--json` |
| `cpm verify [<name>]` | 用 receipt 逐文件校验（含依赖检查） | `--json` |
| `cpm recover` | 回放未提交事务并回滚 | `--json` |
| `cpm gc` | 回收无引用 blob 与旧事务日志 | `--dry-run` |
| `cpm pack <dir>` | `PKG.toml` → `.ckc`（确定性；**已实现**为 `userland/pkgmgr/ckc.py pack`） | `--out`、`--print-rev` |
| `cpm catalog sign <file>` | 用私钥签 catalog（构建/维护侧） | `--key` |

### 9.2 退出码（必须稳定，脚本依赖它）

| 码 | 含义 |
| ---- | ---- |
| 0 | 成功 |
| 1 | 用法/参数错误（`EINVAL`、路径越界也在这一档的「参数错误」里，日志给具体 errno） |
| 2 | 校验/安全失败（`EVERIFY`、`EINTEGRITY`、未知公钥） |
| 3 | 事务失败（已回滚） |
| 4 | 环境/IO 错误（`EIO`、`ENOSPC`、`EFBIG`） |
| 5 | 并发冲突（`ELOCK`） |

### 9.3 输出约定

- 人类可读行前缀固定为 `cpm: <verb> ...`（便于 grep 与断言）；
- `--json` 输出单行 JSON（字段与 receipt/plan 的结构同名），验收脚本优先用 `--json`；
- 错误行格式：`cpm: error <ERRNO> <context> <hint>`，`ERRNO` 用第 9.2 节的符号名。

---

## 10. 与本项目的关系

### 10.1 为什么在这个项目里做（而不是通用发行版）

- Cerlesse 自己造内核，**包类别天然跨域**（内核模块/驱动/用户态工具/服务/镜像基线），
  通用包管理器都只覆盖其中一层；
- 项目的既有约定正好与提案同向：确定性产物（`mkfs.py` 的固定时间戳）、
  内容校验（ELF Loader 的段校验、`check_user_elf.py`）、
  「抽象收口」（VFS 是文件语义的唯一入口，`syscall` 层零改动就能读 FAT）——
  提案里的「签名 catalog + 内容哈希 + 单入口写入」是同一套思路的延伸；
- 语言分工沿用 `docs/lang-selection.md`：参考实现用 **Python**（主机侧工具的一致选择，纯标准库），
  将来要常驻/高性能的部分（如 store 的并发校验）再考虑 Rust。

### 10.2 内核侧未来的接入点（都不在 P0）

1. **只读基线**：镜像/`/disk` 上的文件树注册成 `image-baseline` 数据源（只读，内核已经有 FAT 只读后端）；
2. **可写安装区**：等 v0.9 的 FAT 写入或新的可写后端落地后，`system`/`user` scope 才真正可写；
3. **模块/驱动加载语义**：`category = "driver"` 的包将来需要「加载/卸载内核模块」的 syscall 或设备管理层，
   与 `docs/drivers.md` 的驱动注册框架对接（`Driver::matches/attach` 已经是「谁来管它」的雏形）；
4. **用户程序约束**：v0.7 的 ELF Loader（基址 `USER_VIRT_BASE`、按段权限、无加载时重定位、映像窗口 64 KiB）
   就是用户态包必须满足的格式约束；`cpm pack` 将来可以直接调 `tools/check_user_elf.py` 校验包内 ELF；
5. **多程序体验**：`argc/argv` 与 `fork/execve/wait4` 落地前，用户态包只能「单程序运行」，
   shell 式包管理（`cpm` 在系统里跑）不成立。

---

## 11. MVP 分期（每期都以「可验收」为出口）

### 11.1 P0 —— 主机侧参考实现（纯 Python 标准库，不启 QEMU、不碰内核）

拆成两步，第一步已经落地（2026-10-11）：

**P0a（已落地）—— 包容器与打包/校验规则**

- `userland/pkgmgr/ckc.py`：`.ckc` 容器（3.2）的 `pack` / `verify` / `info`；
- 确定性打包（两次打包逐字节相同）+ `--print-rev`；
- 打包期规则：`PKG.toml` 子集解析、target 规范化与 scope 白名单、setuid/setgid/sticky 拒绝、
  `hooks = true` 拒绝、超长路径 `ENAMETOOLONG`、重复 target 拒绝；
- 校验：头部逐字段 + payload SHA-256 + ustar 结构（排序 / 首条目 / mtime / 补零 / 仅常规文件）；
- 验收：`make test-pkg`（`userland/pkgmgr/tests/test_ckc.py`，**37 项**，含用系统 `tar` 的独立复核）。

**P0b（已落地 2026-10-12）—— 事务、状态与目录**

`userland/pkgmgr/cpm.py`（纯标准库）实现 7、8 节的事务与 6.4 的回收；权威落点：

| 交付物 | 落点 |
| ---- | ---- |
| 目录骨架（`scopes/` · `store/sha256/` · `receipts/` · `txn/` · `cache/` · `catalog/` · `trust/`） | `cpm init` |
| 回执（3.5，当前 + **上一修订** `.prev.toml`） | `cpm/{add,remove,rollback}` 原子写（临时文件 + `os.replace`） |
| 事务日志（3.6，append-only、`seq` 单调、逐条 `fsync`） | `txn/<utc>-<pid>.jsonl`；前像在 `txn/<id>/<n>` |
| 事务阶段 7.1（plan → stage → verify → activate → commit，可回滚） | `add`/`rollback`/`remove` 共用一套；**备份与 `activate` 记录都在写目标之前落日志**（WAL 顺序） |
| 内容寻址 store + 文件级去重（4.4） | `store/sha256/<hex>`（同内容只存一份） |
| 安装期重新校验（5.2 的路径白名单必须**在主机侧再跑一遍**） | `cpm` 调 `ckc.manifest_from_data` —— 「打包期检查过了」不是安装期的前提 |
| 失败注入（测试开关） | `--fail-after <阶段>`（自动回滚、退出码 3）与 `--crash-after <阶段>`（硬退出、留未提交事务给 `recover`） |
| `recover`（7.2 的「中断后可恢复」） | 扫描未提交事务并回滚；含**陈旧锁接管**（7.3） |
| 单写者锁（7.3） | `<root>/cpm.lock`（pid + 起始时间；活着 ⇒ `ELOCK`，已死 ⇒ warn + 接管）；读者不加锁 |
| `gc`（6.4） | `--dry-run` 先列出；回收无引用 blob、已提交/已回滚的日志与 `cache/` |
| 退出码 9.2 / `--json` 输出 / `cpm: <verb> ...` 行前缀 | 全命令统一 |
| 命令 | `init`/`add`/`remove`/`rollback`/`status`/`ls`/`verify`/`plan`/`apply`/`recover`/`gc`（`pack` 仍由 `ckc.py` 提供） |

**仍未做（P0b 的剩余面）**：

- `catalog sign/verify` 与 `trust add`（Ed25519；阻塞于开放问题 13.1「纯标准库没有 Ed25519」的定夺）；
- `[requires]` 的依赖求解（4.5：目前只记录、不隐式安装）与镜像基线对接（P1）；
- `--offline` 目前是「恒成立」的语义（本实现只读 store），将来接网络后它才成为真正的开关。

**验收**：`make test-pkg`（= `test-ckc` 37 项 + `test-cpm` **63 项**，主机侧、不启 QEMU）。
`test-cpm` 覆盖 12.1 里与签名/catalog 无关的条目：激活与状态、回退、离线回退、幂等、去重、
原子失败、可恢复（含掉电）、路径越界（零写入）、特权位、hook 拒绝、并发、预算；
其中「安装期重新校验」一条用了手工拼的**容器合法但 target 越出 scope** 的 `.ckc`，
证明拒绝发生在写任何文件之前。

> **与 CKM 的关系（2026-10-11）**：P0b 的交付物（事务、回执、catalog/签名、`recover`）就是
> [CKM](ckm.md) 的 **CKM-3 阶段**，实现位置从 `userland/pkgmgr/` 延伸到 CKM Service（`userland/ckmd`）；
> P1 原先说的「内核侧只保留未来的消费点」现在有具体契约：CKM 的组件状态接口（`ckm_component_state`，
> 号 259）与 `/ckm/**` 状态树。**包格式（P0a）与本节的事务语义不变**，本文仍是包格式与事务的唯一规格。

### 11.2 P1 —— 与内核交付路径对接

- 让 `tools/image-builder/build_image.py` 能把一组 `.ckc` 铺进镜像（复用 `tools/mkfs/mkfs.py` 的 FAT 写入路径），
  从而「cpm 装的包」与「镜像里的文件」用同一套修订标识；
- 引入 `image-baseline` scope 的真实数据源（`/disk` FAT 只读 → 生成基线清单）；
- 用户态包（可在 ring3 跑的程序）接入 `check_user_elf.py` 校验；
- 验收：`make test-pkg` 新增「镜像基线清单可读」场景；`make test-fat` 回归不变。

### 11.3 P2 —— 之后再考虑（当前不做）

- 增量二进制补丁（delta）；压缩归档；渠道优先级与多来源同时命中；
- 内核侧「包管理器服务」（在系统里运行 `cpm`）；P2P 分发；
- 版本区间求解与完整的依赖闭包。

---

## 12. 验收标准

### 12.1 P0 验收条目（全部可脚本断言，`make test-pkg`）

`[x]` 的是已落地并由 `make test-pkg`（`test-ckc` 37 + `test-cpm` 63 项）断言的条目；
`[ ]` 的属 catalog/签名部分（P0b 剩余面，阻塞于开放问题 13.1）。

- [x] **确定性**（已落地，`make test-pkg`）：同一目录树 `ckc.py pack` 两次，两个 `.ckc` 逐字节相同，`--print-rev` 输出相同；
- [ ] **签名**：catalog 任一字节被改 ⇒ `verify` 失败（退出码 2），且错误信息指出是签名不符；
- [ ] **未知公钥**：用未 `trust add` 的公钥签的 catalog ⇒ 拒绝（退出码 2），提示 `cpm trust add`；
- [ ] **内容绑定**：把 catalog 里的 `rev` 改成另一个 `.ckc` 的 rev ⇒ 校验失败（退出码 2），不写任何文件；
- [x] **blob 篡改**（已落地，`make test-pkg`）：`.ckc` 内一个字节被改 ⇒ 退出码 2，错误信息指出是 payload 摘要不符；
- [x] **激活与状态**（已落地，`make test-cpm`）：`add` 后 `status --json` 显示 `activated` + 正确 rev；`verify` 逐文件通过；
- [x] **回退**（已落地，`make test-cpm`）：`add` 新 rev → `rollback` → 文件内容逐字节等于上一 rev，`status` 的 rev 回到旧值；
- [x] **离线回退**（已落地，`make test-cpm`）：`rollback --offline` 成功（不触网）；
- [x] **幂等**（已落地，`make test-cpm`）：连续两次 `apply`，第二次对 scope **零写入**（`st_mtime_ns` 未变，日志含 `already active`）；
- [x] **去重**（已落地，`make test-cpm`）：两个包含相同文件的包安装后，store 中该内容只有一份（`gc --dry-run` 列出 0 项可回收）；
- [x] **原子失败**（已落地，`make test-cpm`）：`add --fail-after activate` ⇒ 退出码 3，scope 内文件恢复到事务前状态，无半成品 receipt；
- [x] **可恢复**（已落地，`make test-cpm`）：留下未 `commit` 的 `txn/`（`--crash-after` 模拟掉电）⇒ `cpm recover` 回滚并报告；
- [x] **路径越界**（已落地，`make test-cpm`）：手工拼一个「容器合法但 target 越出 scope」的 `.ckc` ⇒ `EINVAL`（退出码 1），**零文件写入**；
- [x] **特权位**（已落地，`make test-pkg`）：`mode = 0o4755` ⇒ 打包与安装期都拒绝；
- [x] **hook 拒绝**（已落地，`make test-pkg`）：`hooks = true` ⇒ 拒绝；
- [x] **并发**（已落地，`make test-cpm`）：持有锁时并发写命令 ⇒ 退出码 5；`status` 在锁被持有时仍可用；
- [x] **预算**（已落地，`make test-cpm`）：删掉 `catalog/` 后 `status` 仍成功（不读 catalog、不扫 store）；
- [ ] **catalog 上限**：> 1 MiB 的 catalog ⇒ `EFBIG`（退出码 4）。

### 12.2 与既有验收的关系

- 本提案不改变任何现有验收：`make test-all`（内核侧 12 个串口断言目标）、`make go-test`、
  `make test-orch`（11 场景）都必须保持现状；
- P0 的 `make test-pkg` 是**主机侧**测试（`userland/pkgmgr/tests/test_ckc.py`，不启 QEMU），不进 `tests/orchestrate.json`（那里是 QEMU 场景）；
- 不改 `tools/image-builder` / `tools/mkfs` 的现有行为（P1 才动，且以回归通过为前提）。

---

## 13. 风险与开放问题

### 13.1 需要定夺的点

1. **Ed25519 的依赖**：纯 Python 标准库没有 Ed25519。选项：(a) P0 允许 `cryptography` 作为**可选依赖**，
   没有时无法 `sign`（但 `verify` 可用内置实现）；(b) 引用一份小巧的纯 Python Ed25519 实现（约 200 行，需评审）。
   倾向 (a) + (b) 的组合：实现内置验证、签名可用可选依赖。**定夺前不进 P0。**
2. **tar 的确定性与 `ustar` 限制**：路径 > 100 字符需要 `ustar` 前缀或 PAX 扩展头；PAX 需要固定字段顺序才算确定性。
   P0 决定：单条路径 ≤ 100 字符，超出即 `ENAMETOOLONG`（简单、可断言）。
3. **配置文件语义**：5.3 的「配置文件白名单」需要一个显式列表才可验收；P0 先用后缀约定
   （`/etc/**` 与 `*.conf`/`*.toml`/`*.ini` 且 target 在 `/etc` 下）并在文档里写死。
4. **硬链接的风险**：硬链接让 store 与 scope 共享 inode ⇒ scope 内的意外写入会污染 store。
   P0 用「先硬链接、失败回退复制」并在 `verify` 里逐文件查 `digest`；若 P0 验收发现污染路径无法充分防护，
   则改为「始终复制」（牺牲空间换确定性）。

### 13.2 已知限制（写下来避免误读）

- 不是通用包管理器：没有求解器、没有安装脚本、没有跨架构（`architecture` 目前只记录不参与判断）；
- 事务原子性是「文件级 + 可恢复」，不是「scope 级原子」；
- 内核侧（P1 之前）无法真正写盘，因此 P0 的一切验证都在主机侧目录树上完成；
- 与现有 `make image` 路径并存但尚未打通（P1）。

---

## 14. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-12 | **阶段五：主机侧工具迁出内核仓库** —— `tools/pkgmgr/{ckc.py,cpm.py}` → `userland/pkgmgr/`，`tests/test_{ckc,cpm}.py` → `userland/pkgmgr/tests/`；宿主工具属于**用户态 workspace**（与 `libc/`、`user/` 同级），内核仓库不再持有包管理器实现；`make test-pkg` 由根 Makefile 转发到 `userland/`（见 `docs/pure-kernel.md` 8.2）；本文所有路径引用同步 |
| 2026-10-07 | 新增本文档：提出一种高效、安全、轻量且具备类似 Windows 包管理器灵活性的包管理器设计方案，含意图驱动模型、安装区与事务回退、按需元数据、安全域边界等要点 |
| 2026-10-12 | **P0b 落地（= CKM-3 事务引擎）**：新增 `userland/pkgmgr/cpm.py`（纯标准库）—— 目录骨架（`scopes`/`store/sha256`/`receipts`/`txn`/`cache`/`catalog`/`trust`）、回执（当前 + `.prev.toml`）、事务日志（`txn/<utc>-<pid>.jsonl`，append-only + `fsync`）、7.1 的 stage/verify/activate/commit + 逆序回滚、内容寻址 store 与文件级去重、`recover`（未提交事务回滚 + 陈旧锁接管）、`gc`、单写者锁、9.2 退出码与 `--json`；**安装期用 `ckc.manifest_from_data` 重跑域边界/路径白名单**（`userland/pkgmgr/ckc.py` 为此把 `load_manifest` 的校验体拆成可复用的 `manifest_from_data`，`make test-pkg` 的 37 项不变）；失败注入开关 `--fail-after <阶段>`（自动回滚、退出码 3）与 `--crash-after <阶段>`（硬退出 9、留未提交事务给 `recover`）；新增验收 `userland/pkgmgr/tests/test_cpm.py`（`make test-cpm`，**63 项**，含系统 `tar` 独立复核、逐字节回滚比对、越界包零写入）与 Makefile 目标 `test-pkg = test-ckc + test-cpm`、`test-cpm`；11.1 的 P0b 由「待做」改为落地清单，12.1 标记已落地条目；**仍未做**：catalog 签名/`trust`（开放问题 13.1）与 P1 内核侧接入 |
| 2026-10-11 | **P0a 落地：`.ckc` 包容器（后缀定名 CKC）** —— 3.2 从「`.cpk` 裸 tar 设想」改写为容器规格：64 字节头（magic `CKC\x01` / 版本 / 头长 / flags / payload_len / payload_off / 32 字节 payload_sha256 / 保留位，小端）+ 确定性 ustar payload（归档名 = 目标路径去 `/`、`PKG.toml` 第一、其余按字节序、mtime/uid/gid 全 0、仅常规文件、补零为 0、不压缩）；身份 = **payload** 哈希；新增实现 `userland/pkgmgr/ckc.py`（纯标准库：`pack`/`verify`/`info` + 退出码 0/1/2/4 + `PKG.toml` 子集解析与 scope 白名单/setuid/超长路径等打包期规则）与验收 `userland/pkgmgr/tests/test_ckc.py`（`make test-pkg`，**37 项**：确定性、头部逐字段、payload 篡改、ustar 结构规则、打包期规则、系统 `tar` 独立复核）；11.1 拆成 P0a（已落地）/P0b（事务、catalog、receipt）；同步 docs/README.md 索引、DEVELOPMENT.md 的 H-06 行、README.md、tools/README.md、project-structure.md |
| 2026-10-11 | **提案 v1（本版）**：把设想补成可实施的规格 —— 术语字段级定义（2）、七个制品格式（`PKG.toml`/`.cpk`/`catalog(+sig)`/`cpkg.toml`/receipt/journal/store，3）、内容标识与三级校验（4）、安全模型（信任根/域边界/**纯文件安装**/威胁对策表，5）、效率预算（6.3）、事务阶段与原子性边界（7）、scope 白名单与到内核现状的映射（8）、CLI 与退出码（9）、MVP 三期（11）、可脚本断言的验收条目（12.1）；同步更新索引状态（docs/README.md）与 DEVELOPMENT.md 的 H-06 行 |
