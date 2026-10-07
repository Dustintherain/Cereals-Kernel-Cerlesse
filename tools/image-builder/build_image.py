#!/usr/bin/env python3
"""Cerlesse 磁盘镜像构建脚本（v0.1 实现）。

把 UEFI bootloader（boot.efi → \\EFI\\BOOT\\BOOTX64.EFI）与内核 ELF
（kernel → \\KERNEL.ELF）组装为 QEMU/OVMF 可启动的 FAT 磁盘镜像。

依赖：mtools（mformat / mmd / mcopy），无需 root 挂载。
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

BLOCK_SIZE = 1024 * 1024  # 64 MiB 镜像
IMAGE_BLOCKS = 64


def run(cmd: list[str]) -> None:
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write(f"command failed: {' '.join(cmd)}\n")
        sys.stderr.write(result.stderr)
        raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description="build Cerlesse bootable FAT image")
    parser.add_argument("--boot", required=True, help="path to boot.efi")
    parser.add_argument("--kernel", required=True, help="path to kernel ELF")
    parser.add_argument("--out", required=True, help="output disk image path")
    args = parser.parse_args()

    boot = Path(args.boot)
    kernel = Path(args.kernel)
    out = Path(args.out)

    for tool in ("mformat", "mmd", "mcopy"):
        if shutil.which(tool) is None:
            raise SystemExit(f"missing tool: {tool} (install mtools)")

    if not boot.is_file():
        raise SystemExit(f"bootloader not found: {boot} (run `make boot` first)")
    if not kernel.is_file():
        raise SystemExit(f"kernel not found: {kernel} (run `make kernel` first)")

    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("wb") as f:
        f.write(b"\x00" * (BLOCK_SIZE * IMAGE_BLOCKS))

    img = str(out)
    run(["mformat", "-i", img, "::"])
    run(["mmd", "-i", img, "::/EFI"])
    run(["mmd", "-i", img, "::/EFI/BOOT"])
    run(["mcopy", "-i", img, str(boot), "::/EFI/BOOT/BOOTX64.EFI"])
    run(["mcopy", "-i", img, str(kernel), "::/KERNEL.ELF"])

    print(f"image ready: {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
