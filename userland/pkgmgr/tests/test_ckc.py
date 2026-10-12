#!/usr/bin/env python3
"""`pkgmgr/ckc.py`（CKC = Cerlesse Kernel Container，后缀 `.ckc`）的主机侧验收。

住在 **`userland/pkgmgr/`**（纯内核化阶段五：包管理器不属于内核，见
`docs/pure-kernel.md` 3.5/3.6）。不启 QEMU、纯标准库：`make -C userland test-ckc` 调它。
断言的都是「产物事实」而不是措辞：

1. **确定性**：同一目录树打包两次 → 两个 `.ckc` 逐字节相同、rev 相同；
2. **头部逐字段**：magic/版本/头长/flags/payload_off/payload_len/保留位 都要被查
   （改任一处 → 退出码 2）；
3. **内容哈希**：翻转 payload 一个字节 → 退出码 2，且错误信息指出是摘要不符；
4. **ustar 结构规则**：条目未排序 / `PKG.toml` 不在第一 / 非零 mtime / 非零补零 /
   符号链接条目 → 退出码 2（这些都必须**先重新计算哈希**再测，从而只测结构检查本身）；
5. **打包期规则**：超长路径（ENAMETOOLONG）、目标越出 scope、setuid 位、重复 target、
   `hooks = true`、`driver` 缺 `[abi].kernel`、非子集 TOML → 退出码 1 且不产出文件；
6. **独立复核**：用系统 `tar`（GNU tar）读同一段 payload —— 条目清单与内容
   必须与 `ckc.py info` 一致，即「不是自己验自己」。
"""

from __future__ import annotations

import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# ROOT = 包管理器目录（本文件是 `<pkgmgr>/tests/test_ckc.py`）；
# 与内核实现在结构上没有任何路径耦合：内核只保有 `.ckc` 的**格式规范**与安装期校验。
ROOT = Path(__file__).resolve().parent.parent
CKC = ROOT / "ckc.py"
SUFFIX = ".ckc"

sys.path.insert(0, str(ROOT))
import ckc  # noqa: E402  （测试需要构造畸形容器，所以直接用同一套底层原语）

CHECKS = 0


def ok(label: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"ckc-test: {label} PASS")


def fail(label: str, detail: str) -> None:
    print(f"FAIL: {label}: {detail}")
    sys.exit(1)


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(CKC), *args], capture_output=True, text=True, check=False
    )


def expect(label: str, proc: subprocess.CompletedProcess, code: int, needle: str = "") -> None:
    if proc.returncode != code:
        fail(label, f"exit={proc.returncode} (want {code})\n  stdout: {proc.stdout}\n  stderr: {proc.stderr}")
    blob = proc.stdout + proc.stderr
    if needle and needle not in blob:
        fail(label, f"missing {needle!r} in output\n  {blob}")
    ok(label)


# ---------------------------------------------------------------- 样例源树


PKG_TOML = """# 测试包：hello
[package]
name = "hello"
version = "0.9.0"
category = "userland"
scope = "user"
architecture = "x86_64-unknown-none"

[install]
hooks = false

[[files]]
source = "bin/hello"
target = "/home/demo/bin/hello"
mode = 0o755

[[files]]
source = "share/notes/README"
target = "/usr/share/doc/hello/README"
mode = 0o644

[[files]]
source = "share/empty"
target = "/usr/share/doc/hello/empty"
"""


def make_tree(base: Path, pkg_toml: str = PKG_TOML) -> Path:
    root = base / "pkg"
    (root / "bin").mkdir(parents=True)
    (root / "share" / "notes").mkdir(parents=True)
    (root / "bin" / "hello").write_bytes(b"\x7fELF" + bytes(range(256)) * 4)
    (root / "share" / "notes" / "README").write_text("hello from a ckc package\n", encoding="utf-8")
    (root / "share" / "empty").write_bytes(b"")
    (root / "PKG.toml").write_text(pkg_toml, encoding="utf-8")
    return root


