# Cerlesse Kernel Manager（CKM）设计提案

- 阶段：跨阶段 / 内核级权限与扩展管理（CKM；执行上排在 v1.0 之后，桌面路线之前）
- 最后更新：2026-10-12
- 状态：🚧 **CKM-1（内核接口）、CKM-2（可写持久化 + 覆盖层机制）与 CKM-3（事务引擎）已落地**
  —— 落地清单见 **9.1 / 9.2 / 9.3**，验收见第 10 节。
  - **CKM-1**：`kernel/src/ckm/`（`cred`/`policy`/`state`/`audit`/`api`）+ v1.0 的
    `kernel/src/security/`（凭证 uid/gid + 文件 mode + `EACCES`）+ 判定点 1/3/5/6
    + syscall `257`/`258`/`259`；
  - **CKM-2**：系统盘尾部 1 MiB 的**双槽 A/B + CRC32 状态区**（`ckm/store.rs`，掉电一致的原子写，
    开放问题 1 因此取 (b)）+ **覆盖层注册表与路径改写钩子**（`ckm/overlay.rs`，钩子在
    `fs::mount::resolve`）—— 验收是「写 → 重启 → 读回逐字节一致」与「注册/撤销改变 `open` 结果」；
  - **CKM-3**：**事务引擎**（第 7 节的事务阶段与回执）在**主机侧**落地 ——
    `userland/pkgmgr/cpm.py`（H-06 P0b）：回执、事务日志、stage/verify/activate/commit + 逆序回滚、
    `recover`（未提交事务）、`gc`、单写者锁、失败注入，验收 `make test-cpm`（63 项）；
    对应「安装可追踪、失败可回滚、重启可恢复」（第 10 节场景 3 的主机侧形态）；
  - **仍未实现**：CKM-4 `ckmd`/`ckm` CLI（把 cpm 的事务搬进内核侧的用户态服务）、
    CKM-5 覆盖层全量（`.mask`、基线保护、开机顺序）、CKM-6 恢复环境与安全模式、
    CKM-7 可加载内核模块 —— 仍按第 9 节分期推进
- 关联：[../DEVELOPMENT.md](../DEVELOPMENT.md)（H-07 需求行）· [docs/README.md](./README.md)（索引）·
  [docs/package-manager.md](./package-manager.md)（`.ckc` 包格式与事务，CKM 的包格式层）·
  [docs/architecture.md](./architecture.md)（第 5 节非目标）· [docs/pure-kernel.md](./pure-kernel.md)（内核/用户态边界）·
  [docs/filesystem.md](./filesystem.md)（VFS/RAMFS/FAT 现状）· [docs/syscall.md](./syscall.md)（syscall ABI 与用户程序约束）·
  [docs/storage.md](./storage.md)（**CSS**：卷/盘符/挂载与存储管理，CKM 的存储子系统）·
  [docs/project-structure.md](./project-structure.md)（目录归属）

> 本文档顶部保留阶段 / 最后更新 / 状态字段，与项目文档维护约定对齐。
> 约定：**文档即规格** —— 带「必须」的条款是参考实现要照着做并被验收的条款，
> 其余是理由与开放问题。不写「大概」「可能」这类无法验收的表述。

## 0. 一句话、分层与边界

CKM 不是「内核里的包管理器」，而是 **内核提供强制权限控制 + 特权用户态服务执行操作 + 可插拔扩展系统提供功能** 的三层结构：

```text
CKM Manager（管理界面）          用户态、无特权：查看状态、发起操作、看审计
   ckm（CLI，第一版唯一界面）     ↕ 经内核校验的接口
CKM Service（特权管理服务）      用户态、持权限：包安装、启停、事务、签名校验、修复回滚
   ckmd                          ↕ 受内核策略保护的受控接口
CKM Core（内核管理核心）          内核内：身份/能力校验、受控系统接口、组件状态、安全策略、审计
   kernel/src/ckm/              只实现**必须由内核保证的机制**
Cerlesse Kernel                  进程、内存、syscall、VFS、驱动、调度、隔离
```

三条边界（写下来避免误读）：

1. **内核不认识「包」**：CKM Core 不解析 `.ckc`、不解压、不下载、不比较版本、不做依赖求解、不做界面。
   它只回答两类问题：*这个主体有没有这个能力*（第 3 节）、*这个组件处于什么状态*（第 5 节）。
2. **权限判断在内核、不在管理器**：管理器自己算出来的结论只是 UX；任何受保护动作都要经过
   `ckm::policy::check`，管理器被替换/篡改也不能越权（第 3.4 节的判定点表）。
3. **第一版不做动态内核代码加载**（第 11 节）：内核模块类扩展只登记状态、不装载。

借鉴的是 KernelSU 的**分层与「系统无损」思想**（内核侧强制、元模块式可插拔覆盖、模块状态机、
安全模式恢复），不照搬 Android 的 `su`/SELinux/root 隐藏机制（见第 11 节）。

> 引用到的 KernelSU 事实（本文提到时以官方文档为准）：元模块接管常规模块的安装与挂载、
> 官方参考实现 `meta-overlayfs` 用内核 OverlayFS 做系统无损修改、**同一时刻只能装一个元模块**
> （<https://kernelsu.org/guide/metamodule.html>）；模块目录标记 `disable`/`remove`/`skip_mount`、
> `module.prop` 的 `id`/`versionCode`、模块 ZIP **不能**在 Recovery 里刷
> （<https://kernelsu.org/guide/module.html>）；卷键安全模式禁用全部模块
> （<https://kernelsu.org/guide/rescue-from-bootloop.html>）；Root Profile 由内核强制
> （<https://kernelsu.org/guide/app-profile.html>）。**Cerlesse 的差异**：不采用 Android 的
> `su`/SELinux 语义，覆盖层第一版内置（6.2），恢复使用独立镜像而不是 Recovery。

## 1. 目标、验收面与非目标

| 目标 | 落成什么条款 | 在哪一节 |
| ---- | ---- | ---- |
| **内核级权限控制** | 凭证 + 能力位 + 判定点表；拒绝 = `-EPERM` + 审计事件 + 串口证据 | 3 |
| **受控系统接口** | `kernel/src/ckm/` 的模块划分与 syscall 号（Cerlesse 扩展段）+ 定长记录布局 | 4 |
| **扩展管理** | 四类扩展、元数据字段、状态机与合法转换表、确定性加载顺序 | 5 |
| **系统无损修改** | 覆盖层（baseline + overlay + priority），停用即撤销；基线文件永不被覆盖 | 6 |
| **故障修复与回滚** | 事务阶段、回执、失败注入、`recover`；安全模式与恢复镜像 | 7 / 8 |
| **可验收** | 第 10 节把 8 条故障场景落成串口断言 / 主机侧断言 / QEMU 场景名 | 10 |

**非目标**（第一版明确不做，理由见第 11 节）：动态内核模块装载、GUI、多管理员/ACL/RBAC、
扩展携带安装脚本或 hook、内核态包管理器、Android 兼容层。

## 2. 现状审计（每条都有据）

| 项 | 现状 | 证据 | CKM 需要它吗 |
| ---- | ---- | ---- | ---- |
| 凭证 / 权限 / 能力 | ✅ **已落地（v1.0 / CKM-1）**：`security/{credential,capability,permission}.rs`（`Cred { uid, gid, caps }`、6 位能力、mode 判定）+ `ckm/{cred,policy,state,audit,api}.rs` | `make test-ckm`（22 断言）、内核自测 `ckm: selftest PASS (checks=46)` | ✅ 判定点 1/3/5/6 的落点 |
| 文件权限 | ✅ **已落地**：mode 位由 `security/permission.rs` 真正判定（→`-13`），受保护前缀额外要 `CAP_FS_SYSTEM_WRITE`（→`-1`），两者在证据与审计里**可区分** | `fs: stat /etc/motd … perms=-rw-r--r--`、`hello-bin: ckm open /etc/hello-bin.flag -> -1 (EPERM expected)` vs `-> -13 (EACCES expected)` | ✅ 判定点 1 |
| 可写持久化 | RAMFS 在内存里、**FAT12/16/32 后端只读**（写 = `EROFS`） | `docs/filesystem.md` 3.x、`fs/fat.rs` 模块文档 | ✅ 阶段 2 的前置（扩展状态/回执/store 都要落盘；**同时也是 CSS 字母表/卷标偏好的落点**，见 storage.md 6.6） |
| 分区 / 卷 / 盘符 | **完全没有**：块设备按**整盘绝对 LBA** 寻址、挂载点写死在 `fs::mount_disks()`（`dev0→/disk`、`dev1→/mnt/fat`，且只挂 FAT）、Shell 提示符里的 `C:` 是常量字符串 | `driver/block.rs`、`fs/mod.rs`、`docs/storage.md` 第 2 节 | ✅ 归 **CSS**（存储管理）；CKM 只提供它的权限判定与状态持久化，不重复实现卷表 |
| 扩展包格式 | `.ckc` 容器 + 确定性打包 + payload 哈希 + 打包期规则**已落地**（P0a，`make test-pkg` 37 项） | `docs/package-manager.md` 3.2 / 11.1 | ✅ 直接复用，**不重做**（见 2.2） |
| `.ckc` 的事务/回执/目录 | **未做**（P0b：`cpm.py`、catalog 签名、事务日志、`rollback`/`recover`） | `docs/package-manager.md` 11.1 | ✅ = CKM 阶段 3 的交付物 |
| syscall 扩展号段 | `0..=255` = Linux 语义，`>= 256` = Cerlesse 扩展；已用 `getprocs = 256` 与 CSS-1 的 `storage_volumes = 261`/`storage_rescan = 265`，**当前 `SYS_MAX = 267`**（CSS-1 后） | `abi/src/syscall.rs`（**单一事实源**，阶段三）、`docs/syscall.md` 3.8/3.11 | ✅ CKM 从 257 起编号（4.3） |
| 内核/用户态边界 | 已有 `tools/check-boundary.py`（内核不得嵌套 cargo/内嵌用户态字节/依赖用户态包）与独立 workspace `userland/` | `docs/pure-kernel.md` 第 8 节、`make check-boundary` | ✅ CKM Core 留在内核，`ckmd`/`ckm` 必须进 `userland/` |
| 审计 / 事件 | ✅ **已落地（CKM-1）**：`ckm/audit.rs` 固定容量环形缓冲（64 条，回绕后仍可读）+ 每次判定的串口证据；用户态经 `ckm_audit_read`(258) 读 48 字节记录 | `ckm: audit seq=… result=deny …`、`ckm: audit ring wrapped capacity=64`、`hello: ckm audit_read -> -1`（无 `CAP_AUDIT_READ`） | ✅ 阶段 1 |
| 引导菜单 / 恢复镜像 | boot crate 只做「装载内核并移交」；无菜单、无恢复条目、无启动失败计数 | `docs/boot.md`、`boot/` | ✅ 阶段 6（依赖阶段 2 的持久化） |

