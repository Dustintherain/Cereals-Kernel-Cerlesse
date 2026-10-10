/* Cerlesse 内核链接脚本（v0.1；v0.3 追加 __kernel_start/__kernel_end）
 * 内核加载到 32MiB：QEMU/OVMF 实测低地址 0x900000-0x1500000 被
 * 固件 BootServicesData 占用，空闲常规内存从 0x1500000 开始。
 * 实际加载由 boot/ 依据 ELF 程序头完成（AllocateAddress）。
 * v0.3：导出内核映像范围，帧分配器据此保留内核帧，内核堆紧随其后。
 */
ENTRY(_start)

SECTIONS
{
    . = 32M;
    __kernel_start = .;

    .text : {
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss : {
        *(COMMON)
        *(.bss .bss.*)
    }

    . = ALIGN(4096);
    __kernel_end = .;

    . = ALIGN(4096);
    __user_entry_anchor = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
    }
}
