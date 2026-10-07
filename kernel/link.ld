/* Cerlesse 内核链接脚本（v0.1）
 * 内核加载到 32MiB：QEMU/OVMF 实测低地址 0x900000-0x1500000 被
 * 固件 BootServicesData 占用，空闲常规内存从 0x1500000 开始。
 * 实际加载由 boot/ 依据 ELF 程序头完成（AllocateAddress）。
 */
ENTRY(_start)

SECTIONS
{
    . = 32M;

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

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
    }
}