### 2.1 三件已经具备的前提

1. **机制/功能分层已有制度**（`docs/pure-kernel.md`）：内核只提供机制，因此「CKM Core 在内核、CKM Service 在用户态」不是新规矩，是既有规矩的延伸；
2. **`.ckc` 已经是「内容即身份」**：payload 的 SHA-256 就是修订号，CKM 的签名/回执/回滚都建立在它之上，不需要第二套标识；
3. **验收是可以只看串口的**：内核侧任何新机制都能用「一行串口证据 + 一个 `make` 目标」验收，
   这决定了 CKM 的接口必须**可观测**（4.5 的串口行格式是硬要求）。

### 2.2 与 H-06（包管理器提案）的关系 —— 明确不建两套

| 层 | 归属 | 说明 |
| ---- | ---- | ---- |
| 包容器与打包规则 | **H-06 P0a（已落地）** | `.ckc` 格式、确定性、打包期规则；CKM 直接复用，**不新增第二套格式** |
| 事务 / 回执 / catalog / 签名 | **CKM 阶段 3（= H-06 P0b）** | 规格继续以 `docs/package-manager.md` 第 7/8/11 节为准，本文只补「与内核状态机的接口」（第 7 节） |
| 用户态包管理器 `cpm` | **CKM Service 的一个子命令集** | 第一版 CLI 统一叫 `ckm`，`ckm pkg add/remove/rollback/verify` 对应 `cpm` 的动词；不再并列维护两个工具 |
| 内核侧「消费点」 | **CKM 阶段 1/4 的接口** | 原 package-manager 提案里「内核侧只保留未来的消费点」在本文变成具体契约（4.3 的 `ckm_component_state` + 第 5 节状态机） |

## 3. 权限模型（定稿）

### 3.1 主体与凭证

每个进程持有一份凭证（v1.0 在 `kernel/src/security/credential.rs` 落地，CKM 在其上加能力位）：

```rust
struct Cred {
    uid: u32,        // 用户
    gid: u32,        // 主组
    caps: u32,       // 能力位（3.2）
}
```

固定的主体划分（**单管理员模型**，第一版刻意很小）：

| uid | 主体 | 初始能力 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 内核上下文 / 引导 | 全部 | 内核自己发起的操作不受判定（内核不是「外部主体」） |
| 0 | `/init`（第一个用户进程） | `CAP_FS_SYSTEM_WRITE` | 建立系统视图；**不给 `CAP_CKM_ADMIN`**（最小权限） |
| 1000 | 管理员（`admin`） | `CAP_CKM_ADMIN \| CAP_FS_SYSTEM_WRITE \| CAP_AUDIT_READ \| CAP_STORAGE_ADMIN \| CAP_STORAGE_FORMAT` | 阶段 1 是**内核常量** `CKM_ADMIN_UID = 1000`；阶段 4 起由签名配置决定 |
| 1001 | 管理服务（`ckmd`） | `CAP_CKM_SERVICE \| CAP_FS_SYSTEM_WRITE \| CAP_AUDIT_READ` | 「服务」而不是「管理员」：能执行事务，但不能改策略（3.4 判定点 4） |
| ≥ 2000 | 普通用户 / 普通扩展 | 无 | 默认拒绝 |

**凭证只有一条改变路径（必须）**：`execve` 一个**由 CKM 状态表登记为组件程序**（5.3 的 `kind=userland|system-component`）的路径时，
内核把该组件声明的 uid/gid 装进新凭证；其余情况凭证**完全沿袭父进程**。

因此第一版**没有** `setuid`/`setgid` 系统调用、**没有** `su`、**没有** 给普通进程提权的路径 ——
「谁能做什么」由内核的组件表 + 常量决定，而不是由用户态自己声明。

### 3.2 能力位（第一版 6 位：CKM 4 位 + 存储 2 位）

| 能力 | 位 | 语义 | 谁持有 |
| ---- | ---- | ---- | ---- |
| `CAP_CKM_ADMIN` | `0x1` | 改 CKM 策略/授予撤销/决定进入或退出安全模式 | uid 1000 |
| `CAP_CKM_SERVICE` | `0x2` | 执行扩展事务：启停组件、写扩展状态、注册覆盖层 | uid 1001（`ckmd`） |
| `CAP_FS_SYSTEM_WRITE` | `0x4` | 写受保护前缀（`/etc`、`/bin`、`/lib`、`/init`、基线区） | uid 0 的 `/init`、1000、1001 |
| `CAP_AUDIT_READ` | `0x8` | 读审计环形缓冲（`ckm_audit_read`） | 1000、1001 |
| `CAP_STORAGE_ADMIN` | `0x10` | 存储管理的受控动作：挂载/卸载卷、分配或取消盘符、重扫设备（**不改数据、不格式化**） | 1000 |
| `CAP_STORAGE_FORMAT` | `0x20` | 创建/格式化/检查文件系统（**破坏性**，与上面的管理能力分开） | 1000 |

> 存储两位的语义、受控动作清单与 syscall 号段见 [storage.md](./storage.md) 第 6 节（CSS）。
> 拆成两位的理由：挂载/卸载是**可逆**的日常操作，格式化是**破坏性**操作 —— 合成一位就无法只给其一。
> `/init`(uid 0) 与 `ckmd`(1001) **不**持有存储能力（最小权限：建立系统视图与包事务都不需要改字母表）。

规则（必须）：

- 能力位是**位图**，判定只看「需要的位置是否都在持有位里」（`need & !caps == 0`）；
- 能力**不可继承性提升**：`fork`/`execve` 不增加能力，只有 3.1 的组件表路径能换 uid/gid，
  且新凭证的能力 = 该组件登记的能力（**不并集父进程**）；
- 没有能力位表示的细粒度（ACL、SELinux 式标签）—— 见第 12 节开放问题 1。

### 3.3 文件保护与 mode 判定（与 v1.0 的分工）

- **文件 mode**（`-rwxr-xr-x` 的读/写/执行位 + uid/gid 属主）由 `security/permission.rs`（v1.0）判定，
  失败返回 `EACCES(-13)`；这是 POSIX 意义上的检查，CKM 不重复实现；
- **受保护前缀**额外要求能力（判定点 1）：即使 mode 允许，写 `/bin/**`、`/lib/**`、`/etc/**`、`/init`
  仍需 `CAP_FS_SYSTEM_WRITE`，失败返回 `EPERM(-1)`（**刻意与 `EACCES` 分开**：一个是「不属于你」，
  一个是「你没有这个能力」，两者在审计与测试里必须可区分）；
- 受保护前缀的列表是**内核常量**（`ckm::policy::PROTECTED_PREFIXES`），第一版不允许运行时增删（阶段 4 起可由管理员改，且改动本身进审计）。

### 3.4 判定点表（阶段 1 的全部强制点）

| # | 操作 | 需要的条件 | 失败 | 落点（现有代码） |
| ---- | ---- | ---- | ---- | ---- |
| 1 | 写受保护前缀（`open(O_WRONLY\|O_CREAT)` / `write` 到已打开文件） | `CAP_FS_SYSTEM_WRITE` | `EPERM(-1)` | `syscall/file.rs` |
| 2 | 读扩展状态树（`/ckm/**`，第 5.4 节） | 无（只读） | — | `fs/` 路由 + `syscall/dir.rs` |
| 3 | 写扩展状态树 | `CAP_CKM_SERVICE`（内核自身写不受限） | `EPERM(-1)` | `fs/` 路由 |
| 4 | 改 CKM 策略 / 触发安全模式 / 授予能力（阶段 4） | `CAP_CKM_ADMIN` | `EPERM(-1)` | `syscall/ckm.rs`（新增） |
| 5 | 读审计缓冲 | `CAP_AUDIT_READ` | `EPERM(-1)` | `syscall/ckm.rs` |
| 6 | 查自己的凭证/能力与某个 op 的判定结果 | 无（只能查自己） | — | `syscall/ckm.rs` |
| 7 | `execve` 到受保护前缀的程序 | 程序文件可读（mode）+ 目标路径在 CKM 组件表里才有 uid 变化 | `EACCES(-13)` / 无提权 | `syscall/process.rs` |
| 8 | 挂载/卸载卷、分配或取消盘符、重扫设备（CSS） | `CAP_STORAGE_ADMIN` | `EPERM(-1)` | `syscall/storage.rs` |
| 9 | 创建/格式化/检查文件系统（CSS，CSS-4 起） | `CAP_STORAGE_FORMAT` | `EPERM(-1)` | `syscall/storage.rs` |
| 10 | 写存储状态树（`/storage/**`，reader 只读不受限） | `CAP_STORAGE_ADMIN` | `EPERM(-1)` | `fs/` 路由 |

