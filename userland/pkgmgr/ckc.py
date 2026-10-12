#!/usr/bin/env python3
"""CKC —— Cerlesse 内核包容器（`docs/package-manager.md` 第 3.2 节）。

## 为什么有独立后缀（而不是一个裸 tar）

`.ckc` = **C**erlesse **K**ernel **C**ontainer。后缀 `.ckc` + 文件头魔数 `CKC\\x01`
让一个内核包**一眼可辨、不会被当成普通 tar 随便解开**，同时把「校验所需的信息」
放在最前面：读 64 字节头就能判断版本、是否被截断、以及 payload 的 SHA-256，
不需要先解析整个 tar。

## 容器布局（v1）

```text
偏移  长度  字段             说明
0     4     magic            b"CKC\\x01"（"CKC" + 容器版本 1）
4     2     header_len       LE u16 = 64（头长固定，方便以后扩展时仍能定位 payload）
6     2     flags            LE u16 = 0（v1 未定义任何位；非 0 = 未知特性 → 拒绝）
8     8     payload_len      LE u64（payload 字节数）
16    8     payload_off      LE u64 = 64（payload 起始偏移）
24    32    payload_sha256   payload 的 SHA-256（= 修订标识 rev 的内容部分）
56    8     reserved         必须全 0（写入 0，读取时非 0 → 拒绝）
```

整型一律 **小端**；头长固定 64，因此 `payload_off` 目前恒为 64（字段仍然写出来，
因为读的时候要能拒绝「头长不匹配」的伪造包）。

**身份 = payload 的哈希**，不是整个文件的哈希：同样的文件内容 ⇒ 同样的 `rev`，
与打包时间、机器、头部字节无关（头部字段本身在 verify 时被逐一检查，但不参与身份）。
rev 写作 `sha256:<64 位小写十六进制>`。

## payload：确定性 ustar（自写，不用 `tarfile`）

| 规则 | 取值 | 理由 |
| ---- | ---- | ---- |
| 归档名 | **安装目标路径去掉开头的 `/`**（如 `usr/bin/hello`） | 容器的含义自解释：`info` 不需要读 `PKG.toml` 也能看出文件会落到哪 |
| 条目顺序 | `PKG.toml` 第一，其余按归档名的**字节序** | 消除文件系统迭代顺序的影响 |
| 格式 | POSIX ustar（`ustar\\0` + `00`），单条路径 ≤ 100 字节 | 不用 PAX 扩展头，避免「扩展头字段顺序」这种确定性陷阱 |
| `mtime` / `uid` / `gid` / `uname` / `gname` | 全 0 / 空 | 打包时间与构建者身份不进入产物 |
| `mode` | 取自 `PKG.toml`；`setuid`/`setgid`/`sticky` 一律拒绝 | 见 `docs/package-manager.md` 5.2 |
| 只允许常规文件 | 目录/符号链接/设备 → 报错 | 目录由解包方按路径创建，避免目录条目顺序问题 |
| 压缩 | 无 | 压缩参数会成为身份的一部分 |

## PKG.toml：只支持一个**明确的子集**

刻意不引入第三方 TOML 依赖（本项目的工具都是纯标准库）：

- `[table]` 段头（单层，不支持点号键）
- `key = "字符串"` / `key = 整数` / `key = 0o755` / `key = true|false`
- `[[array.of.tables]]`
- 行内表 `{ a = 1, b = "x" }`
- `#` 注释与空行

**不支持的构造（数组、多行字符串、日期、浮点…）会明确报错**，不静默忽略 ——
静默忽略会让「包里的元数据」与「作者写的元数据」悄悄不一致。

## 用法

```bash
python3 userland/pkgmgr/ckc.py pack   <dir> [-o out.ckc] [--print-rev]
python3 userland/pkgmgr/ckc.py verify <file.ckc>
python3 userland/pkgmgr/ckc.py info   <file.ckc>
```

退出码（与 `docs/package-manager.md` 9.2 一致）：`0` 成功、`1` 用法/参数错误
（含路径越界、超长路径）、`2` 校验失败、`4` I/O 或环境错误。
（`3` 事务失败 / `5` 并发冲突属于还没实现的 P0b 事务层。）
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import sys
from pathlib import Path

# ---- 容器常量 ----

SUFFIX = ".ckc"
MAGIC = b"CKC\x01"
VERSION = 1
HEADER_LEN = 64
PAYLOAD_OFF = HEADER_LEN
BLOCK = 512
MAX_NAME = 100
RESERVED = 8

# ---- 退出码（docs/package-manager.md 9.2） ----
EXIT_OK = 0
EXIT_USAGE = 1
EXIT_VERIFY = 2
EXIT_IO = 4

# ---- 包元数据的合法取值（docs/package-manager.md 3.1 / 5.2 / 8.2） ----

CATEGORIES = ("system", "driver", "userland", "service", "image-baseline")
SCOPES = ("system", "shared", "user")
#: 每个 scope 允许的安装目标前缀（按路径段比较，避免 `/etcmal` 命中 `/etc`）
SCOPE_TARGETS = {
    "system": ("/bin", "/sbin", "/lib", "/usr/bin", "/usr/sbin", "/usr/lib", "/etc", "/usr/share"),
    "shared": ("/usr/share", "/opt", "/var"),
    "user": ("/home", "/usr/share", "/opt"),
}
#: 允许出现的权限位（其余位 —— 尤其是 setuid/setgid/sticky —— 一律拒绝）
ALLOWED_MODE = 0o777


class CkcError(Exception):
    """带稳定 errno 名与退出码的错误（CLI 只打印，不抛栈）。"""

    def __init__(self, errno: str, message: str, hint: str = "", exit_code: int = EXIT_USAGE):
        super().__init__(message)
        self.errno = errno
        self.message = message
        self.hint = hint
        self.exit_code = exit_code


def _fail(errno: str, message: str, hint: str = "", exit_code: int = EXIT_USAGE) -> "CkcError":
    return CkcError(errno, message, hint, exit_code)


def _verify_fail(message: str, hint: str = "") -> "CkcError":
    """校验类失败统一退出码 2（docs/package-manager.md 9.2）。"""
    return _fail("EVERIFY", message, hint, EXIT_VERIFY)


# ---------------------------------------------------------------- TOML 子集


def parse_toml_subset(text: str) -> dict:
    """解析上文列出的 TOML 子集；遇到不支持的构造直接抛 `EINVAL`。"""
    root: dict = {}
    current: dict = root
    for lineno, raw in enumerate(text.splitlines(), 1):
        line = _strip_comment(raw).strip()
        if not line:
            continue
        if line.startswith("[["):
            if not line.endswith("]]"):
                raise _fail("EINVAL", f"line {lineno}: malformed array-of-tables header")
            name = line[2:-2].strip()
            _check_key(name, lineno)
            entries = root.setdefault(name, [])
            if not isinstance(entries, list):
                raise _fail("EINVAL", f"line {lineno}: {name!r} is already a table")
            current = {}
            entries.append(current)
            continue
        if line.startswith("["):
            if not line.endswith("]"):
                raise _fail("EINVAL", f"line {lineno}: malformed table header")
            name = line[1:-1].strip()
            _check_key(name, lineno)
            table = root.setdefault(name, {})
            if not isinstance(table, dict):
                raise _fail("EINVAL", f"line {lineno}: {name!r} is already an array of tables")
            current = table
            continue
        if "=" not in line:
            raise _fail("EINVAL", f"line {lineno}: expected `key = value`")
        key, _, value = line.partition("=")
        key = key.strip()
        _check_key(key, lineno)
        current[key] = _parse_value(value.strip(), lineno)
    return root


def _strip_comment(line: str) -> str:
    """去掉 `#` 注释（简单起见不支持字符串内的 `#`；子集里也不会用到）。"""
    index = line.find("#")
    return line if index < 0 else line[:index]


def _check_key(key: str, lineno: int) -> None:
    if not key or not re.fullmatch(r"[A-Za-z0-9_.\-]+", key):
        raise _fail("EINVAL", f"line {lineno}: unsupported key {key!r}")


def _parse_value(value: str, lineno: int):
    if value.startswith('"'):
        if not value.endswith('"') or len(value) < 2:
            raise _fail("EINVAL", f"line {lineno}: unterminated string")
        return value[1:-1]
    if value.startswith("{"):
        if not value.endswith("}"):
            raise _fail("EINVAL", f"line {lineno}: unterminated inline table")
        table: dict = {}
        for part in _split_inline(value[1:-1], lineno):
            if "=" not in part:
                raise _fail("EINVAL", f"line {lineno}: malformed inline table entry")
            k, _, v = part.partition("=")
            _check_key(k.strip(), lineno)
            table[k.strip()] = _parse_value(v.strip(), lineno)
        return table
    if value.startswith("["):
        raise _fail(
            "EINVAL",
            f"line {lineno}: arrays are outside the supported TOML subset",
            "用 [[array.of.tables]] 或拆成多个键",
        )
    if value in ("true", "false"):
        return value == "true"
    if value.startswith("0o"):
        body = value[2:].replace("_", "")
        if not body or not re.fullmatch(r"[0-7]+", body):
            raise _fail("EINVAL", f"line {lineno}: malformed octal integer {value!r}")
        return int(body, 8)
    if re.fullmatch(r"[0-9][0-9_]*", value):
        return int(value.replace("_", ""))
    if re.fullmatch(r"-?[0-9]+\.[0-9]+", value):
        raise _fail("EINVAL", f"line {lineno}: floats are outside the supported TOML subset")
    raise _fail("EINVAL", f"line {lineno}: unsupported value {value!r}")


def _split_inline(body: str, lineno: int) -> list:
    """按逗号切行内表（子集里不做嵌套，因此不需要真正的词法分析）。"""
    parts, current, in_string = [], "", False
    for ch in body:
        if ch == '"':
            in_string = not in_string
        if ch == "," and not in_string:
            parts.append(current)
            current = ""
        else:
            current += ch
    if in_string:
        raise _fail("EINVAL", f"line {lineno}: unterminated string in inline table")
    parts.append(current)
    return [p for p in (p.strip() for p in parts) if p]


# ---------------------------------------------------------------- 包定义（PKG.toml）


class FileEntry:
    """包内一个文件：源（作者目录内）→ 目标（安装后的绝对路径）。"""

    __slots__ = ("source", "target", "mode")

    def __init__(self, source: str, target: str, mode: int):
        self.source = source
        self.target = target
        self.mode = mode

    @property
    def archive_name(self) -> str:
        """归档名 = 目标路径去掉开头的 `/`（见模块文档）。"""
        return self.target.lstrip("/")


class Manifest:
    __slots__ = ("name", "version", "category", "scope", "abi_kernel", "files")

    def __init__(self, name, version, category, scope, abi_kernel, files):
        self.name = name
        self.version = version
        self.category = category
        self.scope = scope
        self.abi_kernel = abi_kernel
        self.files = files


def load_manifest(root: Path) -> Manifest:
    """读取并**校验**包定义（`PKG.toml`）。所有规则都对应文档里的条款。"""
    manifest_path = root / "PKG.toml"
    if not manifest_path.is_file():
        raise _fail("ENOENT", f"missing PKG.toml in {root}")
    try:
        data = parse_toml_subset(manifest_path.read_text(encoding="utf-8"))
    except UnicodeDecodeError as exc:
        raise _fail("EINVAL", f"PKG.toml is not valid UTF-8: {exc}")
    return manifest_from_data(data)


def manifest_from_data(data: dict) -> Manifest:
    """从**已解析**的 `PKG.toml` 子集构造并校验 `Manifest`。

    与 [`load_manifest`] 共用同一套规则（域边界、scope 路径白名单、特权位、
    重复 target、hooks 拒绝）。**为什么单独暴露这个函数**：安装期（同目录的 `cpm.py`）
    必须对 `.ckc` payload 里的 `PKG.toml` **重新**跑一遍这些检查 ——
    「打包期检查过了」不能作为前提，因为一个包从作者手里传出后可以被任何人重新打包，
    而 `verify_bytes` 只做「容器结构 + payload 哈希 + 声明与归档条目一致」，不做白名单判定。
    两条路径共用这里，规则就不会一边改一边漏。
    """
    package = data.get("package")
    if not isinstance(package, dict):
        raise _fail("EINVAL", "PKG.toml is missing the [package] table")
    name = _require_str(package, "name")
    version = _require_str(package, "version")
    category = _require_str(package, "category")
    scope = _require_str(package, "scope")
    if category not in CATEGORIES:
        raise _fail("EINVAL", f"unknown category {category!r}", f"allowed: {', '.join(CATEGORIES)}")
    if scope not in SCOPES:
        raise _fail("EINVAL", f"unknown scope {scope!r}", f"allowed: {', '.join(SCOPES)}")

    abi = data.get("abi") if isinstance(data.get("abi"), dict) else {}
    abi_kernel = abi.get("kernel") if isinstance(abi, dict) else None
    # 域边界（文档 5.2）：内核级内容只能进 system scope
    if category in ("system", "driver") and scope != "system":
        raise _fail("EINVAL", f"category {category!r} must use scope 'system', got {scope!r}")
    if category == "driver" and not abi_kernel:
        raise _fail("EINVAL", "category 'driver' must declare [abi].kernel")

    install = data.get("install") if isinstance(data.get("install"), dict) else {}
    if install.get("hooks"):
        raise _fail("EINVAL", "install hooks are not allowed", "纯文件安装是硬约束（文档 5.3）")

    raw_files = data.get("files")
    if not isinstance(raw_files, list) or not raw_files:
        raise _fail("EINVAL", "PKG.toml must declare at least one [[files]] entry")

    files: list[FileEntry] = []
    seen_targets: set[str] = set()
    for index, item in enumerate(raw_files):
        if not isinstance(item, dict):
            raise _fail("EINVAL", f"[[files]] #{index} must be a table")
        source = _require_str(item, "source", where=f"[[files]] #{index}")
        target = _require_str(item, "target", where=f"[[files]] #{index}")
        mode = item.get("mode", 0o644)
        if not isinstance(mode, int):
            raise _fail("EINVAL", f"[[files]] #{index}: mode must be an integer (e.g. 0o755)")
        _validate_target(target, scope)
        if mode & ~ALLOWED_MODE:
            raise _fail(
                "EINVAL",
                f"[[files]] #{index}: mode {oct(mode)} sets setuid/setgid/sticky bits",
                "只允许 0o600–0o777 的常规权限位（文档 3.1）",
            )
        if target in seen_targets:
            raise _fail("EINVAL", f"[[files]] #{index}: duplicate target {target!r}")
        seen_targets.add(target)
        files.append(FileEntry(source, target, mode))
    return Manifest(name, version, category, scope, abi_kernel, files)


def _require_str(table: dict, key: str, where: str = "[package]") -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise _fail("EINVAL", f"{where}: missing non-empty string {key!r}")
    return value


def _validate_target(target: str, scope: str) -> None:
    """目标路径必须是绝对、规范化、且落在该 scope 的白名单前缀内（文档 3.1 / 8.2）。"""
    if not target.startswith("/"):
        raise _fail("EINVAL", f"target {target!r} must be an absolute path")
    if target.endswith("/") or "//" in target:
        raise _fail("EINVAL", f"target {target!r} must not end with '/' or contain '//'")
    parts = [p for p in target.split("/") if p]
    if any(p in (".", "..") for p in parts):
        raise _fail("EINVAL", f"target {target!r} must not contain '.' or '..'")
    prefixes = SCOPE_TARGETS[scope]
    if not any(target == p or target.startswith(p + "/") for p in prefixes):
        raise _fail(
            "EINVAL",
            f"target {target!r} is outside scope {scope!r}",
            f"allowed prefixes: {', '.join(prefixes)}",
        )


# ---------------------------------------------------------------- payload（确定性 ustar）


def _octal(value: int, width: int) -> bytes:
    body = format(value, "o").rjust(width - 1, "0")
    if len(body) > width - 1:
        raise _fail("EINVAL", f"value {value} does not fit in {width - 1} octal digits")
    return body.encode("ascii") + b"\x00"


def _check_name(name: str) -> None:
    if not name or name.startswith("/") or ".." in name.split("/"):
        raise _fail("EINVAL", f"unsafe archive name {name!r}")
    if len(name.encode("utf-8")) > MAX_NAME:
        raise _fail(
            "ENAMETOOLONG",
            f"archive name {name!r} exceeds {MAX_NAME} bytes",
            "v1 固定 ustar，不用 PAX 扩展头（文档 3.2）",
        )


def tar_entry(name: str, data: bytes, mode: int) -> bytes:
    """单个常规文件条目（512 字节头 + 数据 + 补零）。"""
    _check_name(name)
    header = bytearray(BLOCK)
    header[0:len(name)] = name.encode("utf-8")
    header[100:108] = _octal(mode, 8)
    header[108:116] = _octal(0, 8)   # uid
    header[116:124] = _octal(0, 8)   # gid
    header[124:136] = _octal(len(data), 12)
    header[136:148] = _octal(0, 12)  # mtime
    header[148:156] = b" " * 8       # 校验和先按空格算
    header[156:157] = b"0"           # typeflag = 常规文件
    header[257:263] = b"ustar\x00"   # POSIX ustar magic
    header[263:265] = b"00"          # ustar 版本
    header[265:297] = b"\x00" * 32   # uname
    header[297:329] = b"\x00" * 32   # gname
    checksum = sum(header)
    header[148:156] = format(checksum, "06o").encode("ascii") + b"\x00 "
    padding = b"\x00" * (-len(data) % BLOCK)
    return bytes(header) + data + padding


def build_payload(root: Path, manifest: Manifest) -> bytes:
    """把包定义里声明的文件打成确定性 ustar 流（`PKG.toml` 第一）。"""
    parts = [tar_entry("PKG.toml", (root / "PKG.toml").read_bytes(), 0o644)]
    for entry in sorted(manifest.files, key=lambda e: e.archive_name.encode("utf-8")):
        source = root / entry.source
        if not source.is_file():
            raise _fail("ENOENT", f"[[files]] source {entry.source!r} not found under {root}")
        parts.append(tar_entry(entry.archive_name, source.read_bytes(), entry.mode))
    parts.append(b"\x00" * BLOCK * 2)
    return b"".join(parts)


# ---------------------------------------------------------------- 容器


class Header:
    __slots__ = ("version", "header_len", "flags", "payload_len", "payload_off", "digest", "reserved")

    def __init__(self, version, header_len, flags, payload_len, payload_off, digest, reserved):
        self.version = version
        self.header_len = header_len
        self.flags = flags
        self.payload_len = payload_len
        self.payload_off = payload_off
        self.digest = digest
        self.reserved = reserved

    @property
    def rev(self) -> str:
        return f"sha256:{self.digest}"


def wrap(payload: bytes) -> bytes:
    """给 payload 套上 64 字节的 CKC 头。

    头里的 `payload_sha256` 是 **32 字节原始摘要**（不是 64 个十六进制字符）：
    头长固定 64，字段宽度写死在表里，因此任何“变宽”的表示都必须在这里就被排除。
    """
    header = bytearray(HEADER_LEN)
    header[0:4] = MAGIC
    header[4:6] = HEADER_LEN.to_bytes(2, "little")
    header[6:8] = (0).to_bytes(2, "little")
    header[8:16] = len(payload).to_bytes(8, "little")
    header[16:24] = PAYLOAD_OFF.to_bytes(8, "little")
    header[24:56] = hashlib.sha256(payload).digest()
    header[56:64] = b"\x00" * RESERVED
    assert len(header) == HEADER_LEN, "header must stay exactly HEADER_LEN bytes"
    return bytes(header) + payload


def read_header(data: bytes) -> Header:
    """解析并**校验**头部；任何不一致都抛 `EVERIFY`（退出码 2）。"""
    if len(data) < HEADER_LEN:
        raise _verify_fail(f"file is {len(data)} bytes, shorter than the {HEADER_LEN}-byte header")
    if data[0:4] != MAGIC:
        raise _verify_fail(
            f"bad magic {data[0:4]!r}",
            "只有 CKC v1 容器（后缀 .ckc）能通过校验",
        )
    header_len = int.from_bytes(data[4:6], "little")
    if header_len != HEADER_LEN:
        raise _verify_fail(f"header_len={header_len} != {HEADER_LEN}")
    flags = int.from_bytes(data[6:8], "little")
    if flags != 0:
        raise _verify_fail(f"flags=0x{flags:04x} is not defined in v1", "未知特性位一律拒绝")
    payload_len = int.from_bytes(data[8:16], "little")
    payload_off = int.from_bytes(data[16:24], "little")
    if payload_off != PAYLOAD_OFF:
        raise _verify_fail(f"payload_off={payload_off} != {PAYLOAD_OFF}")
    reserved = data[56:64]
    if reserved != b"\x00" * RESERVED:
        raise _verify_fail(f"reserved bytes are not zero: {reserved!r}")
    if payload_off + payload_len != len(data):
        raise _verify_fail(
            f"payload span {payload_off}+{payload_len} != file size {len(data)}",
            "包被截断或追加了尾部数据",
        )
    digest = data[24:56].hex()  # 32 字节原始摘要 → 64 位小写十六进制
    return Header(VERSION, header_len, flags, payload_len, payload_off, digest, reserved)


class Entry:
    __slots__ = ("name", "size", "mode")

    def __init__(self, name, size, mode):
        self.name = name
        self.size = size
        self.mode = mode


def parse_payload(payload: bytes) -> list:
    """按 ustar 规则解析并**校验** payload（顺序、字段、补零、结尾块）。"""
    entries: list[Entry] = []
    offset = 0
    total = len(payload)
    while True:
        if offset + BLOCK > total:
            raise _verify_fail(f"payload ends mid-block at offset {offset}")
        block = payload[offset:offset + BLOCK]
        if block == b"\x00" * BLOCK:
            if payload[offset:] != b"\x00" * (total - offset):
                raise _verify_fail("payload has data after the end-of-archive marker")
            if total - offset != 2 * BLOCK:
                raise _verify_fail("payload must end with exactly two zero blocks")
            break
        offset += BLOCK

        name = block[0:100].split(b"\x00", 1)[0].decode("utf-8", "replace")
        mode = _read_octal(block[100:108])
        uid = _read_octal(block[108:116])
        gid = _read_octal(block[116:124])
        size = _read_octal(block[124:136])
        mtime = _read_octal(block[136:148])
        typeflag = block[156:157]
        if block[257:263] != b"ustar\x00" or block[263:265] != b"00":
            raise _verify_fail(f"entry {name!r}: not a POSIX ustar header")
        if typeflag not in (b"0", b"\x00"):
            raise _verify_fail(f"entry {name!r}: only regular files are allowed (typeflag={typeflag!r})")
        if block[345:500].strip(b"\x00"):
            raise _verify_fail(f"entry {name!r}: ustar prefix field must be empty")
        if mtime != 0 or uid != 0 or gid != 0:
            raise _verify_fail(f"entry {name!r}: mtime/uid/gid must be 0 for reproducibility")
        if block[265:329].strip(b"\x00"):
            raise _verify_fail(f"entry {name!r}: uname/gname must be empty")
        if mode & ~ALLOWED_MODE:
            raise _verify_fail(f"entry {name!r}: mode {oct(mode)} sets setuid/setgid/sticky bits")
        _check_verify_name(name)
        if _read_octal(block[148:156]) != _checksum(block):
            raise _verify_fail(f"entry {name!r}: header checksum mismatch")
        if offset + size > total:
            raise _verify_fail(f"entry {name!r}: truncated data")
        data_end = offset + size
        padding = payload[data_end:data_end + (-size % BLOCK)]
        if padding.strip(b"\x00"):
            raise _verify_fail(f"entry {name!r}: data padding must be zeros")
        entries.append(Entry(name, size, mode))
        offset = data_end + (-size % BLOCK)

    if not entries:
        raise _verify_fail("payload has no entries")
    if entries[0].name != "PKG.toml":
        raise _verify_fail("the first entry must be PKG.toml", f"got {entries[0].name!r}")
    names = [e.name for e in entries[1:]]
    if names != sorted(names, key=lambda n: n.encode("utf-8")):
        raise _verify_fail("entries after PKG.toml must be sorted byte-wise by name")
    if len(set(names)) != len(names):
        raise _verify_fail("duplicate entry names")
    return entries


def _read_octal(field: bytes) -> int:
    body = field.strip(b"\x00 ").lstrip(b" ")
    if not body:
        return 0
    if not re.fullmatch(rb"[0-7]+", body):
        raise _verify_fail(f"{field!r} is not an octal field")
    return int(body, 8)


def _checksum(block: bytes) -> int:
    return sum(block[:148]) + sum(b" " * 8) + sum(block[156:])


def _check_verify_name(name: str) -> None:
    if not name or name.startswith("/") or ".." in name.split("/"):
        raise _verify_fail(f"unsafe archive name {name!r}")
    if len(name.encode("utf-8")) > MAX_NAME:
        raise _verify_fail(f"archive name {name!r} exceeds {MAX_NAME} bytes")


# ---------------------------------------------------------------- verify / info


class Report:
    __slots__ = ("header", "entries", "name", "version", "category", "scope")

    def __init__(self, header, entries, name, version, category, scope):
        self.header = header
        self.entries = entries
        self.name = name
        self.version = version
        self.category = category
        self.scope = scope


def verify_bytes(data: bytes) -> Report:
    """完整校验一个 `.ckc`：头部 → payload 哈希 → ustar 结构 → 与 PKG.toml 交叉核对。"""
    header = read_header(data)
    payload = data[header.payload_off:]
    actual = hashlib.sha256(payload).hexdigest()
    if actual != header.digest:
        raise _verify_fail(
            f"payload digest mismatch: header={header.digest} actual={actual}",
            "包内容被改动过（或头部哈希被改）",
        )
    entries = parse_payload(payload)
    manifest_raw = _entry_data(payload, entries, "PKG.toml")
    try:
        text = manifest_raw.decode("utf-8")
    except UnicodeDecodeError:
        raise _verify_fail("PKG.toml inside the payload is not valid UTF-8")
    manifest = parse_toml_subset(text)
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise _verify_fail("payload PKG.toml is missing [package]")
    name = _require_str(package, "name")
    version = _require_str(package, "version")
    category = _require_str(package, "category")
    scope = _require_str(package, "scope")
    declared = [f.get("target", "").lstrip("/") for f in manifest.get("files", []) if isinstance(f, dict)]
    archived = [e.name for e in entries if e.name != "PKG.toml"]
    if sorted(declared, key=lambda n: n.encode("utf-8")) != archived:
        raise _verify_fail(
            "payload entries do not match the targets declared in PKG.toml",
            f"declared={sorted(declared)} archived={archived}",
        )
    return Report(header, entries, name, version, category, scope)


def _entry_data(payload: bytes, entries: list, name: str) -> bytes:
    offset = 0
    for entry in entries:
        header = payload[offset:offset + BLOCK]
        if entry.name == name:
            start = offset + BLOCK
            return payload[start:start + entry.size]
        offset += BLOCK + entry.size + (-entry.size % BLOCK)
        del header
    raise _verify_fail(f"entry {name!r} not found")


# ---------------------------------------------------------------- CLI


def cmd_pack(args: argparse.Namespace) -> int:
    root = Path(args.directory)
    if not root.is_dir():
        raise _fail("ENOENT", f"not a directory: {root}")
    manifest = load_manifest(root)
    out = Path(args.out) if args.out else root.parent / f"{manifest.name}-{manifest.version}{SUFFIX}"
    if out.suffix != SUFFIX:
        raise _fail("EINVAL", f"output must end with {SUFFIX!r}", f"got {out.name!r}")
    payload = build_payload(root, manifest)
    data = wrap(payload)
    tmp = out.with_name(out.name + ".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, out)
    digest = hashlib.sha256(payload).hexdigest()
    print(
        f"ckc: pack ok out={out} suffix={SUFFIX} name={manifest.name} version={manifest.version} "
        f"entries={len(manifest.files) + 1} payload={len(payload)} bytes file={len(data)} bytes "
        f"rev=sha256:{digest}"
    )
    if args.print_rev:
        print(f"sha256:{digest}")
    return EXIT_OK


def cmd_verify(args: argparse.Namespace) -> int:
    data = _read_package(args.file)
    report = verify_bytes(data)
    print(
        f"ckc: verify ok file={args.file} magic=CKCv{report.header.version} "
        f"name={report.name} version={report.version} category={report.category} scope={report.scope} "
        f"entries={len(report.entries)} payload={report.header.payload_len} "
        f"bytes rev=sha256:{report.header.digest}"
    )
    return EXIT_OK


def cmd_info(args: argparse.Namespace) -> int:
    data = _read_package(args.file)
    report = verify_bytes(data)
    print(f"ckc: file={args.file}")
    print(f"ckc: magic=CKCv{report.header.version} header_len={report.header.header_len} flags={report.header.flags}")
    print(f"ckc: payload_off={report.header.payload_off} payload_len={report.header.payload_len} reserved=0")
    print(f"ckc: name={report.name} version={report.version} category={report.category} scope={report.scope}")
    print(f"ckc: rev=sha256:{report.header.digest}")
    for entry in report.entries:
        print(f"ckc: entry {entry.name} size={entry.size} mode={oct(entry.mode)}")
    return EXIT_OK


def _read_package(path: str) -> bytes:
    file = Path(path)
    if not file.is_file():
        raise _fail("ENOENT", f"file not found: {file}", exit_code=EXIT_IO)
    if file.suffix != SUFFIX:
        raise _fail(
            "EINVAL",
            f"{file.name!r} does not have the {SUFFIX} suffix",
            "内核包一律使用 .ckc（CKC = Cerlesse Kernel Container）",
        )
    return file.read_bytes()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ckc", description="Cerlesse 内核包容器（.ckc）工具")
    sub = parser.add_subparsers(dest="command", required=True)

    pack = sub.add_parser("pack", help="把一棵带 PKG.toml 的目录树打成 .ckc")
    pack.add_argument("directory")
    pack.add_argument("-o", "--out")
    pack.add_argument("--print-rev", action="store_true", help="额外单独打印 rev（脚本用）")
    pack.set_defaults(func=cmd_pack)

    verify = sub.add_parser("verify", help="校验一个 .ckc（头部 + 哈希 + 结构 + 与 PKG.toml 交叉核对）")
    verify.add_argument("file")
    verify.set_defaults(func=cmd_verify)

    info = sub.add_parser("info", help="打印头部字段与条目清单")
    info.add_argument("file")
    info.set_defaults(func=cmd_info)
    return parser


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except CkcError as err:
        hint = f" ({err.hint})" if err.hint else ""
        print(f"ckc: error {err.errno} {err.message}{hint}", file=sys.stderr)
        return err.exit_code


if __name__ == "__main__":
    sys.exit(main())
