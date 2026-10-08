#!/usr/bin/env python3
"""键盘回显集成测试（v0.4 键盘输入验收）。

流程：
1. 启动 QEMU（OVMF + 磁盘镜像），串口输出到管道，监控台开在 unix socket；
2. 等待串口出现内核就绪标记（默认 ``KBIRQ enabled``）；
3. 通过 QEMU monitor 的 ``sendkey`` 注入真实按键；
4. 断言 IRQ1 回调（`KBIRQ ENTRY` / `KB <字符>` / `KBIRQ EXIT`）出现在串口日志中。

用法：
  python3 tests/test_keyboard.py --disk build/disk-kb.img \\
      --serial-log build/serial-kb.log \\
      --expect "KBIRQ ENTRY" --expect "KB a" --expect "KBIRQ EXIT"
"""

import argparse
import shutil
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "build"
OVMF_CODE = Path("/usr/share/OVMF/OVMF_CODE.fd")
OVMF_VARS_SRC = Path("/usr/share/OVMF/OVMF_VARS.fd")
TIMEOUT_SECONDS = 45
READY_MARKER = b"KBIRQ enabled"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Cerlesse keyboard serial-log test")
    parser.add_argument("--disk", default=str(BUILD / "disk-kb.img"), help="磁盘镜像路径")
    parser.add_argument(
        "--expect",
        action="append",
        default=None,
        help="串口必须出现的字符串（可重复）",
    )
    parser.add_argument(
        "--key",
        action="append",
        default=None,
        help="通过 QEMU monitor 注入的按键名（可重复），默认 a/b/1",
    )
    parser.add_argument("--serial-log", default=None, help="串口日志输出路径")
    parser.add_argument("--timeout", type=int, default=TIMEOUT_SECONDS, help="等待秒数")
    parser.add_argument(
        "--ovmf-vars",
        default=None,
        help="OVMF_VARS 副本路径（并行编排时按场景隔离，默认 build/OVMF_VARS.fd）",
    )
    parser.add_argument(
        "--monitor",
        default=None,
        help="QEMU monitor socket 路径（默认 build/qemu-monitor.sock）",
    )
    return parser.parse_args()


class SerialCapture:
    """在后台线程里持续收集 QEMU 的串口输出。"""

    def __init__(self, stream) -> None:
        self._stream = stream
        self._buf = bytearray()
        self._lock = threading.Lock()
        self._thread = threading.Thread(target=self._pump, daemon=True)
        self._thread.start()

    def _pump(self) -> None:
        while True:
            chunk = self._stream.readline()
            if not chunk:
                return
            with self._lock:
                self._buf.extend(chunk)

    def snapshot(self) -> bytes:
        with self._lock:
            return bytes(self._buf)

    def wait_for(self, marker: bytes, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if marker in self.snapshot():
                return True
            time.sleep(0.2)
        return marker in self.snapshot()


def send_keys(sock_path: Path, keys: list[str]) -> None:
    """连接 QEMU monitor 并逐个注入按键。"""
    last_error: OSError | None = None
    for _ in range(50):  # 等待 monitor socket 就绪
        try:
            conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            conn.connect(str(sock_path))
            break
        except OSError as err:  # monitor 尚未监听
            last_error = err
            time.sleep(0.2)
    else:
        raise RuntimeError(f"cannot connect to QEMU monitor: {last_error}")

    with conn:
        conn.settimeout(5.0)
        try:
            conn.recv(4096)  # monitor 欢迎信息
        except OSError:
            pass
        for key in keys:
            conn.sendall(f"sendkey {key}\n".encode())
            time.sleep(0.2)
        time.sleep(0.5)


def main() -> int:
    args = parse_args()
    disk = Path(args.disk)
    keys = args.key or ["a", "b", "1"]
    expects = args.expect or ["KBIRQ ENTRY", "KB a", "KBIRQ EXIT"]
    serial_log = Path(args.serial_log) if args.serial_log else BUILD / "serial-kb.log"

    if shutil.which("qemu-system-x86_64") is None:
        print("FAIL: missing tool qemu-system-x86_64")
        return 2
    if not disk.is_file():
        print(f"FAIL: disk image not found: {disk} (run `make image` first)")
        return 2

    BUILD.mkdir(parents=True, exist_ok=True)
    vars_copy = Path(args.ovmf_vars) if args.ovmf_vars else BUILD / "OVMF_VARS.fd"
    vars_copy.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(OVMF_VARS_SRC, vars_copy)

    monitor = Path(args.monitor) if args.monitor else BUILD / "qemu-monitor.sock"
    if monitor.exists():
        monitor.unlink()

    cmd = [
        "qemu-system-x86_64",
        "-machine", "q35",
        "-m", "512M",
        "-drive", f"if=pflash,format=raw,readonly=on,file={OVMF_CODE}",
        "-drive", f"if=pflash,format=raw,file={vars_copy}",
        # snapshot=on：磁盘以临时写时复制方式打开，不占用镜像写锁，
        # 多个并行场景可共享同一磁盘镜像（H-04 编排器），且不污染原始镜像。
        "-drive", f"format=raw,file={disk},snapshot=on",
        "-serial", "stdio",
        "-monitor", f"unix:{monitor},server,nowait",
        "-display", "none",
        "-no-reboot",
    ]

    print(f"running: {' '.join(cmd)}")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    capture = SerialCapture(proc.stdout)

    injection_error: str | None = None
    try:
        if capture.wait_for(READY_MARKER, args.timeout):
            send_keys(monitor, keys)
        else:
            injection_error = f"serial never reached {READY_MARKER!r}"
    except Exception as err:  # noqa: BLE001 - 报告为测试失败原因
        injection_error = f"key injection failed: {err}"

    # 给内核一点时间处理并回显
    time.sleep(1.0)

    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=10)

    output = capture.snapshot()
    serial_log.write_bytes(output)

    missing = [exp for exp in expects if exp.encode() not in output]
    if not missing and injection_error is None:
        print(f"PASS: all {len(expects)} expectation(s) found on serial (log: {serial_log})")
        return 0

    if injection_error:
        print(f"FAIL: {injection_error}")
    for exp in missing:
        print(f"FAIL: {exp!r} not found on serial (log: {serial_log})")
    tail = output[-4000:]
    print("--- serial tail ---")
    sys.stdout.buffer.write(tail)
    sys.stdout.buffer.flush()
    print()
    return 1


if __name__ == "__main__":
    sys.exit(main())