**每次 deny 必须同时产生三样东西**（缺一即验收不过）：

1. 返回值 `-EPERM`（或该行指定的 errno）；
2. 一条审计事件（4.4 的定长记录）；
3. 一行串口证据（4.5 的格式）。

## 4. 内核接口（定稿）

### 4.1 模块划分（`kernel/src/ckm/`，新增，阶段 1）

| 文件 | 职责 | 明确不做 |
| ---- | ---- | ---- |
| `mod.rs` | 常量（`CKM_ABI_VERSION`、`CKM_ADMIN_UID`、受保护前缀）、`init()`（幂等，建立空状态表与审计环） | 不解析任何包/清单 |
| `cred.rs` | 凭证与能力位定义、`current()`、`cap_ok(need)`、组件表驱动的 uid/gid 切换 | 不做文件 mode 判定（那是 `security/permission.rs`） |
| `policy.rs` | 判定点表（3.4）的实现：`check(op, cred, arg) -> Result<(), Errno>`，拒绝理由枚举 | 不读磁盘、不做事务 |
| `state.rs` | 组件/扩展状态表（5.4）+ 合法转换校验 + 确定性枚举顺序 | 不持久化（阶段 3 由服务写回执；内核重启后由服务重新注入） |
| `audit.rs` | 固定容量环形缓冲（**必须** ≥ 64 条）+ 串口行输出 + `ckm_audit_read` 的数据源 | 不做持久化审计（阶段 6 之后再谈） |
| `api.rs` | 给 `syscall/` 用的唯一入口（把上面四个模块包成几个 `pub` 函数） | 不暴露内部类型给别的内核模块 |
| `overlay.rs`（CKM-2） | 覆盖层注册表（`register`/`unregister`/`lookup`/`rewrite`）+ 最长前缀/优先级/同优先级冲突拒绝；判定点 7/8 | 不碰文件系统（建覆盖区、读写文件都由 `fs/` 发起）、不做 `.mask` 与基线保护（CKM-5） |
| `store.rs`（CKM-2） | 可写持久化状态区：双槽 A/B + CRC32 的键值存储（`get`/`put`）+ 跨重启自测 | 不做事务/回执（CKM-3）、不认识包、不做审计落盘（CKM-6） |

依赖关系（必须）：`syscall/* → ckm::api → {policy, cred, state, audit}`；
`fs/` 依赖 `ckm::policy` 与前缀常量，**并**（CKM-2 起）依赖 `ckm::overlay` 做路径改写；
**`ckm/` 不反向依赖 `syscall/` 与 `fs/`**（与 `security/` 一样是叶子被依赖者）——
因此覆盖层的**文件动作**（建立 `/ckm/overlay/**`、读写文件、验证视图变化）都在 `fs/` 侧发起
（`fs::overlay_selftest`），`ckm/overlay.rs` 本身只维护注册表。
`ckm/store.rs` 额外依赖 `driver/{serial, block}`（状态区是**扇区级**原始区，它不在任何文件系统里）。
现有 `security/` 继续承担凭证的 UID/GID 与文件 mode 判定（v1.0），两者分工写在 4.1 与 3.3。

> **现状（2026-10-12）**：上表七个文件已全部落地（含 9.1/9.2 的落地清单），
> 依赖方向（`syscall/* → ckm::api`、`fs/ → ckm/`、`ckm/` 不反向依赖 `syscall/`/`fs/`）
> 由 `make check-boundary` 与模块内 `use` 共同保证。

### 4.2 CKM ABI 版本

- 内核导出常量 `CKM_ABI_VERSION: u32 = 1`（第一版）；
- 扩展清单必须声明 `abi.kernel`（整数）；**判定规则第一阶段取相等**（`ext.abi_kernel == CKM_ABI_VERSION`），
  不等即拒绝安装并留证据 `ckm: verify id=<id> result=deny reason=abi`（不等号语义留给后续版本，见开放问题 5）；
- 内核自己的 syscall ABI 版本已有 `SYS_MAX` 表达；CKM 的 ABI 版本与它独立（前者是调用号边界，后者是管理接口/记录布局的边界）。

### 4.3 CKM 系统调用（Cerlesse 扩展段，接在 `getprocs = 256` 之后）

| 号 | 名字 | 签名（5 参数 ABI） | 权限 | 行为 |
| ---- | ---- | ---- | ---- | ---- |
| 257 | `ckm_policy_query` | `(op, arg, out_ptr, out_len, 0)` | 无（**只能查当前进程**） | 返回该 op 在本进程凭证下的判定（`0` = 允许，`-EPERM` = 拒绝）+ 定长记录（4.4） |
| 258 | `ckm_audit_read` | `(buf_ptr, buf_len, since_seq, out_count_ptr, 0)` | `CAP_AUDIT_READ` | 把审计环里 `seq > since_seq` 的记录拷进用户缓冲，返回**条数** |
| 259 | `ckm_component_state` | `(buf_ptr, buf_len, out_count_ptr, 0, 0)` | 无（只读） | 按 5.4 的**确定性顺序**导出组件状态记录 |
| 260 | `ckm_policy_set` | **阶段 4 才定义** | `CAP_CKM_ADMIN` | 第一版**不实现**（阶段 1 的授予面为零） |

- `SYS_MAX` 由 `257` 改为 **`260`**（即 `0..=259` 可用），`syscall_table.rs` 的号段说明同步；
  其中 **261–266 已被 CSS（存储管理）占用**（`storage_volumes`/`storage_mount`/`storage_unmount`/
  `storage_assign_letter`/`storage_rescan`/`storage_format`，见 [storage.md](./storage.md) 6.3），
  因此 `SYS_MAX` 的**最终值**是 **267**：CKM 自身号段仍是 257–260，两套接口共享同一个扩展段；
- 260 在阶段 1–3 落在公共 ENOSYS 桩上（保持「号已定义但未实现」可区分，机制已在 v1.0 保留）；
- 号一旦发布不再回收利用（与 Linux 的号段纪律一致）。

### 4.4 定长记录（内核侧唯一权威定义）

**审计事件（48 字节）** —— `ckm_audit_read` 的单元，也是「拒绝必须可核查」的载体：

| 偏移 | 大小 | 字段 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 8 | `seq` | 单调递增（从 1 起），环形缓冲满后回绕并置 `wrapped` 标志 |
| 8 | 8 | `pid` | 发起进程 |
| 16 | 4 | `uid` | 凭证 uid |
| 20 | 4 | `gid` | 凭证 gid |
| 24 | 2 | `op` | 事件类型（4.6 枚举） |
| 26 | 1 | `result` | `0` = allow，`1` = deny |
| 27 | 1 | `reserved` | 0 |
| 28 | 4 | `need` | deny 时需要的 capability 位，否则 0 |
| 32 | 8 | `detail` | op 相关（被拒路径哈希 / 组件槽位 / 页数 / errno） |
| 40 | 8 | `reserved` | 0 |

**组件状态（64 字节）** —— `ckm_component_state` 的单元：

| 偏移 | 大小 | 字段 | 说明 |
| ---- | ---- | ---- | ---- |
| 0 | 16 | `id` | 扩展标识（`[A-Za-z0-9._-]{1,15}`，NUL 填充） |
| 16 | 2 | `kind` | `1` userland / `2` system-component / `3` system-override / `4` kernel-module（阶段 7 前恒不出现） |
| 18 | 1 | `state` | `0` absent / `1` staged / `2` installed / `3` active / `4` disabled / `5` failed / `6` removed-pending |
| 19 | 1 | `reserved` | 0 |
| 20 | 4 | `version_code` | 整数版本（比较规则同 H-06） |
| 24 | 4 | `abi_kernel` | 声明兼容的 CKM ABI 版本 |
| 28 | 4 | `priority` | 覆盖层优先级（大者优先；非覆盖层恒 0） |
| 32 | 24 | `rev` | 内容修订（payload SHA-256 的前 24 字节，与 `.ckc` 同一身份） |
| 56 | 8 | `reserved` | 0 |

两份布局的偏移与大小**必须**在 `docs/api.md` 与 `libc` 里镜像一次（与 `DIRENT_SIZE`/`PROCINFO_SIZE` 同样的做法：
内核是唯一权威定义，用户态用 `ckm_audit_*`/`ckm_component_*` 解析器，不自己算偏移）。

### 4.5 串口证据格式（必须）

```text
ckm: init ABI_VERSION=1 admin_uid=1000 protected=4 audit_cap=64
ckm: policy op=fs_write pid=3 uid=2000 result=deny need=0x4 detail=0x2
ckm: policy op=policy_query pid=1 uid=1000 result=allow need=0x0 detail=0x0
ckm: component id=hello kind=1 state=3 rev=0x1f3c… priority=0
ckm: audit seq=1 op=fs_write result=deny pid=3 uid=2000
ckm: selftest PASS (checks=N)
```

- 判定行**必须**在每次 `check` 后打印（allow 与 deny 都打），因此「内核真的判断过」是可断言的串口事实；
- `op` 用**短名**而不是数字（数字在 4.6，改动时不会让日志变成天书）：`fs_write`、`state_write`、
  `policy_set`、`audit_read`、`policy_query`、`exec_component`、`overlay_register`、`overlay_conflict`、
  `storage_mount`、`storage_unmount`、`storage_letter`、`storage_format`、`storage_rescan`；
- 组件状态行只在**状态转换**时打印（不是每次查询），因此 `make test-ckm` 可以断言「谁在什么时候动了什么」；
- `need` / `detail` / `rev` 都用 `print_hex` 打印，它**自带 `0x` 前缀**（因此是 `need=0x4`，不是 `0x0x4`）。

### 4.6 事件/操作枚举（内核常量，用户态镜像）

