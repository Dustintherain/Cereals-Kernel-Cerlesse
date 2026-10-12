#!/usr/bin/env python3
"""Cerlesse 用户程序 ELF 检查器（v0.7）。

`make user` 用它做**不启 QEMU** 的脚手架验收：编译链接出来的用户程序必须满足
内核 ELF Loader（`kernel/src/syscall/elf.rs`）与 `user.ld` 约定：

1. `ELF64 / little-endian / ET_EXEC / EM_X86_64`；
2. 链接基址 = 0x0000_0080_0000_0000（= `address_space.rs::USER_VIRT_BASE`）；
3. 每个 `PT_LOAD` **起始地址页对齐**（否则 ELF Loader 会拒绝：同页既 W 又 X）；
4. 没有「可写段与可执行段落在同一页」（W^X 不退化）；
5. 映像终点 ≤ 基址 + 64 KiB（`USER_IMAGE_MAX`）；
6. 入口落在映像范围内；
7. 没有 `SHT_REL/RELA + SHF_ALLOC`（内核不做加载时重定位）。

这些条件在 `kernel/build.rs`（构建期内核时）与运行期装载器里也会各查一次；
本脚本的存在是为了能**单独**验证脚手架，不用等内核编译或 QEMU 启动。
"""

import struct
import sys
from pathlib import Path

USER_VIRT_BASE = 0x0000_0080_0000_0000
USER_IMAGE_MAX = 64 * 1024
PAGE = 4096
PT_LOAD = 1
PF_X, PF_W, PF_R = 0x1, 0x2, 0x4


def fail(msg: str) -> "None":
    print(f"FAIL: {msg}")
    sys.exit(1)


def perms(flags: int) -> str:
    return "".join(c for c, bit in (("R", PF_R), ("W", PF_W), ("X", PF_X)) if flags & bit)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: check_user_elf.py <path-to-user-elf>")
        return 2
    path = Path(sys.argv[1])
    if not path.is_file():
        fail(f"user ELF not found: {path} (run `make user`)")
    data = path.read_bytes()

    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        fail("not an ELF64 little-endian file")

    e_type, e_machine = struct.unpack_from("<HH", data, 16)
    if e_type != 2:
        fail(f"not ET_EXEC (e_type={e_type}): 内核不做加载时重定位")
    if e_machine != 62:
        fail(f"not EM_X86_64 (e_machine={e_machine})")

    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phentsize, phnum = struct.unpack_from("<HH", data, 54)

    loads = []
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type, p_flags = struct.unpack_from("<II", data, off)
        p_offset, p_vaddr, _p_paddr, p_filesz, p_memsz = struct.unpack_from(
            "<QQQQQ", data, off + 8
        )
        if p_type != PT_LOAD:
            continue
        loads.append((p_vaddr, p_offset, p_filesz, p_memsz, p_flags))

    if not loads:
        fail("no PT_LOAD segment")

    print(f"user ELF: {path} ({len(data)} bytes, entry={entry:#x})")
    print(f"  PT_LOAD segments: {len(loads)}")
    for vaddr, p_offset, filesz, memsz, p_flags in loads:
        print(
            f"  seg vaddr={vaddr:#x} off={p_offset:#x} "
            f"filesz={filesz:#x} memsz={memsz:#x} {perms(p_flags)}"
        )

    base = min(v[0] for v in loads)
    if base != USER_VIRT_BASE:
        fail(f"link base {base:#x} != USER_VIRT_BASE {USER_VIRT_BASE:#x} (check user.ld)")

    for vaddr, _off, _filesz, memsz, _flags in loads:
        if vaddr % PAGE != 0:
            fail(f"PT_LOAD {vaddr:#x} vaddr not page-aligned (ALIGN(0x1000) missing in user.ld)")
        if memsz == 0:
            fail(f"PT_LOAD {vaddr:#x} has zero memsz")

    end = max(vaddr + memsz for vaddr, _o, _f, memsz, _fl in loads)
    if end > USER_VIRT_BASE + USER_IMAGE_MAX:
        fail(f"image end {end:#x} exceeds user image window ({USER_IMAGE_MAX} bytes)")
    if not (base <= entry < end):
        fail(f"entry {entry:#x} outside image [{base:#x}, {end:#x})")

    # W/X 页重叠：ELF Loader 按页取权限并集，重叠会让 W^X 退化（运行期会拒绝）
    for i, (vaddr, _o, _f, memsz, flags) in enumerate(loads):
        if not flags & PF_W:
            continue
        stop = vaddr + memsz
        for o_vaddr, _o2, _f2, o_memsz, o_flags in loads[i + 1 :]:
            if not o_flags & PF_X:
                continue
            if vaddr < o_vaddr + o_memsz and o_vaddr < stop:
                fail(f"W segment {vaddr:#x} shares a page with X segment {o_vaddr:#x}")

    # 可装载重定位
    shoff, = struct.unpack_from("<Q", data, 40)
    shentsize, shnum = struct.unpack_from("<HH", data, 58)
    if shoff and shnum:
        for i in range(shnum):
            off = shoff + i * shentsize
            sh_type, = struct.unpack_from("<I", data, off + 4)
            sh_flags, = struct.unpack_from("<Q", data, off + 8)
            if sh_type in (4, 9) and sh_flags & 0x2:
                fail("loadable relocations present (SHT_REL/SHT_RELA + SHF_ALLOC)")

    print(
        "OK: 基址 / 段页对齐 / W^X 不重叠 / 映像窗口 / 无加载时重定位 全部满足 "
        "(ELF Loader 按段权限逐页映射)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
