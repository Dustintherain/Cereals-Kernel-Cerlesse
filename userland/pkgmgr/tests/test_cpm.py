#!/usr/bin/env python3
"""`pkgmgr/cpm.py`（H-06 **P0b** = **CKM-3** 事务引擎）的主机侧验收。

住在 **`userland/pkgmgr/`**（纯内核化阶段五：安装/卸载/事务属于用户态管理服务，
内核只保留权限判定与必要的持久化接口）。不启 QEMU、纯标准库：
`make -C userland test-cpm` 调它（与同目录的 `tests/test_ckc.py` 一起）。
断言的都是**产物事实**而不是措辞 —— 每次断言都能失败，不是走过场：

1. **成功路径**：`add` 之后文件逐字节等于包内内容、回执的 `rev` = `.ckc` 的 payload 哈希
   （内容即身份）、`status` 报 `activated`、`verify` 逐文件通过；
2. **独立复核**：把同一段 payload 交给系统 `tar` 解出来，与 scope 里落地的文件比对（不是自己验自己）；
3. **幂等**：重复 `add`/`apply` 命中 `already active` ⇒ **零写入**（用 `st_mtime_ns` 断言）；
4. **原子失败**：`add --fail-after activate` ⇒ 退出码 3，scope 内文件**逐字节**回到事务前，
   且**没有半成品回执**（回执的 `rev` 仍是上一修订）；
5. **掉电与恢复**：`add --crash-after activate`（硬退出 9、不释放锁）留下未提交事务 ⇒
   `cpm recover` 回滚它（含陈旧锁接管），文件与回执回到事务前；
6. **回退**：装到新修订 → `rollback --offline` → 文件逐字节等于上一修订，`status` 的 rev 回退；
7. **篡改检出**：就地改一个已交付文件 ⇒ `cpm verify` 退出码 2（`EINTEGRITY`），
   同修订重装（自愈）后通过，且**修订历史不被挤掉**；
8. **安装期重新校验**：手工拼一个「容器合法、但 target 越出 scope」的 `.ckc` ⇒
   安装被拒（`EINVAL`）且**零文件写入** —— 证明「打包期检查过了」不是安装期的前提；
9. **不可写路径**：`--scope` 与包声明的 scope 不符 ⇒ 拒绝且零写入；
10. **去重**：两个包共享同一份内容 ⇒ store 只有一份 blob（`gc --dry-run` 列 0 项可回收）；
11. **并发**：锁被活着的进程持有时写命令 ⇒ 退出码 5，而 `status` 仍然可用；
12. **卸载**：配置文件（`/etc/**`）默认保留，`--purge` 才删；
13. **退出码与错误**：后缀错、scope 越域、ABI 不符、未知包名都落在 9.2 规定的档位。
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# ROOT = 包管理器目录（本文件是 `<pkgmgr>/tests/test_cpm.py`）。
ROOT = Path(__file__).resolve().parent.parent
CPM = ROOT / "cpm.py"

sys.path.insert(0, str(ROOT))
import ckc  # noqa: E402  （构造畸形/越界包与解析回执需要同一套底层原语）

CHECKS = 0


def ok(label: str) -> None:
    global CHECKS
    CHECKS += 1
    print(f"cpm-test: {label} PASS")


def fail(label: str, detail: str) -> None:
    print(f"FAIL: {label}: {detail}")
    sys.exit(1)


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(CPM), *args], capture_output=True, text=True, check=False
    )


def expect(label: str, proc: subprocess.CompletedProcess, code: int, needle: str = "") -> None:
    if proc.returncode != code:
        fail(label, f"exit={proc.returncode} (want {code})\n  stdout: {proc.stdout}\n  stderr: {proc.stderr}")
    blob = proc.stdout + proc.stderr
    if needle and needle not in blob:
        fail(label, f"missing {needle!r} in output\n  {blob}")
    ok(label)


# ---------------------------------------------------------------- 构造包


def pkg_toml(name, scope, files, category="userland", abi=None) -> str:
    lines = [
        "[package]",
        f'name = "{name}"',
        'version = "1.0.0"',
        f'category = "{category}"',
        f'scope = "{scope}"',
    ]
    if abi is not None:
        lines += ["[abi]", f"kernel = {abi}"]
    for source, target, mode in files:
        lines += [
            "[[files]]",
            f'source = "{source}"',
            f'target = "{target}"',
            f"mode = 0o{mode:o}",
        ]
    return "\n".join(lines) + "\n"


_TREE_SEQ = [0]


def make_package(base: Path, name: str, files, scope="user", category="userland", abi=None):
    """用 `ckc` 的 P0a 原语打一个真实 `.ckc`；返回 `(bytes, rev)`。"""
    _TREE_SEQ[0] += 1
    tree = base / f"src-{name}-{_TREE_SEQ[0]}"
    for source, _target, _mode in files:
        path = tree / source
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(CONTENT[(name, source)])
    (tree / "PKG.toml").write_text(pkg_toml(name, scope, files, category, abi), encoding="utf-8")
    manifest = ckc.load_manifest(tree)
    payload = ckc.build_payload(tree, manifest)
    return ckc.wrap(payload), f"sha256:{hashlib.sha256(payload).hexdigest()}"


def forged_package(name: str, scope: str, target: str, archive: str) -> bytes:
    """容器结构合法、但**打包期规则本会拒绝**的包（用来测安装期是否重新校验）。

    绕过 `ckc.load_manifest` 直接拼 payload：`verify_bytes` 只做「容器 + 哈希 + 声明与归档一致」，
    因此这种包能过 `verify`，必须由 **cpm 的安装期检查** 拦下来。
    """
    toml = pkg_toml(name, scope, [("bin/hello", target, 0o755)])
    payload = (
        ckc.tar_entry("PKG.toml", toml.encode("utf-8"), 0o644)
        + ckc.tar_entry(archive, b"forged payload\n", 0o755)
        + b"\x00" * 1024
    )
    return ckc.wrap(payload)


#: 每个包的源文件内容（`(name, source)` → bytes）
CONTENT: dict[tuple[str, str], bytes] = {}


def contents(root: Path, scope: str, target: str) -> bytes:
    return (root / "scopes" / scope / target.lstrip("/")).read_bytes()


def mtimes(root: Path) -> dict:
    return {
        str(p): p.stat().st_mtime_ns
        for p in sorted((root / "scopes").rglob("*"))
        if p.is_file()
    }


def journals(root: Path):
    out = []
    for path in sorted((root / "txn").glob("*.jsonl")):
        records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
        out.append((path, records))
    return out


def journal_state(records) -> str:
    ops = {r.get("op") for r in records}
    if "commit" in ops:
        return "committed"
    if "rollback" in ops:
        return "rolled-back"
    return "open"


def receipt_of(root: Path, scope: str, name: str) -> dict:
    path = root / "receipts" / scope / f"{name}.toml"
    return ckc.parse_toml_subset(path.read_text(encoding="utf-8"))


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        root = base / "root"

        # 两个修订的 hello（内容不同 ⇒ rev 不同）与一个共享内容的 extra
        CONTENT[("hello", "bin/hello")] = b"\x7fELF v1\x00" + bytes(range(64))
        CONTENT[("hello", "share/README")] = b"hello v1 readme\n"
        CONTENT[("extra", "share/NOTES")] = CONTENT[("hello", "bin/hello")]
        CONTENT[("cfg", "etc/app.conf")] = b"# default config\nkey = 1\n"

        files_v1 = [("bin/hello", "/home/demo/bin/hello", 0o755), ("share/README", "/usr/share/doc/hello/README", 0o644)]
        hello_v1, rev_v1 = make_package(base, "hello", files_v1)
        CONTENT[("hello2", "bin/hello")] = b"\x7fELF v2\x00" + bytes(range(64))
        CONTENT[("hello2", "share/README")] = b"hello v2 readme\n"
        files_v2 = [("bin/hello", "/home/demo/bin/hello", 0o755), ("share/README", "/usr/share/doc/hello/README", 0o644)]
        # 复用 hello 的名字：v2 通过「同名不同内容、手工指定 rev」区分
        tree_v2 = base / "src-hello-v2"
        for source, _target, _mode in files_v2:
            path = tree_v2 / source
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(CONTENT[("hello2", source)])
        (tree_v2 / "PKG.toml").write_text(pkg_toml("hello", "user", files_v2), encoding="utf-8")
        payload_v2 = ckc.build_payload(tree_v2, ckc.load_manifest(tree_v2))
        hello_v2 = ckc.wrap(payload_v2)
        rev_v2 = f"sha256:{hashlib.sha256(payload_v2).hexdigest()}"

        (base / "hello-v1.ckc").write_bytes(hello_v1)
        (base / "hello-v2.ckc").write_bytes(hello_v2)
        extra_pkg, _extra_rev = make_package(base, "extra", [("share/NOTES", "/usr/share/doc/extra/NOTES", 0o644)])
        (base / "extra.ckc").write_bytes(extra_pkg)
        cfg_pkg, _cfg_rev = make_package(base, "cfg", [("etc/app.conf", "/etc/hello/app.conf", 0o644)], scope="system")
        (base / "cfg.ckc").write_bytes(cfg_pkg)

        if rev_v1 == rev_v2:
            fail("fixtures", "the two revisions must differ")

        # ---- 1. init ----
        expect("init", run("init", "--root", str(root)), 0, "cpm: init ok")
        for sub in ("scopes", "store/sha256", "receipts", "txn", "cache"):
            if not (root / sub).is_dir():
                fail("init skeleton", f"missing {sub}")
        ok("init creates the directory skeleton")

        # ---- 2. 成功路径 ----
        expect("add v1", run("add", "hello", "--root", str(root), "--from", str(base / "hello-v1.ckc"), "--scope", "user"),
               0, "cpm: add ok")
        if contents(root, "user", "/home/demo/bin/hello") != CONTENT[("hello", "bin/hello")]:
            fail("installed content", "bin/hello bytes differ from the package payload")
        if contents(root, "user", "/usr/share/doc/hello/README") != CONTENT[("hello", "share/README")]:
            fail("installed content", "README bytes differ from the package payload")
        ok("installed files match the package payload byte for byte")

        receipt = receipt_of(root, "user", "hello")["receipt"]
        if receipt["rev"] != rev_v1:
            fail("receipt rev", f"{receipt['rev']} != {rev_v1}")
        ok("receipt rev == the .ckc payload hash (content is identity)")

        # ---- 3. 独立复核：系统 tar 读同一段 payload ----
        tar_path = base / "v1.tar"
        tar_path.write_bytes(hello_v1[64:])
        extracted = subprocess.run(
            ["tar", "-xOf", str(tar_path), "home/demo/bin/hello"], capture_output=True, check=False
        )
        if extracted.returncode != 0 or extracted.stdout != CONTENT[("hello", "bin/hello")]:
            fail("independent tar cross-check", "system tar disagrees with the installed bytes")
        ok("independent tar cross-check of the installed bytes")

        # ---- 4. status / verify ----
        status = run("status", "hello", "--root", str(root), "--json")
        if status.returncode != 0:
            fail("status --json", status.stderr)
        row = json.loads(status.stdout)["packages"][0]
        if (row["state"], row["rev"], row["files"]) != ("activated", rev_v1, 2):
            fail("status --json", json.dumps(row))
        ok("status --json reports activated + the right rev + file count")
        expect("verify ok", run("verify", "hello", "--root", str(root)), 0, "cpm: verify checked=1 failed=0")

        # ---- 5. 幂等：重复 add 零写入 ----
        before = mtimes(root)
        expect("add v1 again (idempotent)",
               run("add", "hello", "--root", str(root), "--from", str(base / "hello-v1.ckc"), "--scope", "user"),
               0, "already active")
        if mtimes(root) != before:
            fail("idempotent add", "a repeat add modified the scope")
        ok("repeat add is a no-op with zero writes")

        # ---- 6. 原子失败 ----
        expect("add v2 --fail-after activate",
               run("add", "hello", "--root", str(root), "--from", str(base / "hello-v2.ckc"), "--scope", "user",
                   "--fail-after", "activate"),
               3, "ETXN")
        if contents(root, "user", "/home/demo/bin/hello") != CONTENT[("hello", "bin/hello")]:
            fail("atomic failure", "bin/hello was not restored to v1")
        if contents(root, "user", "/usr/share/doc/hello/README") != CONTENT[("hello", "share/README")]:
            fail("atomic failure", "README was not restored to v1")
        ok("failed txn restores every file to the pre-transaction bytes")
        if receipt_of(root, "user", "hello")["receipt"]["rev"] != rev_v1:
            fail("atomic failure", "a half-written receipt survived")
        ok("failed txn leaves no half-written receipt")
        failed_journal = [r for _p, r in journals(root) if journal_state(r) == "rolled-back"]
        if not failed_journal:
            fail("atomic failure", "no rollback record in the journal")
        ok("failed txn appends a `rollback` record to the journal")

        # ---- 7. 掉电（硬退出）与 recover ----
        crash = run("add", "hello", "--root", str(root), "--from", str(base / "hello-v2.ckc"), "--scope", "user",
                    "--crash-after", "activate")
        if crash.returncode != 9:
            fail("crash-after", f"exit={crash.returncode} (want 9)\n  {crash.stderr}")
        ok("--crash-after activate exits hard (simulated power loss)")
        if contents(root, "user", "/home/demo/bin/hello") != CONTENT[("hello2", "bin/hello")]:
            fail("crash state", "the scope should be left half-applied (v2)")
        ok("crash leaves the scope half-applied (what recover must undo)")
        if not (root / "cpm.lock").is_file():
            fail("crash lock", "a crashed writer should leave its lock behind")
        ok("crash leaves the single-writer lock behind (stale)")
        if not any(journal_state(r) == "open" for _p, r in journals(root)):
            fail("crash journal", "no uncommitted transaction was left behind")

        recovered = run("recover", "--root", str(root))
        expect("recover", recovered, 0, "cpm: recover rolled back")
        if "stale lock" not in recovered.stdout + recovered.stderr:
            fail("recover stale lock", "recover did not report the stale lock")
        ok("recover reclaims the stale lock")
        if (root / "cpm.lock").exists():
            fail("recover lock release", "the lock was not released")
        ok("recover releases the lock it took")
        if contents(root, "user", "/home/demo/bin/hello") != CONTENT[("hello", "bin/hello")]:
            fail("recover", "recover did not restore bin/hello to v1")
        if contents(root, "user", "/usr/share/doc/hello/README") != CONTENT[("hello", "share/README")]:
            fail("recover", "recover did not restore README to v1")
        if receipt_of(root, "user", "hello")["receipt"]["rev"] != rev_v1:
            fail("recover", "the receipt did not return to v1")
        ok("recover restores both files and the receipt to the pre-crash state")
        if any(journal_state(r) == "open" for _p, r in journals(root)):
            fail("recover", "an uncommitted transaction survived recover")
        ok("no uncommitted transaction survives recover")

        # ---- 8. 回退（纯本地） ----
        expect("add v2 for real",
               run("add", "hello", "--root", str(root), "--from", str(base / "hello-v2.ckc"), "--scope", "user"),
               0, "cpm: add ok")
        if receipt_of(root, "user", "hello")["receipt"]["rev"] != rev_v2:
            fail("add v2", "receipt rev did not move to v2")
        expect("rollback --offline",
               run("rollback", "hello", "--root", str(root), "--scope", "user", "--offline"), 0, "cpm: rollback ok")
        if contents(root, "user", "/home/demo/bin/hello") != CONTENT[("hello", "bin/hello")]:
            fail("rollback", "bin/hello is not the previous revision's bytes")
        if contents(root, "user", "/usr/share/doc/hello/README") != CONTENT[("hello", "share/README")]:
            fail("rollback", "README is not the previous revision's bytes")
        ok("rollback restores every file to the previous revision (offline)")
        back = receipt_of(root, "user", "hello")["receipt"]
        if back["rev"] != rev_v1 or back["prev_rev"] != rev_v2:
            fail("rollback receipt", json.dumps(back))
        ok("rollback swaps the receipt: rev=v1, prev_rev=v2 (symmetric)")

        # ---- 9. 篡改检出与自愈 ----
        target = root / "scopes" / "user" / "home" / "demo" / "bin" / "hello"
        target.write_bytes(b"tampered\n")
        expect("verify detects tampering", run("verify", "hello", "--root", str(root)), 2, "EINTEGRITY")
        expect("self-heal reinstall", run("add", "hello", "--root", str(root), "--from", str(base / "hello-v1.ckc"),
                                          "--scope", "user"), 0, "cpm: add ok")
        expect("verify after self-heal", run("verify", "hello", "--root", str(root)), 0, "failed=0")
        healed = receipt_of(root, "user", "hello")["receipt"]
        if healed["rev"] != rev_v1 or healed["prev_rev"] != rev_v2:
            fail("self-heal history", f"same-rev reinstall must not drop the previous revision: {json.dumps(healed)}")
        ok("same-revision self-heal keeps the revision history intact")

        # ---- 10. 安装期重新校验（越界 target，零写入） ----
        forged = forged_package("hello", "user", "/usr/bin/hello", "usr/bin/hello")
        (base / "forged.ckc").write_bytes(forged)
        # 先确认它确实能通过容器校验（否则测的就不是安装期检查了）
        try:
            ckc.verify_bytes(forged)
        except ckc.CkcError as err:
            fail("forged fixture", f"the container must verify; got {err.message}")
        ok("forged .ckc passes the container check (structure + payload hash)")
        before_files = mtimes(root)
        expect("install rejects an out-of-scope target",
               run("add", "hello", "--root", str(root), "--from", str(base / "forged.ckc"), "--scope", "user"),
               1, "outside scope")
        if mtimes(root) != before_files:
            fail("install re-validation", "a rejected package wrote to the scope")
        ok("install-time re-validation rejects it with zero writes")

        # ---- 11. scope 不符 ----
        expect("scope mismatch rejected",
               run("add", "hello", "--root", str(root), "--from", str(base / "hello-v1.ckc"), "--scope", "system"),
               1, "declares scope")
        if (root / "scopes" / "system").exists():
            fail("scope mismatch", "a system scope directory was created anyway")
        ok("scope mismatch is rejected before any write")

        # ---- 12. ABI 不符 ----
        CONTENT[("abi", "share/x")] = b"abi probe\n"
        abi_pkg, _ = make_package(base, "abi", [("share/x", "/usr/share/doc/abi/x", 0o644)], abi=2)
        (base / "abi.ckc").write_bytes(abi_pkg)
        expect("ABI mismatch rejected",
               run("add", "abi", "--root", str(root), "--from", str(base / "abi.ckc"), "--scope", "user"),
               1, "needs ABI 2")

        # ---- 13. 去重 ----
        expect("add extra",
               run("add", "extra", "--root", str(root), "--from", str(base / "extra.ckc"), "--scope", "user"),
               0, "cpm: add ok")
        referenced = set()
        for path in (root / "receipts").rglob("*.toml"):
            data = ckc.parse_toml_subset(path.read_text(encoding="utf-8"))
            for entry in data.get("files", []):
                referenced.add(str(entry["digest"]).split(":", 1)[1])
        stored = {p.name for p in (root / "store" / "sha256").glob("*") if p.is_file()}
        if stored != referenced:
            fail("dedup", f"store={sorted(stored)} referenced={sorted(referenced)}")
        ok("store holds exactly one blob per unique content (content-addressed dedup)")
        shared = hashlib.sha256(CONTENT[("hello", "bin/hello")]).hexdigest()
        if shared not in stored:
            fail("dedup", "the blob shared by two packages is missing")
        ok("two packages sharing a file share a single store blob")
        gc_dry = run("gc", "--root", str(root), "--dry-run")
        expect("gc dry-run", gc_dry, 0, "blobs=0")
        ok("gc finds no reclaimable blob while both receipts reference them")

        # ---- 14. 并发 ----
        root_lock = root / "cpm.lock"
        root_lock.write_text(f"pid={os.getpid()}\nstarted=x\n", encoding="utf-8")
        expect("concurrent write is refused",
               run("add", "cfg", "--root", str(root), "--from", str(base / "cfg.ckc"), "--scope", "system"),
               5, "ELOCK")
        expect("status works while locked", run("status", "--root", str(root)), 0, "cpm: status")
        root_lock.unlink()
        ok("a live lock blocks writers but not readers")

        # ---- 15. 卸载（配置文件默认保留） ----
        expect("add cfg",
               run("add", "cfg", "--root", str(root), "--from", str(base / "cfg.ckc"), "--scope", "system"),
               0, "cpm: add ok")
        (root / "scopes" / "system" / "etc" / "hello" / "app.conf").write_bytes(b"user edited\n")
        expect("remove keeps the config",
               run("remove", "cfg", "--root", str(root), "--scope", "system"), 0, "kept=1")
        if not (root / "scopes" / "system" / "etc" / "hello" / "app.conf").is_file():
            fail("remove", "the config file was deleted without --purge")
        if (root / "receipts" / "system" / "cfg.toml").exists():
            fail("remove", "the receipt survived removal")
        ok("remove drops the receipt but keeps an edited config file")

        expect("add cfg again",
               run("add", "cfg", "--root", str(root), "--from", str(base / "cfg.ckc"), "--scope", "system"),
               0, "cpm: add ok")
        expect("remove --purge",
               run("remove", "cfg", "--root", str(root), "--scope", "system", "--purge"), 0, "removed=1")
        if (root / "scopes" / "system" / "etc" / "hello" / "app.conf").exists():
            fail("remove --purge", "the config file survived --purge")
        ok("remove --purge deletes the config file")
        if (root / "cpm.lock").exists():
            fail("lock hygiene", "a successful transaction left the lock behind")
        ok("a successful transaction releases the lock")

        # ---- 16. apply 幂等（另一棵 root，走 cpkg.toml 意图清单） ----
        root2 = base / "root2"
        run("init", "--root", str(root2))
        (root2 / "cache" / "pkgs").mkdir(parents=True, exist_ok=True)
        shutil.copyfile(base / "hello-v1.ckc", root2 / "cache" / "pkgs" / "hello-v1.ckc")
        (root2 / "cpkg.toml").write_text(
            '[settings]\nchannel = "local-dev"\nscope = "user"\n\n[[want]]\nname = "hello"\nscope = "user"\n',
            encoding="utf-8",
        )
        expect("apply #1 installs", run("apply", "--root", str(root2)), 0, "status=installed")
        snapshot = mtimes(root2)
        expect("apply #2 skips", run("apply", "--root", str(root2)), 0, "skipped=1")
        if mtimes(root2) != snapshot:
            fail("apply idempotency", "the second apply wrote to the scope")
        ok("apply converges then is a zero-write no-op (idempotent)")
        expect("plan reports skip", run("plan", "--root", str(root2)), 0, "action=skip")
        # 预算（6.3）：`status` 不读 catalog —— 删掉 `catalog/` 后仍然成功（不会退化成「必须联网/读索引」）
        shutil.rmtree(root2 / "catalog", ignore_errors=True)
        expect("status ignores the catalog", run("status", "--root", str(root2)), 0, "cpm: status")

        # ---- 17. gc 真删：已完成日志回收，被引用的 blob 保留 ----
        expect("gc", run("gc", "--root", str(root2)), 0, "cpm: gc ok")
        if list((root2 / "txn").glob("*.jsonl")):
            fail("gc journals", "committed journals were not reclaimed")
        if not list((root2 / "store" / "sha256").glob("*")):
            fail("gc blobs", "gc deleted a blob that a receipt still references")
        ok("gc reclaims finished journals but keeps referenced blobs")

        # ---- 18. 退出码/错误细节 ----
        expect("bad suffix rejected", run("add", "hello", "--root", str(root), "--from", str(base / "v1.tar"),
                                          "--scope", "user"), 1, "does not have the .ckc suffix")
        expect("verify unknown package", run("verify", "nope", "--root", str(root)), 1, "is not installed")
        expect("missing package", run("add", "ghost", "--root", str(root), "--from", str(base / "hello-v1.ckc"),
                                      "--scope", "user"), 1, "no local .ckc provides")
        expect("missing root", run("status", "--root", str(base / "nope")), 4, "root is not initialized")

        # ---- 19. 回执是合法的 TOML 子集（可被同一套解析器读回） ----
        parsed = receipt_of(root, "user", "hello")
        if parsed["receipt"]["name"] != "hello" or len(parsed["files"]) != 2:
            fail("receipt round-trip", json.dumps(parsed["receipt"]))
        ok("the receipt round-trips through the shared TOML-subset parser")

    print(f"cpm-test: all {CHECKS} checks PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