```text
1 fs_write         2 state_write      3 policy_set       4 audit_read
5 policy_query     6 exec_component   7 overlay_register 8 overlay_conflict
9 safe_mode_enter  10 txn_commit      11 txn_rollback
12 storage_mount   13 storage_unmount 14 storage_letter   15 storage_format
16 storage_rescan
```

**7/8 自 CKM-2 起有落点**（此前只有枚举，没有调用方）：

- `7 overlay_register`：覆盖表的**每一次变动**（注册与撤销都算）走 `policy::check`，
  因此能力不足时是「三件套」拒绝（`-EPERM` + 审计 + 证据行）；
- `8 overlay_conflict`：**语义拒绝** —— 凭证够、但同优先级且前缀重叠。它走 `policy::reject`，
  证据行的 `need=0x0` 是刻意的：**补任何能力都改变不了这个结果**（`docs/ckm.md` 6.3 第 2 条）。
  同一条规则也用于「非法 id/前缀/kind」与「表已满」。

> 「能力不足」与「请求不成立」分开记的两个理由：断言可区分（`need=0x0` vs 非 0），
> 以及审计里 `result=deny` 的原因不会退化成一句「权限不够」。

## 5. 扩展模型（定稿）

### 5.1 四类扩展与第一版范围

| 类型 | `kind` | 示例 | 管理方式 | 第一版 |
| ---- | ---- | ---- | ---- | ---- |
| 用户态扩展 | 1 | Shell、命令、应用、`libc` | 安装/卸载/修复/回滚（文件事务） | ✅ 阶段 3–4 |
| 系统组件扩展 | 2 | 服务、系统工具、配置组件 | 版本与依赖管理、权限审核、健康检查 | ✅ 阶段 3–4 |
| 系统修改模块 | 3 | 替换系统文件、增加资源、覆盖配置 | 内核覆盖层（第 6 节）的启停 | ✅ 阶段 5 |
| 内核模块 | 4 | 可加载驱动、内核功能 | 严格签名 + ABI + 受控装载 | ❌ 阶段 7，第一版只登记状态 |

判断标准（必须）：一个扩展属于哪一类，由**它的清单里的 `kind`** 决定，不由文件后缀或安装路径推断；
`kind = 4` 在阶段 7 之前**一律拒绝安装**，错误码 `EINVAL` + 证据 `ckm: verify id=<id> result=deny reason=kind`。

### 5.2 元数据（复用 `.ckc` 与 `PKG.toml` 子集）

扩展清单就是 H-06 的 `PKG.toml`，CKM 只**新增一个段**（不新增格式）：

```toml
[package]
name    = "hello-extra"
version = "1.2.0"
category = "userland"        # 沿用 H-06：system/driver/userland/service/image-baseline

[extension]                  # CKM 新增，字段名固定
id          = "hello-extra"  # 必须：与目录名/回执键一致，[A-Za-z0-9._-]{1,15}
kind        = 1              # 必须：1..=4（5.1）
version_code = 10200         # 必须：整数，唯一用于版本比较
priority    = 0              # kind=3 必须 ≥1；其余必须为 0
uid         = 2000           # 可选：组件程序的运行 uid（缺省 = 2000）
[extension.abi]
kernel      = 1              # 必须：CKM ABI 版本（4.2）

[requires]
kernel      = ">=1.0"        # 沿用 H-06：只记录、只校验
```

必须拒绝（沿用 H-06 的安全模型，不放松）：安装脚本/hook、setuid/setgid/sticky 位、scope 路径越界、
超长路径、重复 target、未声明 `abi.kernel`、`kind=4`（阶段 7 前）。

### 5.3 生命周期状态机（内核侧状态 = 5.4 记录的 `state`）

```text
        install(校验通过)            activate(依赖满足+无冲突)
absent ───────────────▶ staged ──▶ installed ─────────────▶ active
   ▲                     │              │                    │  │
   │                 校验失败         启动失败             disable│  │enable
   │                     ▼              ▼                    ▼  │
   │                   failed ◀─────────┘                 disabled
   │                     │                                    │
   └──── removed-pending ◀────────────────────────────────────┘
        （下次启动清理；用户数据不受影响）
```

合法转换表（必须，其他转换一律 `EINVAL` 并留证据）：

| 从 | 到 | 谁有权 | 守卫条件 |
| ---- | ---- | ---- | ---- |
| absent | staged | `ckmd`（`CAP_CKM_SERVICE`） | 签名/哈希/ABI/kind/路径白名单全部通过 |
| staged | installed | `ckmd` | 事务提交成功（回执已落盘） |
| staged | failed | 内核 | 任一校验失败，原因进审计 `detail` |
| installed | active | `ckmd` | 依赖满足、覆盖层无冲突（第 6.3 节） |
| active | disabled | `ckmd` 或 `CAP_CKM_ADMIN` | 停用即撤销覆盖（6.4） |
| disabled | active | `ckmd` | 重新激活必须重新做冲突检查 |
| active / disabled | failed | 内核 | 健康检查/启动失败计数超阈值（阶段 6） |
| any | removed-pending | `ckmd` | 标记删除；下次启动清理文件，**保留用户改动过的配置**（沿用 H-06 receipt 语义） |
| removed-pending | absent | 内核（启动清理阶段） | 文件删除完成 + 回执更新 |

### 5.4 内核里的状态表（阶段 1 的形态）

- 固定容量：**32 个槽位**（与 `driver` 注册表、进程表同样的「固定容量 + 明确拒绝」风格；满了返回 `ENOSPC`）；
- 枚举顺序（`ckm_component_state` 必须按它输出）：先 `kind` 升序，再 `priority` 降序，最后 `id` 字节序升序
  —— **确定性**是可断言的前提（与 `fs: ls` 条目顺序、`getprocs` 槽位顺序同一约定）；
- 状态树暴露在 VFS 上：`/ckm/state`（只读文件，定长记录拼接）与 `/ckm/audit`（只读，4.4 记录），
  写它们需要 `CAP_CKM_SERVICE`；`ckmd` 通过 `ckm_component_state` 读，而不是解析串口文本；
- **内核不持久化**状态表：重启后由 `ckmd` 从回执/状态目录重建并重新登记（阶段 3 起）。这样内核不需要认识
  「包」，也不需要持久化格式（第 12 节开放问题 2）。

### 5.5 启动加载顺序（阶段 5 起，必须确定性）

```text
内核 ckm::init()（空表）
  → /init 建立基线视图
  → ckmd 读状态目录 → 按 5.4 顺序逐个 ckm_component_state 登记（阶段 4 起改为内核读状态树）
  → 按登记顺序激活：kind 1/2（文件层）→ kind 3（覆盖层，按 priority 降序）
  → 覆盖层全部注册完成后才放行用户登录（阶段 5 的开关写在 ckmd，内核只提供 overlay_register）
  → 任一扩展激活失败：计数 +1；连续 3 次启动失败 → 安全模式（第 8 节）
```

## 6. 系统无损修改：覆盖层（定稿）

### 6.1 模型

```text
基线区（只读，可信）          覆盖层（可插拔，按 priority）
 /system/**   ← 原始视图  ←  /ckm/overlay/<id>/**  （替代文件）
                             /ckm/overlay/<id>.mask （显式隐藏基线文件）
```

- **基线永不被写**：扩展不得直接修改 `/system/**`（判定点 1 之外，另有一条硬规则：
  基线与覆盖都是「只读挂载 + 内核覆盖表」，写基线一律 `EROFS`，写覆盖目录需要 `CAP_CKM_SERVICE`）；
- **覆盖层是注册表项**（不是「真的挂了文件系统」）：`ckm::api::overlay_register(id, kind, priority, prefix)`，
  内核在 VFS 查表时先查覆盖表（前缀最长匹配 → 优先级最高 → 同优先级冲突即拒绝），再落到基线；
- 覆盖层只对 `kind=3` 的扩展开放；`kind=1/2` 走普通文件事务（装到 `/app/<id>/**` 与 `/bin` 白名单）。

### 6.2 为什么第一版由内核内置实现、而不是「元模块」

KernelSU 把「挂载/覆盖」做成可插拔的**元模块**（官方文档：元模块接管常规模块的安装与挂载，
`meta-overlayfs` 是参考实现，同一时刻只能装一个）。Cerlesse 第一版**内置**覆盖层，理由是可验收性：

1. 内置才能保证「停用扩展即撤销覆盖」是**内核**性质（一条 `overlay_unregister` 就能断言），
   若覆盖由用户态服务实现，则「服务崩了覆盖还在不在」变成不可控；
2. `kind=3` 的冲突检测需要内核已知全部覆盖（否则要靠服务自律）；
3. **元模块化是明确的后续项**：阶段 5 之后可以把「覆盖表的内容来源」做成可插拔
   （内核只保留 `overlay_register/unregister/查表`，注入者可以是 `ckmd`、也可以是另一个扩展服务），
   这与 KernelSU 的分层目的（内核稳定、实现可替换）一致，但**第一版不做**（第 11 节）。

### 6.3 冲突检测（必须）

同一基线路径被两个 `kind=3` 扩展覆盖时：

1. `priority` 不同 → 高者生效，低者保留但**不参与查表**，并记 `ckm: policy op=overlay_register ... result=allow detail=<被遮蔽者>`；
2. `priority` 相同 → **拒绝后注册者**（`ckm: policy op=overlay_conflict ... result=deny`），
   注册失败 → 该扩展进 `failed`，不进入 `active`；
3. 禁止「隐式覆盖」：没有显式 `[extension].priority` 的 `kind=3` 扩展一律拒绝安装（5.2 已写成必须字段）。

### 6.4 停用即撤销（必须可断言）

`active → disabled` 时，内核必须做到三件事，且三件都进审计：