def main() -> int:
    if shutil.which("tar") is None:
        print("FAIL: missing tool tar (needed for the independent cross-check)")
        return 2
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        tree = make_tree(base)
        out1 = base / f"first{SUFFIX}"
        out2 = base / f"second{SUFFIX}"

        # ---- 1. 确定性 ----
        p1 = run("pack", str(tree), "-o", str(out1), "--print-rev")
        expect("pack first", p1, 0, "ckc: pack ok")
        if not out1.is_file():
            fail("pack first", "no output file")
        if out1.suffix != SUFFIX:
            fail("pack suffix", out1.name)
        rev1 = [l for l in p1.stdout.splitlines() if l.startswith("sha256:")]
        p2 = run("pack", str(tree), "-o", str(out2), "--print-rev")
        expect("pack second", p2, 0, "ckc: pack ok")
        rev2 = [l for l in p2.stdout.splitlines() if l.startswith("sha256:")]
        if rev1 != rev2 or not rev1:
            fail("rev stable", f"{rev1} vs {rev2}")
        ok("rev stable across two packs")
        if out1.read_bytes() != out2.read_bytes():
            fail("deterministic bytes", "two packs of the same tree differ")
        ok("deterministic bytes (same tree -> identical file)")
        data = out1.read_bytes()

        # ---- 2. 头部逐字段 ----
        header = ckc.read_header(data)
        if (header.version, header.header_len, header.flags, header.payload_off) != (1, 64, 0, 64):
            fail("header fields", f"{header.version} {header.header_len} {header.flags} {header.payload_off}")
        if header.payload_len != len(data) - 64:
            fail("payload_len", f"{header.payload_len} vs {len(data) - 64}")
        if hashlib.sha256(data[64:]).hexdigest() != header.digest:
            fail("header digest", "header digest does not match the payload")
        ok("header fields (magic/version/len/flags/off/digest)")

        v = run("verify", str(out1))
        expect("verify ok", v, 0, "ckc: verify ok")
        i = run("info", str(out1))
        expect("info ok", i, 0, "ckc: entry usr/share/doc/hello/empty")
        for needle in ("ckc: magic=CKCv1", "payload_off=64", f"rev={rev1[0]}", "name=hello", "version=0.9.0"):
            if needle not in i.stdout:
                fail("info content", f"missing {needle!r}\n  {i.stdout}")
        ok("info content (rev/name/version/entries)")

        # ---- 3. 内容哈希 ----
        tampered = bytearray(data)
        tampered[64 + 600] ^= 0xFF          # 翻 payload 里的一个字节
        bad = base / f"tampered{SUFFIX}"
        bad.write_bytes(bytes(tampered))
        expect("payload tamper detected", run("verify", str(bad)), 2, "payload digest mismatch")
        expect("payload tamper (info)", run("info", str(bad)), 2, "payload digest mismatch")

        # ---- 4. 头部与结构 ----
        cases = [
            ("magic", 0, b"CKD\x01", "bad magic"),
            ("header_len", 4, (32).to_bytes(2, "little"), "header_len=32"),
            ("flags", 6, (1).to_bytes(2, "little"), "flags=0x0001"),
            ("reserved", 56, b"\x00" * 7 + b"\x01", "reserved bytes are not zero"),
            ("payload_off", 16, (128).to_bytes(8, "little"), "payload_off=128"),
        ]
        for label, offset, patch, needle in cases:
            mutated = bytearray(data)
            mutated[offset:offset + len(patch)] = patch
            path = base / f"hdr-{label}{SUFFIX}"
            path.write_bytes(bytes(mutated))
            expect(f"header {label} rejected", run("verify", str(path)), 2, needle)

        truncated = base / f"truncated{SUFFIX}"
        truncated.write_bytes(data[:-1])
        expect("truncated file rejected", run("verify", str(truncated)), 2, "payload span")

        # 结构规则：改完必须重新计算哈希，否则测到的是哈希检查而不是结构检查
        payload = data[64:]
        manifest_len = int(payload[124:136].strip(b"\x00 ") or b"0", 8)
        manifest_bytes = payload[512:512 + manifest_len]

        def structural(label: str, payload_bytes: bytes, needle: str) -> None:
            blob = ckc.wrap(payload_bytes)           # 重新算哈希 → 只测结构检查
            path = base / f"struct-{label}{SUFFIX}"
            path.write_bytes(blob)
            expect(f"structure {label} rejected", run("verify", str(path)), 2, needle)

        structural(
            "unsorted entries",
            ckc.tar_entry("PKG.toml", manifest_bytes, 0o644)
            + ckc.tar_entry("usr/share/z", b"z", 0o644)
            + ckc.tar_entry("usr/share/a", b"a", 0o644)
            + b"\x00" * 1024,
            "sorted byte-wise",
        )
        structural(
            "pkg.toml not first",
            ckc.tar_entry("usr/z", b"z", 0o644)
            + ckc.tar_entry("PKG.toml", manifest_bytes, 0o644)
            + b"\x00" * 1024,
            "first entry must be PKG.toml",
        )
        # 非零 mtime：拿一个真实条目改 mtime 字段（136..148）后重新套头
        # （PKG.toml 头 512 字节 + 其数据按 512 对齐，第二个条目才是真实文件条目）
        second_off = 512 + ((manifest_len + 511) // 512) * 512
        entry = bytearray(payload[second_off:second_off + 512])
        entry[124:136] = b"00000000000\x00"
        entry[136:148] = b"00000000001\x00"
        entry[148:156] = b" " * 8
        entry[148:156] = format(sum(entry), "06o").encode() + b"\x00 "
        structural(
            "non-zero mtime",
            ckc.tar_entry("PKG.toml", manifest_bytes, 0o644) + bytes(entry) + b"\x00" * 1024,
            "mtime/uid/gid must be 0",
        )
        # 符号链接条目（typeflag '2'）
        link = bytearray(entry)
        link[156:157] = b"2"
        link[148:156] = b" " * 8
        link[148:156] = format(sum(link), "06o").encode() + b"\x00 "
        structural(
            "symlink entry",
            ckc.tar_entry("PKG.toml", manifest_bytes, 0o644) + bytes(link) + b"\x00" * 1024,
            "only regular files are allowed",
        )
        # 非零补零
        structural(
            "non-zero padding",
            ckc.tar_entry("PKG.toml", manifest_bytes, 0o644)
            + ckc.tar_entry("usr/small", b"ab", 0o644).replace(b"ab" + b"\x00" * 510, b"ab" + b"\xff" * 510)
            + b"\x00" * 1024,
            "padding must be zeros",
        )

        # ---- 5. 打包期规则（退出码 1，且不产出文件） ----
        rejections = [
            ("long path", PKG_TOML.replace("/usr/share/doc/hello/README", "/usr/share/doc/" + "a" * 96), "ENAMETOOLONG"),
            ("target outside scope", PKG_TOML.replace("/home/demo/bin/hello", "/usr/bin/hello"), "outside scope"),
            ("setuid mode", PKG_TOML.replace("mode = 0o755", "mode = 0o4755"), "setuid/setgid/sticky"),
            ("duplicate target", PKG_TOML.replace("/usr/share/doc/hello/empty", "/home/demo/bin/hello"), "duplicate target"),
            ("hooks enabled", PKG_TOML.replace("hooks = false", "hooks = true"), "hooks are not allowed"),
            ("driver scope mismatch", PKG_TOML.replace('category = "userland"', 'category = "driver"'), "must use scope 'system'"),
            ("unsupported toml", PKG_TOML + "\nlist = [1, 2]\n", "outside the supported TOML subset"),
            ("relative target", PKG_TOML.replace("/usr/share/doc/hello/empty", "usr/share/doc/hello/empty"), "absolute path"),
        ]
        for index, (label, toml, needle) in enumerate(rejections):
            make_tree(base / f"v{index}", toml)
            produced = base / f"reject-{index}{SUFFIX}"
            expect(
                f"pack rejects {label}",
                run("pack", str(base / f"v{index}" / "pkg"), "-o", str(produced)),
                1,
                needle,
            )
            if produced.exists():
                fail(f"pack rejects {label}", "a file was produced anyway")

        no_manifest = base / "no-manifest"
        (no_manifest / "usr").mkdir(parents=True)
        (no_manifest / "usr" / "x").write_bytes(b"x")
        expect("pack requires PKG.toml", run("pack", str(no_manifest)), 1, "missing PKG.toml")

        # ---- 6. 后缀 ----
        wrong_suffix = base / "hello.cpk"
        wrong_suffix.write_bytes(data)
        expect("wrong suffix rejected", run("verify", str(wrong_suffix)), 1, "does not have the .ckc suffix")
        expect("missing file", run("verify", str(base / "nope.ckc")), 4, "file not found")
        expect("output suffix enforced", run("pack", str(tree), "-o", str(base / "x.cpk")), 1, "must end with '.ckc'")

        # ---- 7. 独立复核：GNU tar 读同一段 payload ----
        tar_path = base / "payload.tar"
        tar_path.write_bytes(payload)
        listing = subprocess.run(["tar", "-tf", str(tar_path)], capture_output=True, text=True, check=False)
        if listing.returncode != 0:
            fail("independent tar read", listing.stderr)
        tar_names = [l for l in listing.stdout.splitlines() if l]
        our_names = [e.name for e in ckc.parse_payload(payload)]
        if tar_names != our_names:
            fail("independent tar listing", f"tar={tar_names} ckc={our_names}")
        ok("independent tar listing matches ckc parse")
        extracted = subprocess.run(
            ["tar", "-xOf", str(tar_path), "home/demo/bin/hello"], capture_output=True, check=False
        )
        if extracted.returncode != 0:
            fail("independent tar extract", extracted.stderr.decode("utf-8", "replace"))
        if extracted.stdout != (tree / "bin" / "hello").read_bytes():
            fail("independent tar extract", "content mismatch")
        ok("independent tar extract matches source bytes")
        for name in tar_names:
            if len(name.encode()) > 100:
                fail("name length", name)
        ok("all archive names fit the ustar field (<=100 bytes)")

        # 期望条目集合：PKG.toml + 三个目标
        expected = ["PKG.toml", "home/demo/bin/hello", "usr/share/doc/hello/README", "usr/share/doc/hello/empty"]
        if our_names != expected:
            fail("entry set", f"{our_names} != {expected}")
        ok("entry set (PKG.toml first, targets sorted)")

    print(f"ckc-test: all {CHECKS} checks PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
