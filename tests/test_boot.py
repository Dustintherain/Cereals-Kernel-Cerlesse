#!/usr/bin/env python3
"""QEMU 启动集成测试（v0.1 实现，v0.2 支持多断言与异常注入镜像）。

启动 QEMU（OVMF + 磁盘镜像），收集串口输出，断言所有 --expect 字符串出现。
超时后杀掉 QEMU，以断言结果决定成败。

用法：
  python3 tests/test_boot.py                          # 默认：disk.img，断言 "Kernel started!"
  python3 tests/test_boot.py --disk build/disk-exception.img \
      --expect "Kernel started!" --expect "EXCEPTION: divide error"
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "build"
OVMF_CODE = Path("/usr/share/OVMF/OVMF_CODE.fd")
OVMF_VARS_SRC = Path("/usr/share/OVMF/OVMF_VARS.fd")
TIMEOUT_SECONDS = 40


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Cerlesse QEMU serial-log test")
    parser.add_argument("--disk", default=str(BUILD / "disk.img"), help="磁盘镜像路径")
    parser.add_argument(
        "--expect",
        action="append",
        default=None,
        help="串口必须出现的字符串（可重复）；默认断言 'Kernel started!'",
    )
    parser.add_argument("--serial-log", default=None, help="串口日志输出路径")
    parser.add_argument(
        "--ovmf-vars",
        default=None,
        help="OVMF_VARS 副本路径（并行编排时按场景隔离，默认 build/OVMF_VARS.fd）",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=TIMEOUT_SECONDS,
        help="QEMU 观察窗口秒数（默认 40）",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    disk = Path(args.disk)
    expects = [s.encode() for s in (args.expect or ["Kernel started!"])]
    serial_log = Path(args.serial_log) if args.serial_log else BUILD / "serial.log"
    timeout = args.timeout

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
        "-display", "none",
        "-no-reboot",
    ]

    print(f"running: {' '.join(cmd)}")
    timed_out = False
    try:
        result = subprocess.run(
            cmd, capture_output=True, timeout=timeout, check=False
        )
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired as expired:
        timed_out = True
        output = (expired.stdout or b"") + (expired.stderr or b"")

    serial_log.write_bytes(output)

    missing = [exp for exp in expects if exp not in output]
    if not missing:
        print(f"PASS: all {len(expects)} expectation(s) found on serial (log: {serial_log})")
        return 0

    for exp in missing:
        print(f"FAIL: {exp!r} not found on serial (log: {serial_log})")
    if timed_out:
        print(f"  (qemu was killed after {timeout}s)")
    tail = output[-4000:]
    print("--- serial tail ---")
    sys.stdout.buffer.write(tail)
    sys.stdout.buffer.flush()
    print()
    return 1


if __name__ == "__main__":
    sys.exit(main())