1. 从覆盖表移除该扩展的全部条目（前缀 + mask）；
2. 对受影响的路径**刷新已打开的文件视图**：重新打开得到基线内容（第一版不要求对已打开的 fd 立即生效，
   但必须要求「新 `open` 一定拿到基线」——这条写进验收，避免「看起来撤销了、其实还有旧映射」）；
3. 打印 `ckm: component id=<id> state=4 rev=<rev>`。

## 7. 事务、回执与回滚（阶段 3 = H-06 P0b）

规格继续以 `docs/package-manager.md` 第 7/8/11 节为准，本文只写**与 CKM 状态机的接口**（必须）：

| 事务阶段（H-06） | CKM 侧的状态 | 谁写 |
| ---- | ---- | ---- |
| `prepare`（校验签名/哈希/ABI/kind/路径） | `staged` | `ckmd` 调 `ckm_component_state` 只会**读**；状态由服务镜像进内核（阶段 4） |
| `backup`（前像入 store） | 不变 | `ckmd` |
| `activate`（落文件） | `installed → active` | `ckmd`；内核在 `overlay_register` 时做冲突检查 |
| `commit`（回执落盘） | 保持 | `ckmd` |
| 任何阶段失败 | `failed` | 内核记录原因（4.6 的 `txn_rollback`） |
| `recover`（启动时发现未提交事务） | 回滚到事务前状态并记审计 | `ckmd` + 内核审计 |

必须保持不变的三条既有结论（避免被 CKM 悄悄推翻）：

1. 修订 = 内容哈希，没有第二套版本标识；
2. **纯文件安装、不执行安装脚本**（因此「扩展损坏」不会在安装期执行任意代码）；
3. `rollback` 必须能**离线**完成（基线/缓存足够），这条在第 10 节变成验收条目 7。

## 8. 救援与安全模式（阶段 6）

### 8.1 触发方式（必须都支持，且可脚本断言）

| 触发 | 机制 | 断言 |
| ---- | ---- | ---- |
| 连续 3 次启动未到「CKM 服务就绪」 | 引导器在持久化区写失败计数（第 2 节：需要阶段 2 的可写区） | `ckm: safe mode enter (boot_failures=3)` |
| 引导菜单显式选择 | boot crate 增加菜单项 `Cerlesse (recovery)`，把 `--recovery` 放进 UEFI `LoadOptions` / 内核命令行 | 恢复镜像启动后串口出现 `ckm: recovery mode (services=off)` |
| 管理员在管理界面请求 | `ckm enter-safe-mode`（需要 `CAP_CKM_ADMIN`） | `ckm: safe mode enter (requested_by=1000)` |

### 8.2 安全模式里必须成立的四件事

1. **不加载任何 `kind=3` 覆盖**（覆盖表为空）→ 系统回到基线视图；
2. **不启动 `ckmd`**（因此 CKM 服务自己损坏时也能进来）；
3. 只提供**最小工具集**：恢复用的 shell、`ckm recover`、审计查看、文件校验；全部**不在**扩展可覆盖的前缀里；
4. 进入时把**全部扩展**置为 `disabled`（不是 `failed`），并在审计里记一条 `safe_mode_enter`；
   退出安全模式必须由管理员显式逐项重新激活（不做「自动全开」）。

### 8.3 恢复镜像（独立、可信）

- 恢复环境是**独立启动项 + 独立镜像**，不复用正常启动链的 `/init` 与 `ckmd`（第 10 节验收条目 5 的前提）；
- 它必须能做四件事：查看审计、校验 `store` 里每个 blob 的哈希、重装/回滚指定的扩展、修复 `ckmd` 本体；
- 构建方式（引导菜单 vs. 独立 ESP 条目）见第 12 节开放问题 6。

## 9. 分期计划（每期一个可验收出口）

| 期 | 交付物 | 依赖 | 验收出口 |
| ---- | ---- | ---- | ---- |
| **CKM-1 内核接口** ✅ **已落地（2026-10-12）** | `kernel/src/ckm/`（`cred`/`policy`/`state`/`audit`/`api`）+ 3.4 判定点 1/3/5/6 + `ckm_policy_query`/`ckm_audit_read`/`ckm_component_state` + 串口证据 | **v1.0 的 `security/`（uid/gid + mode）已先落地** | `make test-ckm`（22 断言）：缺能力写受保护前缀被拒（`-EPERM`）+ 审计可读（含回绕）+ 组件状态枚举确定性；uid 2000 的 `EPERM`/`EACCES` 对照由 `make test-proc` 断言 |
| **CKM-2 可写持久化** ✅ **已落地（2026-10-12）** | 系统盘尾部 1 MiB 的**双槽 A/B + CRC32 状态区**（`kernel/src/ckm/store.rs`，开放问题 1 取 (b)）+ VFS 覆盖层的**机制**（`kernel/src/ckm/overlay.rs`：注册/撤销/最长前缀/优先级/同优先级冲突拒绝，钩子在 `fs::mount::resolve`） | CKM-1 | `make test-ckm`（覆盖机制：注册/撤销改变 `open` 结果 + 冲突拒绝）+ `make test-ckm-store`（**跨重启**：写 → 重启 → 读回逐字节一致，且第二次启动只读不写）—— 落地清单见 **9.2** |
| **CKM-3 事务引擎** ✅ **已落地（2026-10-12，主机侧）** | H-06 P0b（`userland/pkgmgr/cpm.py`）+ 回执 + 事务日志 + `recover` + 失败注入（**签名/catalog 除外**，阻塞于开放问题 4） | CKM-2（状态区；内核侧迁移属 CKM-4） | `make test-pkg`（= `test-ckc` 37 + `test-cpm` **63**）覆盖 H-06 12.1 的剩余条目（离线回滚、原子失败、可恢复、幂等、去重、并发、预算）—— 落地清单见 **9.3** |
| **CKM-4 特权服务与 CLI** | `userland/ckmd`（持 `CAP_CKM_SERVICE`）+ `userland/ckm`（CLI）+ 内核状态树 `/ckm/**` 写路径 | CKM-3 | QEMU 场景 `ckm`：装/启/停/卸/回滚全链路 + 拒绝证据（判定点 1/3/4/5） |
| **CKM-5 系统修改模块** | `kind=3` 覆盖层全量（优先级、冲突、开机顺序、基线保护） | CKM-4 | QEMU 场景 `ckm-overlay`：停用后 `open` 回基线（6.4 的三条） |
| **CKM-6 恢复环境** | 恢复镜像 + 引导菜单/命令行 + 启动失败计数 + 安全模式四件事 | CKM-5 | `make test-recovery`：连续 3 次失败进安全模式；从恢复环境重装 `ckmd` |
| **CKM-7 可加载内核模块** | 签名 + ABI + 依赖 + 卸载安全检查（**本文档不展开**） | CKM-6 | 需要单独提案（`docs/ckm-kmod.md`），先于实现评审 |

### 9.1 CKM-1 落地清单（2026-10-12）

规格在上文（第 3–4 节），这里只写**代码落在哪、怎么验收**。

**内核**（`kernel/src/`）

| 位置 | 内容 |
| ---- | ---- |
| `security/credential.rs` | `Cred { uid, gid, caps }`；固定主体常量（`ADMIN_UID = 1000`、管理服务 `1001`、普通用户 `2000`）；`CAP_*`（6 位） |
| `security/capability.rs` | 能力位图判定（`need & !caps == 0`）与串口短名 |
| `security/permission.rs` | 文件 mode 判定（`mode::FILE_DEFAULT = 0o644` / `DIR_DEFAULT = 0o755`，属主/组/其他三段）→ `EACCES` |
| `ckm/mod.rs` | `CKM_ABI_VERSION = 1`、`AUDIT_CAPACITY = 64`、`STATE_SLOTS = 32`、幂等 `init()`（空表 + 引导组件表） |
| `ckm/cred.rs` | `current()`（内核上下文 = uid 0 + 全能力）、`cap_ok(need)`、按组件表换 uid/gid/能力（3.1 的唯一改变路径） |
| `ckm/policy.rs` | 判定点 1/3/5/6/7/8/10 的实现：`check(op, arg)` → `allow` / `-EPERM`；每次判定都打 `ckm: policy …` 并写审计 |
| `ckm/state.rs` | 32 槽组件表 + 合法转换表校验（非法转换 = `ckm: component … transition=deny`）+ **确定性枚举**（`kind` ↑、`priority` ↓、`id` 字节序 ↑） |
| `ckm/audit.rs` | 64 条环形缓冲（`seq` 单调、回绕置 `wrapped`）+ 48 字节定长记录 |
| `ckm/api.rs` | `syscall/` 的唯一入口（`policy_query` / `audit_read` / `component_state`） |
| `syscall/ckm.rs` | 257 `ckm_policy_query` / 258 `ckm_audit_read` / 259 `ckm_component_state`（记录的权威布局在 `ckm/{audit,state}.rs`，用户态只镜像不定义） |
| `syscall/{file,process,storage}.rs`、`fs/` | 判定点的**落点**（受保护前缀的 `open`/`write`、`execve` 换凭证、存储管理动作） |
| `process/process.rs` | 内核栈顶 16 字节对齐（`kstack_top`）—— 与 CKM 同批修掉的 UB：新增静态量会把 `KSTACKS` 顶端挤到奇地址，而 syscall 参数帧是 `&[u64; 5]` |

**用户态**（`userland/`）

| 位置 | 内容 |
| ---- | ---- |
| `libc/src/syscall.rs` | 三个 syscall 包装 + 48/64 字节记录解析器（`ckm_record_*` / `component_*` / `audit_*`）+ `CAP_*` / `CKM_OP_*` / `EPERM` / `EACCES` 常量 |
| `user/hello`（`/init`） | `ckm_selftest()`：uid 0 + `CAP_FS_SYSTEM_WRITE` 的**允许/拒绝两面**（能写 `/etc/**`；`state_write` / `audit_read` / `storage_rescan` 均 `-1`），并按确定性顺序列出组件表 |
| `user/hello-bin`（`/bin/hello`） | `ckm_denials()`：uid 2000 + **零能力**的默认拒绝证据 —— 受保护前缀 `-1`（`EPERM`）与 mode 不许可 `-13`（`EACCES`）**可区分** |

