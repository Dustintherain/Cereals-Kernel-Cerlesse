#!/usr/bin/env python3
"""CPM —— Cerlesse 包管理器事务引擎（H-06 **P0b** = **CKM-3**）。

权威规格：[`docs/package-manager.md`](../../docs/package-manager.md) 第 3.5/3.6/4/5/7/8/9 节，
与 [`docs/ckm.md`](../../docs/ckm.md) 第 7 节（事务阶段 ↔ CKM 状态机的接口）。

这一层只做 P0 的五件事（`docs/package-manager.md` 6.2）：解析意图、定位修订、
校验内容、**编排事务**（stage → verify → activate → commit，可回滚）、记录可观测结果。
它**不认识内核**：`.ckc` 的容器与打包期规则由 [`ckc`](./ckc.py) 负责（P0a），
那边是「包长什么样」，这里是「包怎么安全地落进 scope 并可以撤销」。

## 目录骨架（`cpm init` 建立）

```text
<root>/
  cpm.lock                     单写者锁（内容 = pid + 起始时间）
  scopes/<scope>/<target>      生效文件（target 是绝对路径，去掉开头的 /）
  store/sha256/<hex>           内容寻址 blob（同一份内容只存一份 → 文件级去重）
  receipts/<scope>/<name>.toml        当前回执（3.5）
  receipts/<scope>/<name>.prev.toml   **上一修订**的回执（rollback 的落点；纯本地）
  txn/<utc>-<pid>.jsonl        事务日志（3.6，append-only，seq 单调）
  txn/<utc>-<pid>/<n>          事务的前像（被覆盖文件的原内容）
  cache/ catalog/ trust/       主机侧缓存与信任根骨架（MVP 未用）
```

## 事务（`docs/package-manager.md` 7）

```text
plan → stage → verify → activate → commit
  ↘ 任意阶段失败（或被注入失败）→ rollback（恢复到事务开始前）
```

- **plan**：读 `.ckc`、校验容器、**用 `ckc.manifest_from_data` 重跑域边界与白名单**、
  算意图摘要；此时**不写任何文件**（越界 ⇒ `EINVAL`，零写入）；
- **stage**：把 payload 里的每个文件按内容哈希写进 `store/sha256/<hex>`（先校验后落盘）；
- **verify**：落盘前的最后检查（并发/空间/依赖留给后续版本）；
- **activate**：把 store 里的 blob 落到 scope；**覆盖已有文件前先把前像备份进 `txn/<id>/`**
  —— 备份与 `activate` 记录都在写目标文件**之前**落进日志（WAL 顺序），
  因此「写到一半掉电」也能被 `recover` 还原；
- **commit**：逐文件**重读**并与期望摘要比对（4.2 第 3 层；不符 ⇒ `EINTEGRITY` + 回滚），
  再原子地写入回执（当前 → 上一修订）；
- **rollback**：按日志**逆序**恢复前像 / 删除新增文件、撤销回执轮转，并追加一条 `rollback` 记录。

## 掉电与恢复

`recover` 扫描 `txn/*.jsonl`：**没有 `commit` 也没有 `rollback` 记录的事务是「未完成事务」**，
逐个回滚并报告。这就是「重启后不留下无法解释的半完成状态」（`docs/ckm.md` 第 10 节场景 3）。

## 退出码（`docs/package-manager.md` 9.2，脚本依赖它）

`0` 成功 · `1` 用法/参数（含路径越界、scope 不符）· `2` 校验/安全（`EVERIFY`/`EINTEGRITY`）·
`3` 事务失败（**已回滚**）· `4` 环境/IO（`EIO`/`ENOSPC`/`EFBIG`）· `5` 并发冲突（`ELOCK`）。

## 测试开关（只用于注入失败，不靠真断电）

- `--fail-after <stage|verify|activate|commit>`：在该阶段**完成之后**注入失败 → 自动回滚 → 退出码 3
  （`docs/package-manager.md` 12.1「原子失败」）；
- `--crash-after <阶段>`：在该阶段之后**硬退出**（`os._exit(9)`，不执行回滚、不释放锁），
  把未提交事务留在日志里 —— 用来测 `cpm recover`（12.1「可恢复」）。

## 已知未做（本阶段边界，避免把机制期当成全量期）

catalog 签名与 `trust`（开放问题 13.1，未定夺不进 P0）、`gc` 的引用计数式回收、
依赖求解（4.5 的 `[requires]` 只记录不求解）、镜像基线对接（P1）。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ckc  # noqa: E402  （复用 P0a 的容器解析与打包期规则，不重建一套）

# ---- 退出码（docs/package-manager.md 9.2） ----

EXIT_OK = 0
EXIT_USAGE = 1
EXIT_VERIFY = 2
EXIT_TXN = 3
EXIT_IO = 4
EXIT_LOCK = 5

#: 默认渠道（3.4 的 `[settings].channel`）
DEFAULT_CHANNEL = "local-dev"
#: 本机内核 ABI（对应 `kernel/src/ckm/mod.rs` 的 `CKM_ABI_VERSION`）
DEFAULT_KERNEL_ABI = 1
#: 配置文件后缀（5.3 / 13.1：卸载默认保留，`--purge` 才删）
CONFIG_SUFFIXES = (".conf", ".toml", ".ini")


class CpmError(Exception):
    """带稳定 errno 名与退出码的错误（CLI 只打印，不抛栈）。"""

    def __init__(self, errno: str, message: str, hint: str = "", exit_code: int = EXIT_USAGE):
        super().__init__(message)
        self.errno = errno
        self.message = message
        self.hint = hint
        self.exit_code = exit_code


def _fail(errno: str, message: str, hint: str = "", exit_code: int = EXIT_USAGE) -> CpmError:
    return CpmError(errno, message, hint, exit_code)


def _verify_fail(message: str, hint: str = "") -> CpmError:
    return _fail("EINTEGRITY", message, hint, EXIT_VERIFY)


# ---------------------------------------------------------------- 目录骨架


class Layout:
    """`<root>` 下的固定布局；所有路径都从这里取，避免各处拼字符串。"""

    def __init__(self, root):
        self.root = Path(root)

    @property
    def lock_path(self) -> Path:
        return self.root / "cpm.lock"

    @property
    def scopes(self) -> Path:
        return self.root / "scopes"

    @property
    def store(self) -> Path:
        return self.root / "store" / "sha256"

    @property
    def receipts(self) -> Path:
        return self.root / "receipts"

    @property
    def txn(self) -> Path:
        return self.root / "txn"

    @property
    def cache(self) -> Path:
        return self.root / "cache"

    @property
    def manifest(self) -> Path:
        return self.root / "cpkg.toml"

    def scope_root(self, scope: str) -> Path:
        return self.scopes / scope

    def receipt(self, scope: str, name: str) -> Path:
        return self.receipts / scope / f"{name}.toml"

    def receipt_prev(self, scope: str, name: str) -> Path:
        return self.receipts / scope / f"{name}.prev.toml"

    def target_path(self, scope: str, target: str) -> Path:
        """绝对安装目标 → scope 沙箱里的真实路径（去掉开头的 `/`）。"""
        return self.scope_root(scope) / target.lstrip("/")

    def blob(self, digest: str) -> Path:
        return self.store / digest

    def ensure(self) -> None:
        if not self.root.is_dir():
            raise _fail(
                "ENOENT",
                f"root is not initialized: {self.root}",
                "先运行 `cpm init --root <root>`",
                EXIT_IO,
            )

    def create(self) -> None:
        for rel in ("scopes", "store/sha256", "receipts", "txn", "cache", "catalog", "trust"):
            (self.root / rel).mkdir(parents=True, exist_ok=True)


# ---------------------------------------------------------------- 单写者锁（7.3）


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


class Lock:
    """`<root>/cpm.lock`（`O_CREAT|O_EXCL`）。

    锁的**死活**由文件里的 pid 判断，而不是「文件存在与否」：写命令拿到已存在的锁时，
    若持有者进程还活着 ⇒ `ELOCK`（退出码 5）；若已不在 ⇒ 打一条 warn 并**接管**
    （接管而不是「自动删」：文件仍由新的写者改写，不会出现无人负责的删除）。
    `status`/`ls`/`verify` 等读者不加锁，因此随时可运行。
    """

    def __init__(self, layout: Layout):
        self.layout = layout
        self.acquired = False

    def __enter__(self) -> "Lock":
        self.acquire()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.release()

    def acquire(self) -> None:
        path = self.layout.lock_path
        payload = f"pid={os.getpid()}\nstarted={_utc_now()}\n"
        try:
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            holder = _read_lock_pid(path)
            if _pid_alive(holder):
                raise _fail(
                    "ELOCK",
                    f"lock is held by pid={holder}",
                    "另一个写者正在运行（读者不受影响）",
                    EXIT_LOCK,
                )
            print(f"cpm: warn stale lock pid={holder} (process not running) -> reclaimed")
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass
            fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(payload)
        self.acquired = True

    def release(self) -> None:
        if not self.acquired:
            return
        try:
            os.unlink(self.layout.lock_path)
        except FileNotFoundError:
            pass
        self.acquired = False


def _read_lock_pid(path: Path) -> int:
    try:
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.startswith("pid="):
                return int(line[4:])
    except (OSError, ValueError):
        pass
    return 0


# ---------------------------------------------------------------- 小工具


def _utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _digest_of(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _rev_of(data: bytes) -> str:
    return f"sha256:{_digest_of(data)}"


def _bare(rev: str) -> str:
    """`sha256:<hex>`（或裸 hex）→ 32 字节摘要的十六进制。"""
    return rev.split(":", 1)[1] if ":" in rev else rev


def _norm_rev(rev: str) -> str:
    """任意写法（`sha256:hex` / 裸 hex / `hex@...`）→ 规范的 `sha256:<hex>`。"""
    return f"sha256:{_bare(rev)}"


# ---------------------------------------------------------------- 回执（3.5）


class ReceiptFile:
    __slots__ = ("target", "digest", "size", "mode")

    def __init__(self, target: str, digest: str, size: int, mode: int):
        self.target = target
        self.digest = digest
        self.size = size
        self.mode = mode

    def to_dict(self) -> dict:
        return {"target": self.target, "digest": self.digest, "size": self.size, "mode": self.mode}


class Receipt:
    __slots__ = ("name", "scope", "rev", "prev_rev", "channel", "activated_at", "files")

    def __init__(self, name, scope, rev, prev_rev, channel, activated_at, files):
        self.name = name
        self.scope = scope
        self.rev = rev
        self.prev_rev = prev_rev
        self.channel = channel
        self.activated_at = activated_at
        self.files = files

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "scope": self.scope,
            "rev": self.rev,
            "prev_rev": self.prev_rev,
            "channel": self.channel,
            "activated_at": self.activated_at,
            "files": [f.to_dict() for f in self.files],
        }


def _q(text: str) -> str:
    """TOML 子集里的字符串字面量（解析器不做转义，因此拒绝会破坏字面量的字符）。"""
    if any(ch in text for ch in ('"', "\\", "\n")):
        raise _fail("EINVAL", f"value {text!r} cannot be written into the TOML subset")
    return f'"{text}"'


def receipt_to_toml(receipt: Receipt) -> str:
    lines = [
        "[receipt]",
        f"name = {_q(receipt.name)}",
        f"scope = {_q(receipt.scope)}",
        f"rev = {_q(receipt.rev)}",
        f"prev_rev = {_q(receipt.prev_rev)}",
        f"channel = {_q(receipt.channel)}",
        f"activated_at = {_q(receipt.activated_at)}",
        "",
    ]
    for entry in receipt.files:
        lines += [
            "[[files]]",
            f"target = {_q(entry.target)}",
            f"digest = {_q(entry.digest)}",
            f"size = {entry.size}",
            f"mode = 0o{entry.mode:o}",
            "",
        ]
    return "\n".join(lines)


def read_receipt(path: Path) -> Receipt | None:
    if not path.is_file():
        return None
    try:
        data = ckc.parse_toml_subset(path.read_text(encoding="utf-8"))
    except ckc.CkcError as err:
        raise _fail("EINTEGRITY", f"receipt {path} is malformed: {err.message}", "", EXIT_VERIFY)
    table = data.get("receipt")
    if not isinstance(table, dict):
        raise _fail("EINTEGRITY", f"receipt {path} is missing the [receipt] table", "", EXIT_VERIFY)
    files = []
    for item in data.get("files", []) or []:
        if not isinstance(item, dict):
            continue
        files.append(
            ReceiptFile(
                str(item.get("target", "")),
                str(item.get("digest", "")),
                int(item.get("size", 0)),
                int(item.get("mode", 0)),
            )
        )
    return Receipt(
        str(table.get("name", "")),
        str(table.get("scope", "")),
        str(table.get("rev", "")),
        str(table.get("prev_rev", "")),
        str(table.get("channel", DEFAULT_CHANNEL)),
        str(table.get("activated_at", "")),
        files,
    )


def iter_receipts(layout: Layout):
    """产出 `(scope, name, path, prev: bool)`；顺序确定（scope 字节序 → 名字字节序）。"""
    if not layout.receipts.is_dir():
        return
    for scope_dir in sorted(layout.receipts.iterdir(), key=lambda p: p.name.encode("utf-8")):
        if not scope_dir.is_dir():
            continue
        for path in sorted(scope_dir.glob("*.toml"), key=lambda p: p.name.encode("utf-8")):
            stem = path.name[: -len(".toml")]
            prev = stem.endswith(".prev")
            name = stem[: -len(".prev")] if prev else stem
            yield scope_dir.name, name, path, prev


# ---------------------------------------------------------------- 事务日志（3.6）


class Txn:
    """一个安装/卸载/回滚事务：`txn/<utc>-<pid>.jsonl` + 前像目录 `txn/<utc>-<pid>/`。"""

    def __init__(self, layout: Layout, op: str, name: str, scope: str, rev: str, prev_rev: str):
        self.layout = layout
        stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
        base = f"{stamp}-{os.getpid()}"
        identifier, suffix = base, 1
        while (layout.txn / f"{identifier}.jsonl").exists() or (layout.txn / identifier).exists():
            suffix += 1
            identifier = f"{base}-{suffix}"
        self.id = identifier
        self.path = layout.txn / f"{identifier}.jsonl"
        self.backup_dir = layout.txn / identifier
        self.seq = 0
        self.records: list[dict] = []
        self.op = op
        self.name = name
        self.scope = scope
        self.rev = rev
        self.prev_rev = prev_rev
        self.backups: dict[str, str] = {}

    def record(self, op: str, **fields) -> dict:
        self.seq += 1
        self.records.append({"seq": self.seq, "op": op, **fields})
        line = json.dumps(self.records[-1], sort_keys=True, ensure_ascii=False)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(line + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        return self.records[-1]

    def begin(self) -> None:
        self.record(
            "begin",
            kind=self.op,
            name=self.name,
            scope=self.scope,
            rev=self.rev,
            prev_rev=self.prev_rev,
            intent_digest=self.rev,
        )

    def backup(self, target: str, path: Path) -> None:
        """把前像拷进事务目录，并**在写目标之前**记录 `backup`（WAL 顺序）。"""
        self.backup_dir.mkdir(parents=True, exist_ok=True)
        index = len(self.backups)
        dst = self.backup_dir / str(index)
        shutil.copyfile(path, dst)
        os.chmod(dst, os.stat(path).st_mode & 0o777)
        self.backups[target] = str(dst.relative_to(self.layout.txn.parent))
        self.record("backup", target=target, backup=self.backups[target])


def read_journal(path: Path) -> list[dict]:
    records = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            records.append(json.loads(line))
    return records


def journal_state(records: list[dict]) -> str:
    """`committed` / `rolled-back` / `open`（未完成 = 需要 `recover`）。"""
    ops = {r.get("op") for r in records}
    if "commit" in ops:
        return "committed"
    if "rollback" in ops:
        return "rolled-back"
    return "open"


# ---------------------------------------------------------------- store（内容寻址）


def put_blob(layout: Layout, data: bytes) -> str:
    """按内容哈希把 blob 放进 store（同内容只存一份，原子落盘）。"""
    digest = _digest_of(data)
    dst = layout.store / digest
    if dst.is_file():
        return digest
    layout.store.mkdir(parents=True, exist_ok=True)
    tmp = layout.store / f".tmp-{os.getpid()}-{digest[:8]}"
    with tmp.open("wb") as handle:
        handle.write(data)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp, dst)
    return digest


# ---------------------------------------------------------------- 包解析（复用 P0a）


class Package:
    """一个已校验的本地 `.ckc`：容器报告 + 打包期规则重跑后的清单 + 每个文件的内容。"""

    __slots__ = ("path", "data", "report", "manifest", "files")

    def __init__(self, path, data, report, manifest, files):
        self.path = path
        self.data = data
        self.report = report
        self.manifest = manifest
        self.files = files

    @property
    def rev(self) -> str:
        return self.report.header.rev


def extract_payload(payload: bytes) -> list[tuple[str, int, bytes]]:
    """按 ustar 走一遍 payload，返回 `(archive_name, mode, data)`（结构由 `ckc.parse_payload` 校验）。"""
    entries = ckc.parse_payload(payload)
    out = []
    offset = 0
    for entry in entries:
        start = offset + ckc.BLOCK
        out.append((entry.name, entry.mode, payload[start:start + entry.size]))
        offset = start + entry.size + (-entry.size % ckc.BLOCK)
    return out


def load_package(path: Path) -> Package:
    """读一个 `.ckc`：容器校验（`ckc.verify_bytes`）→ **重跑打包期规则** → 逐文件内容。"""
    if not path.is_file():
        raise _fail("ENOENT", f"package not found: {path}", "", EXIT_IO)
    if path.suffix != ckc.SUFFIX:
        raise _fail("EINVAL", f"{path.name!r} does not have the .ckc suffix")
    data = path.read_bytes()
    try:
        report = ckc.verify_bytes(data)
    except ckc.CkcError as err:
        raise _fail(err.errno, err.message, err.hint, err.exit_code)
    payload = data[report.header.payload_off:]
    entries = dict()
    for name, mode, blob in extract_payload(payload):
        entries[name] = (mode, blob)
    manifest_name = report.entries[0].name
    pkg_toml = entries[manifest_name][1]
    try:
        parsed = ckc.parse_toml_subset(pkg_toml.decode("utf-8"))
    except (ckc.CkcError, UnicodeDecodeError) as err:
        raise _verify_fail(f"PKG.toml inside the payload is unusable: {err}")
    try:
        # 关键：安装期**重新**跑一遍域边界 / scope 白名单 / 特权位 / 重复 target。
        manifest = ckc.manifest_from_data(parsed)
    except ckc.CkcError as err:
        raise _fail(err.errno, err.message, err.hint, err.exit_code)

    files = []
    for entry in manifest.files:
        if entry.archive_name not in entries:
            raise _verify_fail(f"declared file {entry.target!r} is missing from the payload")
        payload_mode, blob = entries[entry.archive_name]
        if payload_mode != entry.mode:
            raise _verify_fail(
                f"{entry.target!r}: mode {oct(entry.mode)} in PKG.toml != {oct(payload_mode)} in the payload",
            )
        files.append((entry.target, entry.mode, blob))
    return Package(path, data, report, manifest, files)


def resolve_package(layout: Layout, spec: str, source: str | None) -> Package:
    """`<name>[@rev]` → 本地 `.ckc`。

    来源默认是 `<root>/cache/pkgs`（`--from` 可以指向一个目录或直接一个 `.ckc`）。
    多个候选且未指定 rev ⇒ `EINVAL`（不做「猜最新的」：可预测优先于方便）。
    """
    name, _, rev = spec.partition("@")
    if not name:
        raise _fail("EINVAL", f"empty package name in {spec!r}")
    origin = Path(source) if source else layout.cache / "pkgs"
    if origin.is_file():
        # 直接指定一个 `.ckc`：错误直接冒泡（后缀错 / 容器校验失败 / 名字或 rev 不符） ——
        # 「明确指了一个文件」不该被当成「目录扫描没找到候选」那样默默跳过。
        package = load_package(origin)
        if package.report.name != name or (rev and _norm_rev(rev) != package.rev):
            raise _fail("ENOENT", f"no local .ckc provides {spec!r} in {origin}")
        return package
    if not origin.is_dir():
        raise _fail("ENOENT", f"source not found: {origin}", "用 --from 指定本地 .ckc 或目录", EXIT_IO)
    candidates_paths = sorted(origin.glob(f"*{ckc.SUFFIX}"), key=lambda p: p.name.encode("utf-8"))

    hits = []
    for path in candidates_paths:
        try:
            package = load_package(path)
        except CpmError:
            continue
        if package.report.name != name:
            continue
        if rev and _norm_rev(rev) != package.rev:
            continue
        hits.append(package)
    if not hits:
        raise _fail("ENOENT", f"no local .ckc provides {spec!r} in {origin}")
    if len(hits) > 1:
        raise _fail(
            "EINVAL",
            f"{len(hits)} local .ckc files provide {name!r}",
            "指定 rev（name@sha256:…）以消除歧义",
        )
    return hits[0]


# ---------------------------------------------------------------- 事务阶段

PHASES = ("stage", "verify", "activate", "commit")


def _inject(args: argparse.Namespace, phase: str) -> None:
    """测试开关（见模块文档的「测试开关」一节）。"""
    if getattr(args, "crash_after", None) == phase:
        print(f"cpm: crash-after {phase} (simulated power loss; txn left uncommitted)")
        os._exit(9)
    if getattr(args, "fail_after", None) == phase:
        raise _fail("ETXN", f"injected failure after phase {phase}", "", EXIT_TXN)


def _stage_files(txn: Txn, layout: Layout, files) -> list[ReceiptFile]:
    """把每个文件写进 store（内容寻址），产出回执照用的文件表。"""
    staged = []
    for target, mode, blob in files:
        digest = put_blob(layout, blob)
        txn.record("stage", target=target, blob=f"sha256:{digest}", size=len(blob), mode=mode)
        staged.append(ReceiptFile(target, f"sha256:{digest}", len(blob), mode))
    return staged


def _verify_phase(txn: Txn, layout: Layout, staged: list[ReceiptFile]) -> None:
    """落盘前检查：现在只断言「blob 都在 store 且长度一致」（依赖/空间留给后续版本）。"""
    for entry in staged:
        path = layout.blob(_bare(entry.digest))
        if not path.is_file() or path.stat().st_size != entry.size:
            raise _verify_fail(f"staged blob for {entry.target!r} is missing or truncated")
    txn.record("verify", result="ok", files=len(staged))


def _activate_file(txn: Txn, layout: Layout, scope: str, entry: ReceiptFile) -> None:
    """备份前像（若有）→ 记录 `activate` → 从 store 落地（都在写目标之前记日志）。"""
    target = layout.target_path(scope, entry.target)
    if target.exists():
        txn.backup(entry.target, target)
    txn.record("activate", target=entry.target, blob=entry.digest)
    target.parent.mkdir(parents=True, exist_ok=True)
    src = layout.blob(_bare(entry.digest))
    shutil.copyfile(src, target)
    os.chmod(target, entry.mode & 0o777)


def _remove_file(txn: Txn, layout: Layout, scope: str, target_rel: str) -> None:
    target = layout.target_path(scope, target_rel)
    if target.exists():
        txn.backup(target_rel, target)
    txn.record("remove", target=target_rel)
    try:
        target.unlink()
    except FileNotFoundError:
        pass


def _missing_files(layout: Layout, scope: str, files) -> list[str]:
    """只看存在性（`stat`，不读内容）：`status` 用它保持「不扫 store、不读 catalog」的预算。"""
    return [
        entry.target
        for entry in files
        if not layout.target_path(scope, entry.target).is_file()
    ]


def _verify_files(layout: Layout, scope: str, files) -> list[str]:
    """逐文件重读并与期望摘要比对（4.2 第 3 层 / `cpm verify`）。返回不符的路径。"""
    bad = []
    for entry in files:
        path = layout.target_path(scope, entry.target)
        if not path.is_file():
            bad.append(f"{entry.target} (missing)")
            continue
        data = path.read_bytes()
        if _digest_of(data) != _bare(entry.digest) or len(data) != entry.size:
            bad.append(f"{entry.target} (digest mismatch)")
    return bad


def _snapshot_receipts(txn: Txn, layout: Layout, name: str, scope: str) -> None:
    """回执轮转前把「当前 + 上一修订」两个文件快照进事务目录，供回滚精确还原。"""
    current = layout.receipt(scope, name)
    prev = layout.receipt_prev(scope, name)
    txn.backup_dir.mkdir(parents=True, exist_ok=True)
    for kind, path in (("current", current), ("prev", prev)):
        if path.is_file():
            shutil.copyfile(path, txn.backup_dir / f"receipt-{kind}.toml")
    txn.record(
        "receipt",
        scope=scope,
        name=name,
        cur_existed=current.is_file(),
        prev_existed=prev.is_file(),
    )


def _write_receipts(layout: Layout, scope: str, name: str, new_current: Receipt | None, rotate_prev: bool = True) -> None:
    """原子写入回执：当前 → 上一修订，再写新的当前（`new_current=None` = 卸载）。

    `rotate_prev=False` 用于**同一修订的自愈重装**（4.3 的「未激活即重新激活」）:
    此时修订号没变，轮转只会把上一修订挤掉，因此只就地重写当前回执，历史不动。
    """
    current = layout.receipt(scope, name)
    prev = layout.receipt_prev(scope, name)
    current.parent.mkdir(parents=True, exist_ok=True)
    old = read_receipt(current)
    if new_current is None:
        for path in (prev, current):
            try:
                path.unlink()
            except FileNotFoundError:
                pass
        return
    if not rotate_prev:
        tmp = current.with_name(current.name + ".tmp")
        tmp.write_text(receipt_to_toml(new_current), encoding="utf-8")
        os.replace(tmp, current)
        return
    if old is not None:
        tmp = prev.with_name(prev.name + ".tmp")
        tmp.write_text(receipt_to_toml(old), encoding="utf-8")
        os.replace(tmp, prev)
    else:
        try:
            prev.unlink()
        except FileNotFoundError:
            pass
    tmp = current.with_name(current.name + ".tmp")
    tmp.write_text(receipt_to_toml(new_current), encoding="utf-8")
    os.replace(tmp, current)


def rollback_txn(txn: Txn, layout: Layout, reason: str) -> None:
    """按日志逆序恢复：前像优先，没有前像的新增文件删除，回执回到事务前。"""
    records = read_journal(txn.path) if txn.path.exists() else txn.records
    backups = {r["target"]: r["backup"] for r in records if r.get("op") == "backup"}
    for record in reversed(records):
        op = record.get("op")
        if op in ("activate", "remove"):
            target_rel = record.get("target", "")
            if target_rel in backups:
                dst = layout.txn.parent / backups[target_rel]
                if dst.is_file():
                    real = layout.target_path(record.get("scope", txn.scope), target_rel)
                    real.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(dst, real)
            elif op == "activate":
                real = layout.target_path(record.get("scope", txn.scope), target_rel)
                try:
                    real.unlink()
                except FileNotFoundError:
                    pass
        elif op == "receipt":
            name = record.get("name", txn.name)
            scope = record.get("scope", txn.scope)
            for kind, path, existed in (
                ("current", layout.receipt(scope, name), record.get("cur_existed", False)),
                ("prev", layout.receipt_prev(scope, name), record.get("prev_existed", False)),
            ):
                snapshot = txn.backup_dir / f"receipt-{kind}.toml"
                if snapshot.is_file():
                    path.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(snapshot, path)
                elif not existed:
                    try:
                        path.unlink()
                    except FileNotFoundError:
                        pass
    shutil.rmtree(txn.backup_dir, ignore_errors=True)
    if journal_state(records) == "open":
        txn.record("rollback", reason=reason)


# ---------------------------------------------------------------- 安装（add / apply）

def install(layout: Layout, package: Package, scope: str, args, channel: str) -> dict:
    """一个安装事务。返回 `{status, rev, files, txn}`；幂等命中时 `status="skipped"` 且**零写入**。"""
    name = package.report.name
    if package.report.scope != scope:
        raise _fail(
            "EINVAL",
            f"package {name!r} declares scope {package.report.scope!r}, not {scope!r}",
            "scope 由包定义（域边界 5.2）；不要用 --scope 越域安装",
        )
    declared_abi = package.manifest.abi_kernel
    if declared_abi is not None and int(declared_abi) != int(args.kernel_abi):
        raise _fail(
            "EINVAL",
            f"package {name!r} needs ABI {declared_abi}, this host provides --kernel-abi {args.kernel_abi}",
        )

    # plan 阶段：不写任何文件
    staged_preview = [ReceiptFile(t, f"sha256:{_digest_of(b)}", len(b), m) for t, m, b in package.files]

    if args.dry_run:
        current = read_receipt(layout.receipt(scope, name))
        same_rev = current is not None and current.rev == package.rev
        return {
            "status": "planned",
            "name": name,
            "scope": scope,
            "rev": package.rev,
            "prev_rev": current.prev_rev if same_rev else (current.rev if current else ""),
            "files": len(staged_preview),
            "txn": None,
        }

    with Lock(layout):
        # 锁内重读：单写者（7.3）—— 即便是幂等命中也要先过锁，这样并发的写命令一定 `ELOCK`
        current = read_receipt(layout.receipt(scope, name))
        same_rev = current is not None and current.rev == package.rev
        if same_rev and not _verify_files(layout, scope, current.files):
            return {
                "status": "skipped",
                "reason": "already active",
                "name": name,
                "scope": scope,
                "rev": package.rev,
                "files": len(current.files),
                "txn": None,
            }
        txn = Txn(layout, "add", name, scope, package.rev, current.rev if current else "")
        txn.begin()
        try:
            staged = _stage_files(txn, layout, package.files)
            _inject(args, "stage")
            _verify_phase(txn, layout, staged)
            _inject(args, "verify")
            for entry in staged:
                _activate_file(txn, layout, scope, entry)
            _inject(args, "activate")
            bad = _verify_files(layout, scope, staged)
            if bad:
                raise _verify_fail("post-activate verification failed: " + ", ".join(bad))
            _snapshot_receipts(txn, layout, name, scope)
            prev_rev = current.prev_rev if same_rev else (current.rev if current else "")
            _write_receipts(
                layout,
                scope,
                name,
                Receipt(name, scope, package.rev, prev_rev, channel, _utc_now(), staged),
                rotate_prev=not same_rev,
            )
            txn.record("commit", receipt=f"receipts/{scope}/{name}.toml", files=len(staged))
            _inject(args, "commit")
        except BaseException as exc:  # noqa: BLE001  （含注入的失败；回滚后原样抛出）
            rollback_txn(txn, layout, reason=getattr(exc, "errno", type(exc).__name__))
            raise
        return {
            "status": "installed",
            "name": name,
            "scope": scope,
            "rev": package.rev,
            "prev_rev": prev_rev,
            "files": len(staged),
            "txn": txn.id,
            "healed": same_rev,
        }


# ---------------------------------------------------------------- 命令


def cmd_init(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.create()
    if not layout.manifest.exists():
        layout.manifest.write_text(
            "[settings]\n"
            f"channel = {_q(args.channel)}\n"
            'scope = "user"\n',
            encoding="utf-8",
        )
    print(
        f"cpm: init ok root={layout.root} "
        "dirs=scopes,store/sha256,receipts,txn,cache,catalog,trust"
    )
    return EXIT_OK


def cmd_add(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    package = resolve_package(layout, args.name, args.from_)
    scope = args.scope or package.report.scope
    channel = args.channel or DEFAULT_CHANNEL
    result = install(layout, package, scope, args, channel)
    if args.json:
        print(json.dumps(result, sort_keys=True, ensure_ascii=False))
    elif result["status"] == "skipped":
        print(
            f"cpm: add skip name={result['name']} scope={result['scope']} rev={result['rev']} "
            f"(already active)"
        )
    elif result["status"] == "planned":
        print(
            f"cpm: add plan name={result['name']} scope={result['scope']} rev={result['rev']} "
            f"prev_rev={result['prev_rev']} files={result['files']}"
        )
    else:
        print(
            f"cpm: add ok name={result['name']} scope={result['scope']} rev={result['rev']} "
            f"files={result['files']} txn={result['txn']}"
        )
    return EXIT_OK


def cmd_remove(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    current = read_receipt(layout.receipt(args.scope, args.name))
    if current is None:
        raise _fail("ENOENT", f"{args.name!r} is not installed in scope {args.scope!r}")
    with Lock(layout):
        txn = Txn(layout, "remove", args.name, args.scope, current.rev, current.rev)
        txn.begin()
        kept = []
        try:
            _snapshot_receipts(txn, layout, args.name, args.scope)
            for entry in current.files:
                target = layout.target_path(args.scope, entry.target)
                if not args.purge and _is_config(entry.target) and target.exists():
                    txn.record("skip_config", target=entry.target)
                    kept.append(entry.target)
                    continue
                _remove_file(txn, layout, args.scope, entry.target)
            _write_receipts(layout, args.scope, args.name, None)
            txn.record("commit", receipt="", files=len(current.files) - len(kept))
        except BaseException as exc:  # noqa: BLE001
            rollback_txn(txn, layout, reason=getattr(exc, "errno", type(exc).__name__))
            raise
    if args.json:
        print(json.dumps(
            {"status": "removed", "name": args.name, "scope": args.scope,
             "removed": len(current.files) - len(kept), "kept": kept, "txn": txn.id},
            sort_keys=True, ensure_ascii=False,
        ))
    else:
        print(
            f"cpm: remove ok name={args.name} scope={args.scope} "
            f"removed={len(current.files) - len(kept)} kept={len(kept)} txn={txn.id}"
        )
        for target in kept:
            print(f"cpm: skip (config exists) {target}")
    return EXIT_OK


def _is_config(target: str) -> bool:
    """配置文件白名单（5.3 / 13.1）：`/etc/**` 或常见配置后缀 —— 卸载默认保留。"""
    return target.startswith("/etc/") or target.lower().endswith(CONFIG_SUFFIXES)


def cmd_rollback(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    current = read_receipt(layout.receipt(args.scope, args.name))
    prev = read_receipt(layout.receipt_prev(args.scope, args.name))
    if current is None:
        raise _fail("ENOENT", f"{args.name!r} is not installed in scope {args.scope!r}")
    if prev is None:
        raise _fail("ENOENT", f"{args.name!r} has no previous revision to roll back to")
    # `--offline` 的语义就是「不触网」：本实现本来就只读 store，所以直接成立（6.4 预算）。
    _ = args.offline
    with Lock(layout):
        txn = Txn(layout, "rollback", args.name, args.scope, prev.rev, current.rev)
        txn.begin()
        try:
            _snapshot_receipts(txn, layout, args.name, args.scope)
            keep = {entry.target for entry in prev.files}
            for entry in current.files:
                if entry.target not in keep:
                    _remove_file(txn, layout, args.scope, entry.target)
            for entry in prev.files:
                blob = layout.blob(_bare(entry.digest))
                if not blob.is_file():
                    raise _fail(
                        "ENOENT",
                        f"blob {entry.digest} for {entry.target!r} is missing from the store",
                        "该修订的文件已被 gc 回收",
                        EXIT_IO,
                    )
                _activate_file(txn, layout, args.scope, entry)
            bad = _verify_files(layout, args.scope, prev.files)
            if bad:
                raise _verify_fail("post-rollback verification failed: " + ", ".join(bad))
            restored = Receipt(
                prev.name, prev.scope, prev.rev, current.rev, prev.channel, _utc_now(), prev.files
            )
            _write_receipts(layout, args.scope, args.name, restored)
            txn.record("commit", receipt=f"receipts/{args.scope}/{args.name}.toml", kind="rollback")
        except BaseException as exc:  # noqa: BLE001
            rollback_txn(txn, layout, reason=getattr(exc, "errno", type(exc).__name__))
            raise
    if args.json:
        print(json.dumps(
            {"status": "rolled-back", "name": args.name, "scope": args.scope,
             "from": current.rev, "to": prev.rev, "txn": txn.id},
            sort_keys=True, ensure_ascii=False,
        ))
    else:
        print(
            f"cpm: rollback ok name={args.name} scope={args.scope} "
            f"from={current.rev} to={prev.rev} txn={txn.id}"
        )
    return EXIT_OK


def cmd_status(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    rows = []
    for scope, name, path, prev in iter_receipts(layout):
        if prev:
            continue
        if args.name and name != args.name:
            continue
        if args.scope and scope != args.scope:
            continue
        receipt = read_receipt(path)
        if receipt is None:
            continue
        # 只 stat：内容层校验交给 `cpm verify`（6.3 预算：status 不读文件内容）
        missing = _missing_files(layout, scope, receipt.files)
        rows.append({
            "name": name,
            "scope": scope,
            "state": "activated" if not missing else "incomplete",
            "rev": receipt.rev,
            "prev_rev": receipt.prev_rev,
            "files": len(receipt.files),
            "missing": missing,
        })
    if args.json:
        print(json.dumps({"packages": rows}, sort_keys=True, ensure_ascii=False))
    elif not rows:
        print("cpm: status no packages")
    else:
        for row in rows:
            print(
                f"cpm: status name={row['name']} scope={row['scope']} state={row['state']} "
                f"rev={row['rev']} prev_rev={row['prev_rev']} files={row['files']}"
            )
    if args.name and not rows:
        raise _fail("ENOENT", f"{args.name!r} is not installed")
    return EXIT_OK


def cmd_ls(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    rows = []
    for scope, name, path, prev in iter_receipts(layout):
        if prev:
            continue
        if args.scope and scope != args.scope:
            continue
        receipt = read_receipt(path)
        if receipt is None:
            continue
        rows.append({"name": name, "scope": scope, "rev": receipt.rev, "files": len(receipt.files)})
    if args.json:
        print(json.dumps({"packages": rows}, sort_keys=True, ensure_ascii=False))
    else:
        for row in rows:
            print(f"cpm: ls name={row['name']} scope={row['scope']} rev={row['rev']} files={row['files']}")
        print(f"cpm: ls count={len(rows)}")
    return EXIT_OK


def cmd_verify(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    checked = 0
    failed = []
    for scope, name, path, prev in iter_receipts(layout):
        if prev:
            continue
        if args.name and name != args.name:
            continue
        receipt = read_receipt(path)
        if receipt is None:
            continue
        checked += 1
        for bad in _verify_files(layout, scope, receipt.files):
            failed.append(f"{name}@{scope}: {bad}")
    if args.json:
        print(json.dumps(
            {"checked": checked, "failed": failed}, sort_keys=True, ensure_ascii=False
        ))
    else:
        for item in failed:
            print(f"cpm: verify mismatch {item}")
        print(f"cpm: verify checked={checked} failed={len(failed)}")
    if failed:
        raise _verify_fail(f"{len(failed)} file(s) do not match their receipt")
    if args.name and checked == 0:
        raise _fail("ENOENT", f"{args.name!r} is not installed")
    return EXIT_OK


# ---------------------------------------------------------------- plan / apply（cpkg.toml）

def read_intent(layout: Layout, manifest_path: str | None) -> tuple[str, str, list[dict]]:
    """读 `cpkg.toml`（3.4）：`(channel, default_scope, wants[])`。"""
    path = Path(manifest_path) if manifest_path else layout.manifest
    if not path.is_file():
        raise _fail("ENOENT", f"intent manifest not found: {path}", "cpm init 会写出一个骨架", EXIT_IO)
    try:
        data = ckc.parse_toml_subset(path.read_text(encoding="utf-8"))
    except ckc.CkcError as err:
        raise _fail("EINVAL", f"cpkg.toml is invalid: {err.message}")
    settings = data.get("settings") if isinstance(data.get("settings"), dict) else {}
    channel = str(settings.get("channel", DEFAULT_CHANNEL))
    default_scope = str(settings.get("scope", "user"))
    wants = data.get("want") or []
    if not isinstance(wants, list):
        raise _fail("EINVAL", "cpkg.toml: [[want]] must be an array of tables")
    seen: dict[str, str] = {}
    for index, want in enumerate(wants):
        if not isinstance(want, dict):
            raise _fail("EINVAL", f"cpkg.toml: [[want]] #{index} must be a table")
        name = str(want.get("name", ""))
        if not name:
            raise _fail("EINVAL", f"cpkg.toml: [[want]] #{index} is missing `name`")
        rev = str(want.get("rev", ""))
        if name in seen and seen[name] != rev:
            raise _fail("EINVAL", f"cpkg.toml: {name!r} is wanted at two different revisions")
        seen[name] = rev
    return channel, default_scope, wants


def compute_plan(layout: Layout, args) -> list[dict]:
    channel, default_scope, wants = read_intent(layout, args.manifest)
    plan = []
    for want in wants:
        name = str(want.get("name", ""))
        rev = str(want.get("rev", ""))
        scope = str(want.get("scope", default_scope))
        package = resolve_package(layout, f"{name}@{rev}" if rev else name, args.from_)
        if package.report.scope != scope:
            raise _fail(
                "EINVAL",
                f"package {name!r} declares scope {package.report.scope!r}, not {scope!r}",
            )
        current = read_receipt(layout.receipt(scope, name))
        if current is not None and current.rev == package.rev and not _verify_files(layout, scope, current.files):
            plan.append({
                "name": name, "scope": scope, "rev": package.rev,
                "prev_rev": current.prev_rev, "action": "skip", "reason": "already active",
                "ckc": str(package.path),
            })
        else:
            plan.append({
                "name": name, "scope": scope, "rev": package.rev,
                "prev_rev": current.rev if current else "", "action": "install",
                "ckc": str(package.path), "channel": channel,
            })
    return plan


def cmd_plan(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    plan = compute_plan(layout, args)
    if args.json:
        print(json.dumps({"plan": plan}, sort_keys=True, ensure_ascii=False))
    else:
        for item in plan:
            print(
                f"cpm: plan name={item['name']} scope={item['scope']} action={item['action']} "
                f"rev={item['rev']} prev_rev={item['prev_rev']}"
            )
        print(f"cpm: plan count={len(plan)} install={sum(1 for i in plan if i['action'] == 'install')}")
    return EXIT_OK


def cmd_apply(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    plan = compute_plan(layout, args)
    results = []
    for item in plan:
        if item["action"] == "skip":
            results.append({"name": item["name"], "scope": item["scope"], "rev": item["rev"], "status": "skipped"})
            continue
        if args.dry_run:
            results.append({"name": item["name"], "scope": item["scope"], "rev": item["rev"], "status": "planned"})
            continue
        package = load_package(Path(item["ckc"]))
        outcome = install(layout, package, item["scope"], args, item.get("channel", DEFAULT_CHANNEL))
        results.append({"name": item["name"], "scope": item["scope"], "rev": item["rev"], "status": outcome["status"]})
    if args.json:
        print(json.dumps({"results": results}, sort_keys=True, ensure_ascii=False))
    else:
        for row in results:
            print(f"cpm: apply name={row['name']} scope={row['scope']} status={row['status']} rev={row['rev']}")
        skipped = sum(1 for r in results if r["status"] == "skipped")
        print(f"cpm: apply count={len(results)} skipped={skipped}")
    return EXIT_OK


# ---------------------------------------------------------------- recover / gc

def cmd_recover(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    recovered = []
    with Lock(layout):
        for path in sorted(layout.txn.glob("*.jsonl"), key=lambda p: p.name.encode("utf-8")):
            records = read_journal(path)
            if journal_state(records) != "open":
                continue
            begin = next((r for r in records if r.get("op") == "begin"), {})
            txn = Txn.__new__(Txn)
            txn.layout = layout
            txn.id = path.stem
            txn.path = path
            txn.backup_dir = layout.txn / path.stem
            txn.seq = records[-1].get("seq", 0)
            txn.records = records
            txn.op = begin.get("kind", "")
            txn.name = begin.get("name", "")
            txn.scope = begin.get("scope", "")
            txn.rev = begin.get("rev", "")
            txn.prev_rev = begin.get("prev_rev", "")
            txn.backups = {}
            rollback_txn(txn, layout, reason="uncommitted (recovered)")
            recovered.append(txn.id)
            print(f"cpm: recover rolled back txn={txn.id} reason=uncommitted")
    if args.json:
        print(json.dumps({"recovered": recovered}, sort_keys=True, ensure_ascii=False))
    else:
        print(f"cpm: recover count={len(recovered)}")
    return EXIT_OK


def cmd_gc(args: argparse.Namespace) -> int:
    layout = Layout(args.root)
    layout.ensure()
    referenced = set()
    for _scope, _name, path, _prev in iter_receipts(layout):
        receipt = read_receipt(path)
        if receipt is None:
            continue
        referenced.add(_bare(receipt.rev))
        for entry in receipt.files:
            referenced.add(_bare(entry.digest))
    blobs = [
        p for p in layout.store.glob("*")
        if p.is_file() and not p.name.startswith(".") and p.name not in referenced
    ]
    journals = []
    for path in sorted(layout.txn.glob("*.jsonl"), key=lambda p: p.name.encode("utf-8")):
        if journal_state(read_journal(path)) != "open":
            journals.append(path)
    cache_entries = sorted(layout.cache.glob("*")) if layout.cache.is_dir() else []

    if args.dry_run:
        if not args.json:
            for path in blobs:
                print(f"cpm: gc would-remove blob={path.name}")
            for path in journals:
                print(f"cpm: gc would-remove txn={path.name}")
            for path in cache_entries:
                print(f"cpm: gc would-remove cache={path.name}")
            print(f"cpm: gc dry-run blobs={len(blobs)} journals={len(journals)} cache={len(cache_entries)}")
        else:
            print(json.dumps(
                {"dry_run": True, "blobs": [p.name for p in blobs],
                 "journals": [p.name for p in journals], "cache": [p.name for p in cache_entries]},
                sort_keys=True, ensure_ascii=False,
            ))
        return EXIT_OK

    with Lock(layout):
        for path in blobs:
            path.unlink()
        for path in journals:
            shutil.rmtree(layout.txn / path.stem, ignore_errors=True)
            path.unlink()
        for path in cache_entries:
            if path.is_dir():
                shutil.rmtree(path, ignore_errors=True)
            else:
                path.unlink()
    print(f"cpm: gc ok blobs={len(blobs)} journals={len(journals)} cache={len(cache_entries)}")
    return EXIT_OK


# ---------------------------------------------------------------- CLI


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="cpm", description="Cerlesse 包管理器（H-06 P0b / CKM-3 事务引擎）")
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p):
        p.add_argument("--root", required=True, help="仓库根（cpm init 建立的目录）")
        p.add_argument("--json", action="store_true", help="单行 JSON 输出")

    init = sub.add_parser("init", help="建立目录骨架")
    init.add_argument("--root", required=True)
    init.add_argument("--channel", default=DEFAULT_CHANNEL)
    init.set_defaults(func=cmd_init)

    add = sub.add_parser("add", help="安装一个本地 .ckc（name 或 name@rev）")
    common(add)
    add.add_argument("name")
    add.add_argument("--scope")
    add.add_argument("--from", dest="from_", help="本地 .ckc 或目录（默认 <root>/cache/pkgs）")
    add.add_argument("--channel", default="")
    add.add_argument("--kernel-abi", type=int, default=DEFAULT_KERNEL_ABI)
    add.add_argument("--dry-run", action="store_true")
    add.add_argument("--fail-after", choices=PHASES, help="测试开关：该阶段后注入失败（自动回滚）")
    add.add_argument("--crash-after", choices=PHASES, help="测试开关：该阶段后硬退出（留给 recover）")
    add.set_defaults(func=cmd_add)

    remove = sub.add_parser("remove", help="卸载（默认保留配置文件）")
    common(remove)
    remove.add_argument("name")
    remove.add_argument("--scope", default="user")
    remove.add_argument("--purge", action="store_true", help="连配置文件一起删")
    remove.set_defaults(func=cmd_remove)

    rollback = sub.add_parser("rollback", help="回到上一修订（纯本地）")
    common(rollback)
    rollback.add_argument("name")
    rollback.add_argument("--scope", default="user")
    rollback.add_argument("--offline", action="store_true", help="不触网（本实现恒为本地）")
    rollback.set_defaults(func=cmd_rollback)

    status = sub.add_parser("status", help="已激活 / 已改动状态（不读 catalog）")
    common(status)
    status.add_argument("name", nargs="?")
    status.add_argument("--scope")
    status.set_defaults(func=cmd_status)

    ls = sub.add_parser("ls", help="列出 scope 里的包")
    common(ls)
    ls.add_argument("--scope")
    ls.set_defaults(func=cmd_ls)

    verify = sub.add_parser("verify", help="用回执逐文件校验")
    common(verify)
    verify.add_argument("name", nargs="?")
    verify.set_defaults(func=cmd_verify)

    plan = sub.add_parser("plan", help="按 cpkg.toml 只算差异")
    common(plan)
    plan.add_argument("--manifest")
    plan.add_argument("--from", dest="from_")
    plan.set_defaults(func=cmd_plan)

    apply_cmd = sub.add_parser("apply", help="按 cpkg.toml 收敛实际状态到意图（幂等）")
    common(apply_cmd)
    apply_cmd.add_argument("--manifest")
    apply_cmd.add_argument("--from", dest="from_")
    apply_cmd.add_argument("--dry-run", action="store_true")
    apply_cmd.add_argument("--offline", action="store_true")
    apply_cmd.add_argument("--kernel-abi", type=int, default=DEFAULT_KERNEL_ABI)
    apply_cmd.add_argument("--fail-after", choices=PHASES)
    apply_cmd.add_argument("--crash-after", choices=PHASES)
    apply_cmd.set_defaults(func=cmd_apply)

    recover = sub.add_parser("recover", help="回滚未提交事务")
    common(recover)
    recover.set_defaults(func=cmd_recover)

    gc = sub.add_parser("gc", help="回收无引用 blob 与已完成事务日志")
    common(gc)
    gc.add_argument("--dry-run", action="store_true")
    gc.set_defaults(func=cmd_gc)

    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except CpmError as err:
        hint = f" ({err.hint})" if err.hint else ""
        print(f"cpm: error {err.errno} {err.message}{hint}", file=sys.stderr)
        return err.exit_code
    except ckc.CkcError as err:
        hint = f" ({err.hint})" if err.hint else ""
        print(f"cpm: error {err.errno} {err.message}{hint}", file=sys.stderr)
        return err.exit_code


if __name__ == "__main__":
    sys.exit(main())