**验收**（可复现的命令 → 断言）

| 命令 | 断言 |
| ---- | ---- |
| `make test-ckm` | 22 条断言 + 2 条正则：`ckm: selftest PASS (checks=46)`、`/init` 的允许/拒绝、组件枚举、`ckm: audit ring wrapped capacity=64`、`verify … reason=kind/id/target` |
| `make test-proc` | uid 2000 的 `-1` / `-13` 对照 + 拒绝一定伴随审计事件（`ckm: audit seq=… uid=2000 result=deny`） |
| `make test-user` | `/init` 的 CKM 证据行（`test-ckm` 断言的子集，防回归） |
| `make test-orch`（场景 `ckm`） | 并行编排下的二次复核（同一批子串断言） |
| 内核自测（46 项） | 能力矩阵逐项对照（`policy::allowed` vs `check`）、非法状态转换、审计回绕、记录长度 |

> 判定点 4（`ckm_policy_set` = 260）**仍未实现**：阶段 1 的授予面为零，号已定义并落在 ENOSYS 桩上（4.3），属于 CKM-4。
> 仍然没有的（截至 CKM-2）：状态树 `/ckm/**` 的**写路径**（5.4，CKM-4）与状态表的持久化
> —— 注意区分：CKM-2 交付的是**持久化介质与覆盖层机制**（9.2），而「把状态表写回去」是 ckmd 的事（CKM-3/4）。

### 9.2 CKM-2 落地清单（2026-10-12）

两件事：**可写持久化的介质**（开放问题 1 的定夺）与**覆盖层的机制**。两者都不涉及「包」与事务。

**介质：系统盘尾部 1 MiB 的双槽 A/B 状态区**

| 项 | 内容 |
| ---- | ---- |
| 定夺 | 开放问题 1 取 **(b) 极小日志式区**；(a)（给 FAT 加写支持）留给 CSS-4 —— 理由见 `kernel/src/ckm/store.rs` 模块头与第 12 节 |
| 落点 | 系统盘（块设备 0 = 启动盘）**尾部 1 MiB**，在 FAT **之外**：`tools/image-builder/build_image.py` 先在 63 MiB 上 `mformat`，再把保留区追加回去 —— 设备仍报 64 MiB、`table=none partitions=1`，但文件系统看不到尾部 1 MiB |
| 布局 | 偏移 0 槽 A（4 KiB）/ 偏移 4096 槽 B（4 KiB）/ 其余保留；槽头 = `magic`("KMS1") + `version` + `seq` + `payload_len` + `crc32(payload)`，payload 是 KV 表（`u16` 计数 + 逐条 `(klen, vlen, key, value)`） |
| 原子性 | **没有活动指针**（指针本身就是一次非原子写）：写者整槽写**非活动槽**，CRC 通过才切内存视图；读者取「有效（magic/version/长度/CRC 全对）且 `seq` 最大」的槽 —— 写到一半掉电时那个槽的 CRC 对不上，上一个槽完好 |
| 代码 | `kernel/src/ckm/store.rs`（`init`/`enabled`/`get`/`put`/`key_count`/`selftest`）；`kernel_main` 在 `storage::init` 之后调用 —— 状态区要块设备，但**不需要**文件系统 |
| 容量 | 16 个键；键 ≤ 32 B / 值 ≤ 255 B（「固定容量 + 明确拒绝」）；`put` 失败**不改**内存视图 |
| 证据 | `ckm: store area dev=… lba=… slots=2 slot_bytes=4096 state=fresh`（首次）或 `state=loaded …`、`ckm: store put key=… len=N seq=N slot=A` / `slot=B`、`ckm: store verify key=… value=… ok=1`、`ckm: store selftest PASS (checks=N)` |
| 命名空间 | CSS 的字母表/卷标偏好预留 `storage/letters`、`storage/labels`（[storage.md](./storage.md) 6.6）—— CSS 只定义键与语义，不定义持久化格式 |

**覆盖层：注册表 + 路径改写钩子**

| 项 | 内容 |
| ---- | ---- |
| 代码 | `kernel/src/ckm/overlay.rs`（`register`/`unregister`/`lookup`/`rewrite`/`reset`/`entry_at`）；钩子在 `kernel/src/fs/mount.rs` 的 `MountTable::resolve` 与 `route`：**规范化之后、查表之前**改写路径（空表是快路径，不分配） |
| 模型 | 覆盖区 = `/ckm/overlay/<id>/**`，注册表项 = `(id, kind=3, priority, prefix)`；改写规则 = `OVERLAY_ROOT/<id>` + 前缀之后的剩余部分（前缀是目录 → 镜像目录内容；前缀是**文件** → 镜像文件本身） |
| 查表 | **最长前缀 → 优先级最高**；同优先级冲突在**注册时**就被拒，因此表里不可能出现「同前缀同优先级」两条 |
| 冲突 | 前缀重叠 + 优先级不同 → 允许，低者保留但不参与查表（证据行给出 `shadowed=<id>`）；重叠 + 优先级相同 → `op=overlay_conflict result=deny`；`kind ≠ 3` / 非法 id / 前缀覆盖 `/ckm`（自指）/ 重复 id → 拒绝 |
| 撤销 | `unregister(id)` 移除该 id 的**全部**条目；因为钩子**每次**查表，「新 `open` 一定拿到基线」不需要刷新已打开的文件（6.4 第 2 条） |
| 落点 | 内核自测 `fs::overlay_selftest()`（在 `fs/` 侧：建覆盖区、注册、读回比对、撤销、断言回基线）—— 用户进程启动时表为空 |

**验收**（可复现的命令 → 断言）

| 命令 | 断言 |
| ---- | ---- |
| `make test-ckm` | 40 条断言 + 3 条正则：CKM-1 的原有断言 + 覆盖层「注册改视图（`changed=1 bytes=26`）/ 冲突拒绝 / 优先级遮蔽 / 逐条撤销回基线（`bytes=23`）/ 负例」+ `ckm: store verify … ok=1` + `ckm: store selftest PASS (checks=6)` |
| `make test-ckm-store` | **跨重启**：清零一份独占镜像副本 → 启动 #1（`state=fresh`，槽 A/B 各写一次）→ 启动 #2 同一份镜像（`state=loaded slot=A seq=3 keys=3`，读回逐字节相同，且 `state=loaded` 的下一行就是读回证据 = **没有重写**） |
| `make test-orch`（场景 `ckm` / `ckm-store`） | 并行编排下的二次复核（`ckm-store` 用自己的镜像副本，因此可与其他场景并行） |
| 内核自测 | `ckm: store selftest PASS (checks=6)`、`fs: overlay selftest PASS`（`make run` 与所有启动路径都会跑） |

> **本清单不包含（避免把机制期当成全量期）**：`.mask` 显式隐藏、基线保护（写基线一律 `EROFS`）、
> 开机加载顺序与「停用扩展 → `state=4` 转换」属于 **CKM-5**；回执/事务/回滚属于 **CKM-3**；
> `ckmd`/`ckm` CLI 与状态树写路径属于 **CKM-4**。
>
> 另外，覆盖层目前仍由**内核内置**注册（6.2）：`overlay_*` 还没有 syscall，CKM-2 的自测就是
> 第一版「注入者」（`fs::overlay_selftest`），`ckmd` 的注册路径在 CKM-4 接上。

### 9.3 CKM-3 落地清单（2026-10-12，主机侧）

CKM-3 的交付物是 **H-06 P0b 的事务引擎**（第 7 节的事务阶段、回执与 `recover`）。
它落在**主机侧**（`userland/pkgmgr/cpm.py`，纯标准库），与 CSS/CKM 的内核侧改动无关 ——
这是刻意的：事务引擎的语义（阶段、回执、回滚、恢复）先在一个可以逐字节比对、可以不启 QEMU
就能跑几十种故障场景的地方定稿，再由 CKM-4 把它作为 `ckmd` 的库搬进用户态服务。

**落点**

| 位置 | 内容 |
| ---- | ---- |
| `userland/pkgmgr/cpm.py` | 事务引擎与 CLI：`init`/`add`/`remove`/`rollback`/`status`/`ls`/`verify`/`plan`/`apply`/`recover`/`gc` |
| `userland/pkgmgr/ckc.py` | 新增 `manifest_from_data`（把 `load_manifest` 的校验体拆出来）—— **安装期重跑域边界/路径白名单** 复用打包期规则，不留两套判定 |
| `userland/pkgmgr/tests/test_cpm.py` | 主机侧验收（63 项，不启 QEMU）；`Makefile` 的 `test-pkg = test-ckc + test-cpm` |
| `receipts/<scope>/<name>.toml`（+ `.prev.toml`） | 回执（3.5）：当前修订 + 上一修订；`rollback` 的落点在本地 store（离线成立） |
| `txn/<utc>-<pid>.jsonl` | 事务日志（3.6）：append-only、`seq` 单调、逐条 `fsync`；前像在 `txn/<id>/<n>` |
| `store/sha256/<hex>` | 内容寻址 blob：同内容只存一份（文件级去重） |
| `<root>/cpm.lock` | 单写者锁（7.3）：pid + 起始时间；活着 ⇒ `ELOCK`（退出码 5），已死 ⇒ warn + 接管 |

**与第 7 节接口的对应**

| 第 7 节的事务阶段 | cpm 里的实现 | 故障后果（已验证） |
| ---- | ---- | ---- |
| `plan`（校验签名/哈希/ABI/kind/路径） | 读 `.ckc` + `ckc.verify_bytes` + **`ckc.manifest_from_data`** + scope/ABI 检查；**不写任何文件** | 越界 ⇒ `EINVAL`（1）+ 零写入；ABI 不符 ⇒ `EINVAL` |
| `stage`（blob 进 store） | `store/sha256/<hex>`（先算摘要后落盘） | 注入失败 ⇒ 回滚，无生效文件 |
| `verify` | blob 存在且长度一致（依赖/空间留给后续版本） | 同上 |
| `activate`（落文件 + 前像） | 先记 `backup` + `activate`（**WAL 顺序**），再从 store 复制到 scope | `--crash-after activate` ⇒ 硬退出 9，留下半成品 + 未提交事务 |
| `commit`（逐文件重读 + 回执） | 重读比对摘要（`EINTEGRITY` ⇒ 回滚）+ 回执轮转（当前 → `.prev`） | `--fail-after activate` ⇒ 回滚到事务前（退出码 3） |
| `rollback`（逆序恢复） | 按日志逆序：前像优先，无前像的新增文件删除；回执回滚 | 文件逐字节回到事务前 |
| `recover`（中断后可恢复） | 扫描未提交事务并回滚；陈旧锁接管 | 第 10 节场景 3 的主机侧形态 |

**四件必须保持不变的事（沿用 H-06）**：修订 = 内容哈希（回执的 `rev` 就是 `.ckc` 的 payload 哈希）；
纯文件安装、不执行安装脚本；`rollback --offline` 纯本地成立；未激活 ≠ 生效（`store/` 是材料、`scopes/` + 回执才是生效）。

**仍未做（CKM-3 的剩余面，不在本期）**：catalog 签名与 `trust` 公钥管理（H-06 本版**不**建第二套签名机制 ——
`ckm.md` 与 `package-manager.md` 必须用同一个决定，开放问题 4 未定夺前不进实现）；
`[requires]` 依赖求解；把事务引擎接进内核侧（`ckmd`/`ckm`，属 **CKM-4**）。

顺序上的三条硬约束（必须）：

1. **CKM-1 之前先做 v1.0 的 `security/`**（uid/gid + mode + `EACCES`）：否则能力判定没有主体可挂靠；
2. **CKM-3 之前必须有 CKM-2**：没有可写持久化就没有回执与可恢复事务，事务引擎只能是玩具；
3. **CKM-2 的可写区同时是 CSS 的字母表/卷标偏好落点**（[storage.md](./storage.md) 6.6）：
   选介质时要把「存储侧也要用」算进去，否则 CSS-3 只能退回确定规则（字母不保留用户自定义）。

## 10. 第一版验收标准（把 8 条故障场景落成断言）

`make test-ckm`（新增 QEMU 场景 + 主机侧断言）必须覆盖下表；每条的「断言」都是可脚本化的，
不依赖人工观察。

| # | 测试 | 合格标准 | 断言方式（必须实现其中至少一条） |
| ---- | ---- | ---- | ---- |
| 1 | 未授权进程请求管理权限 | 被内核拒绝 | 串口 `ckm: policy op=state_write pid=N uid=2000 result=deny need=0x2` + 用户程序拿到 `-1`（`make test-ckm`） |
| 2 | 扩展包签名或哈希错误 | 在修改系统前拒绝 | `ckm: verify id=<id> result=deny reason=digest`（或 `reason=sig`）+ **零文件写入**（主机侧 store 快照对比） |
| 3 | 安装过程中发生错误 | 事务回滚，无半成品状态 | `--fail-after activate` ⇒ 退出码 3 + scope 逐文件回到事务前 + 无半成品 receipt（H-06 12.1） |
| 4 | 系统文件被扩展覆盖 | 停用后恢复原始视图 | 停用后新 `open` 得到基线内容 + `ckm: component id=<id> state=4` + 覆盖表空（`make test-ckm`） |
| 5 | 扩展管理器程序损坏 | 独立恢复环境能修复它 | 删/改坏 `ckmd` 后从恢复镜像启动 → `ckm recover --repair-service` 成功 → 正常启动回到就绪（`make test-recovery`） |
| 6 | 上次更新后无法启动 | 能进入安全模式并回滚 | 注入「扩展激活后 `/init` 失败」⇒ 第 3 次启动 `ckm: safe mode enter (boot_failures=3)` + 全部扩展 `state=4` + 回滚后正常启动 |
| 7 | 网络不可用 | 能用本地基线或缓存恢复 | `ckm pkg rollback --offline` 成功（无任何网络设备时也可通过；与 H-06 12.1 同一条） |
| 8 | 用户数据存在 | 修复系统组件不删用户数据 | 安装/回滚/修复后 `/app/<id>` 之外的用户数据（配置、用户文件）逐字节不变 |

补充（必须同批验收，避免「功能对、机制没生效」）：

- 判定点 1 的两类失败**可区分**：mode 不允许 → `-13`（`EACCES`），能力不足 → `-1`（`EPERM`）；
- 审计环回绕后仍可读（`wrapped` 标志 + 至少保留 64 条中的最新部分）；
- 组件状态枚举顺序在两次查询间**稳定**（相同输入 → 逐字节相同的输出）；
- 不允许任何扩展通过 `kind=3` 之外的路径写基线（`EROFS` 证据）。

**CKM-1 的覆盖情况（2026-10-12）**：第 1 条（未授权进程请求管理权限被拒）与上面「补充」四条已落地并被断言
（`make test-ckm` / `make test-proc`，清单见 9.1）；第 2–8 条依赖 CKM-3/5/6 的交付物（事务、覆盖层、恢复环境），
**在那些期实现之前不宣称通过** —— 文档里的验收条目不因部分实现而变形。

**CKM-2 的覆盖情况（2026-10-12）**：第 4 条（系统文件被扩展覆盖 → 停用后恢复原始视图）的**机制面**已落地
并被断言（注册改变 `open` 结果、`unregister` 把新 `open` 带回基线，清单见 9.2）。
但第 4 条要求的另两样还差：`ckm: component id=<id> state=4`（组件状态机，CKM-5）与
「覆盖表为空 + 基线保护（写基线 `EROFS`）」。因此**第 4 条仍不计入通过**；第 2/3/5/6/7/8 条依赖 CKM-3/5/6。
另外「可写持久化」这条前置已就绪（9.2 的状态区），CKM-3 的回执与事务可以直接建在它上面。

**CKM-3 的覆盖情况（2026-10-12）**：第 3 条（安装过程中发生错误 → 事务回滚，无半成品状态）已落地
并被断言 —— `make test-cpm` 用 `--fail-after activate` 断言「退出码 3 + 每个文件逐字节回到事务前 +
无半成品回执 + 日志里有 `rollback` 记录」，并用 `--crash-after activate` + `cpm recover`
断言「掉电留下的未提交事务被识别并回滚」。第 7 条（网络不可用 → 本地基线/缓存恢复）的**本地部分**
（`rollback --offline` 纯 store 命中、不触网）也已断言。
但第 2 条（签名/哈希错误）只完成了**哈希与 ABI 面**（`EVERIFY`/`EINVAL` + 零文件写入）；
**签名面仍未做**（catalog 签名阻塞于开放问题 4），因此第 2 条**仍不计入通过**。
第 5/6 条（恢复环境、安全模式）依赖 CKM-6；第 8 条（不删用户数据）的安装/回滚面已随
`remove` 的配置保留得到主机侧验证（`make test-cpm`），内核侧面随 CKM-4 再验。

## 11. 非目标（第一版明确不做）

1. **不照搬 Android root 机制**：没有 `su` 二进制、没有 SELinux、不 patch 内核以隐藏 root、
   不追求与 Magisk/KernelSU 模块的二进制兼容；借鉴的是**分层与系统无损思想**（第 0 节）。
2. **不做动态内核代码加载**（`kind=4` 只登记、拒不安装）：内核模块出错会直接破坏内存，
   不能像用户态那样沙箱；装载需要独立提案（CKM-7）。
3. **不做 GUI**：第一版只有 `ckm` CLI（CLI 优先是刻意的：可脚本断言，GUI 的验收成本远高于收益）。
4. **不做多管理员/ACL/RBAC**：单管理员 + 固定能力位（3.1/3.2）。
5. **不做扩展携带脚本/hook**（沿用 H-06：`hooks = true` 一律拒绝）。
6. **不把包管理器放进内核**：内核不认识包、不解析清单、不做依赖求解（第 0 节边界 1）。
7. **不做 OTA/远程仓库协议**：渠道与签名沿用 H-06，第一版只要求「本地基线/缓存可用」（第 10 节 7）。

## 12. 风险与开放问题（需要定夺，未定不进实现）

1. **可写持久化介质**（CKM-2 的前置）—— ✅ **已定夺（2026-10-12，随 CKM-2 落地）**：取 **(b) 极小的日志式区**
   （双槽 A/B + CRC32，落在系统盘尾部 1 MiB，见 9.2）。四条理由：
   1. **要的语义不同**：CKM 要「**原子状态区**」（一次写入要么整体可见、要么完全不可见，掉电后仍能读回
      上一次成功的值），而 FAT 的写语义（FAT 表 + 目录项 + 簇链三处更新）不天然提供这一点；
   2. (b) **不被别的期阻塞**：它不碰任何文件系统，因此可以在 CSS-4 的写入语义评审之前先落地；
   3. CSS 的字母表/卷标偏好只需要**键值持久化**（`storage/letters`、`storage/labels`），不需要 FAT 写；
   4. (a) 仍是 **CSS-4 的第一件事**（对用户价值更大），本定夺不取消它。
   注意与 CSS 的区别（不要混为一谈）：本条要的是「**原子状态区**」（双槽 + CRC + 掉电一致），
   CSS-4 要的是「**文件语义写入**」（簇分配、目录项、可写卷）—— 两者可能落在同一个 FAT 卷上，
   但验收标准不同：一个断回执可恢复，一个断写-重启-读回逐字节一致（[storage.md](./storage.md) 第 8 节）。
2. **覆盖表的内容来源**：第一版由内核内置（6.2）；元模块化（KernelSU 式可插拔注入）作为 CKM-5 之后的演进，
   接口先按 6.1 固定，避免以后改 ABI。
3. **首个管理员身份**：CKM-1 用内核常量 `CKM_ADMIN_UID = 1000`；CKM-4 起改为签名配置。
   「如何在不引入信任悖论的前提下换管理员」必须靠恢复环境（第 8 节）解决，不能由普通路径自举。
4. **签名算法**：与 `docs/package-manager.md` 13.1 同一个开放问题（纯标准库没有 Ed25519）。
   两处必须用同一个决定，不能一个用 Ed25519、一个用别的。
5. **`abi.kernel` 的兼容规则**：第一阶段取 `==`；何时放宽到区间（`>=`）需要一次 ABI 评审，
   且必须说明「旧扩展在新内核上语义不变」的依据。
6. **恢复环境的构建方式**：引导菜单项 + 同一 ESP（改动小）vs. 独立 ESP 条目（隔离强）。
   取决于 `boot/` 与 `tools/image-builder` 的改动成本，CKM-6 开工前定。
7. **审计的持久化**：第一版审计只在内存（重启即丢）；是否需要在 CKM-6 落盘（掉电后仍可核查「谁拒了谁」）
   取决于第 1 条的可写区是否足够便宜。

## 13. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-12 | **阶段五（构建与测试按责任拆分）**：CKM-3 的主机侧实现（事务引擎 + `.ckc` 容器 + 两份套件）不再住在内核仓库 —— `tools/pkgmgr/` → `userland/pkgmgr/`、`tests/test_{ckc,cpm}.py` → `userland/pkgmgr/tests/`，内核侧 `make test-pkg` 改为**跨仓库转发**，用户态侧用 `make -C userland test-pkg` 直接跑；内核 `make test-all` 收敛为纯内核（18 目标），新增 `test-all-with-userland`/`test-orch-userland`，编排场景用 `tags` 分组（`testorch -tag`/`-exclude-tag`）。**CKM 的内核侧机制不变**（能力位/审计/状态区），只是交付物落在 `userland/`；详见 [pure-kernel.md](./pure-kernel.md) 8.3 与 [../userland/pkgmgr/README.md](../userland/pkgmgr/README.md) |
| 2026-10-12 | **CKM-3 落地（事务引擎，主机侧 = H-06 P0b）**：新增 `userland/pkgmgr/cpm.py`（回执 + 事务日志 + stage/verify/activate/commit + 逆序回滚 + `recover` + `gc` + 单写者锁 + 失败注入；**备份与 `activate` 记录都在写目标之前落日志**，因此「写到一半掉电」可恢复）；`userland/pkgmgr/ckc.py` 拆出 `manifest_from_data`，使**安装期重跑域边界/路径白名单**（`make test-pkg` 的 37 项不变）；新增 `userland/pkgmgr/tests/test_cpm.py`（`make test-cpm`，**63 项**，含系统 `tar` 独立复核、逐字节回滚比对、手工拼的越界包零写入）；`make test-pkg` 改为 `test-ckc + test-cpm`；新增 **9.3 落地清单**，第 9 节 CKM-3 行改 ✅，第 10 节补 CKM-3 覆盖情况（第 3 条通过；第 2 条仅哈希/ABI 面，签名面阻塞于开放问题 4）；同步 [package-manager.md](./package-manager.md) 11.1/12.1/14 |
| 2026-10-12 | **CSS-3 盘符接入（CKM 判定点 8 + CKM-2 状态区）落地**：`storage_assign_letter`(264)/`storage_mount`(262)/`storage_unmount`(263) 实现后，存储受控动作真的走 CKM —— 拒绝三件套（`-EPERM` + 审计 + 串口 `ckm: policy op=storage_mount … result=deny need=0x10`）与允许面都有实测（`make test-ckm` 的 uid 2000 拒绝；`make test-programs` 的 Shell uid 1000 `volumes demo` 成功）；**CKM-2 状态区的第一个存储侧消费者**出现：盘符表把自定义映射写进键 `storage/letters`（值是可读文本 `C=fs:1a2b3c4d;Z=raw:4`），跨重启读回由 `make test-ckm-store` 的两次启动给出 —— 即 3.4 硬约束第 3 条「可写区同时是字母表落点」已兑现；判定点 8 不再只是设计（见 [storage.md](./storage.md) 6.6/13） |
| 2026-10-12 | **阶段三（ABI 单一事实源）**：CKM 的 ABI 常量不再由内核与 libc 各写一份 —— `CKM_ABI_VERSION`、审计/组件记录尺寸、6 个能力位、事件编号 1–16、受保护前缀全部移入 `abi::ckm`（`abi/src/ckm.rs`）；内核 `ckm::{mod,audit,state,policy}.rs` 与 `security/capability.rs` 改为再导出/推导（`Op::code`/`from_code` 的 32 处数字改 `abi::ckm::OP_*`，能力集另加 3 条编译期断言），`libc` 侧 19 个常量改为值绑定；`make abi-check` 固化。行为零变化：`make test-ckm`（44+3 / 8+2 / 6+3 断言）全 PASS |
| 2026-10-12 | **CKM-2 落地**：新增 `kernel/src/ckm/store.rs`（系统盘尾部 1 MiB 的**双槽 A/B + CRC32** 状态区：`init`/`get`/`put` + 跨重启自测；`kernel_main` 在 `storage::init` 之后调用）与 `kernel/src/ckm/overlay.rs`（覆盖层注册表 `register`/`unregister`/`lookup`/`rewrite` + 最长前缀/优先级/同优先级冲突拒绝；判定点 7/8 的落点）；`fs::mount::resolve`/`route` 在查表之前接入路径改写（空表快路径），`fs::overlay_selftest` 在 `fs/` 侧建立 `/ckm/overlay/**` 并断言「注册/撤销改变 `open` 结果」；`policy::reject` 新增**语义拒绝**（`need=0x0`，与能力不足可区分）；`tools/image-builder/build_image.py` 改为「先在 63 MiB 上 `mformat`、再追加 1 MiB 保留区」（设备仍是 64 MiB，FAT 看不到状态区）；新增 `make test-ckm-store` 与 `tests/test_ckm_store.py`（两阶段启动：写 → 重启 → 读回）、`test_boot.py --persist`、编排器场景 `ckm-store`（`ckm` 场景同步扩充）；`test-ckm` 扩到 40 断言 + 持久化两启动，`test-fat`/`test-programs`/`test-storage` 的 FAT 几何与根目录条目数断言随之更新（启动盘 FAT 63 MiB、`/ckm` 使根目录 5 → 6 项）；**开放问题 1 定夺为 (b)**；新增 9.2 落地清单，第 10 节补 CKM-2 覆盖情况 |
| 2026-10-12 | **CKM-1 落地**：`kernel/src/ckm/`（`cred`/`policy`/`state`/`audit`/`api`）+ v1.0 `security/`（凭证 uid/gid + mode + `EACCES`）+ syscall `257`/`258`/`259` + 判定点 1/3/5/6 的强制落点与串口证据；`libc` 加三个包装与 48/64 字节记录解析器，`/init` 与 `/bin/hello` 分别提供允许/拒绝证据；新增 `make test-ckm`（22 断言）与编排器场景 `ckm`，`test-user`/`test-proc` 同步扩展；同批修掉一处**内核栈对齐 UB**（新增静态量把 `KSTACKS` 顶端挤到奇地址，syscall 参数帧 `&[u64; 5]` 因此不对齐 —— `process::kstack_top` 现在显式对齐到 16 字节）。头部状态改为「CKM-1 已落地」，第 9 节 CKM-1 行改 ✅ 并新增 **9.1 落地清单**，第 10 节补覆盖情况，4.5 的 `rev` 示例补 `0x` 前缀 |
| 2026-10-11 | 追加**存储侧接口**（配合新建的 [storage.md](./storage.md)，CSS 提案）：能力位由 4 位扩到 **6 位**（`CAP_STORAGE_ADMIN 0x10` / `CAP_STORAGE_FORMAT 0x20`，uid 1000 持有，`/init` 与 `ckmd` 不持有）、判定点表追加 **8–10**（挂载/卸载/字母/重扫、格式化、写 `/storage/**`）、事件枚举追加 **12–16**（`storage_mount`/`storage_unmount`/`storage_letter`/`storage_format`/`storage_rescan`，串口短名同步）、号段说明注明 **CSS 占 261–266 故 `SYS_MAX` 最终值为 267**（CKM 自身号段仍为 257–260）、现状审计补「分区/卷/盘符」一行（归属 CSS）、CKM-2 硬约束补第 3 条（可写区同时是字母表落点）与开放问题 1 的边界澄清（原子状态区 vs 文件语义写入） |
| 2026-10-11 | 创建本文档：定稿权限模型（单管理员 + 4 个能力位 + 7 个判定点，凭证只由 CKM 组件表在 `execve` 时改变）、内核接口（`kernel/src/ckm/` 五模块 + `CKM_ABI_VERSION=1` + 扩展号段 257–259 + 两份定长记录 + 串口证据格式）、扩展生命周期（四类扩展、状态机与合法转换表、确定性枚举/加载顺序）；覆盖层与 KernelSU 元模块的对应关系、事务与 H-06 P0b 的接口、安全模式与恢复环境、CKM-1…CKM-7 分期、8 条故障验收落成断言、非目标与 7 个开放问题；同步 docs/README.md 索引、DEVELOPMENT.md（H-07 行 + 5.12 + 修订记录）、roadmap（第十一阶段）、architecture 第 5 节、project-structure（规划目录）、README（文档入口） |
