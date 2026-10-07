//! Cerlesse Bootloader（v0.1 实现）
//!
//! 流程：串口初始化 → 打开 FAT 卷上的 `\KERNEL.ELF` → 解析 ELF 程序头
//! → AllocateAddress 装载到链接地址（32MiB）→ GetMemoryMap → ExitBootServices
//! → 以 RDI = &BootInfo 跳转内核入口。

#![no_std]
#![no_main]

mod serial;
mod uefi;

use core::ffi::c_void;
use shared::BootInfo;
use uefi::*;

/// `\KERNEL.ELF`（UTF-16）
const KERNEL_PATH: [u16; 12] = [
    0x5C, // '\'
    b'K' as u16,
    b'E' as u16,
    b'R' as u16,
    b'N' as u16,
    b'E' as u16,
    b'L' as u16,
    b'.' as u16,
    b'E' as u16,
    b'L' as u16,
    b'F' as u16,
    0,
];

const PAGE_SIZE: u64 = 4096;
const MAX_PHDRS: usize = 16;

static mut BOOT_INFO: BootInfo = BootInfo {
    magic: 0,
    memory_map: core::ptr::null(),
    memory_map_size: 0,
    memory_descriptor_size: 0,
    memory_descriptor_version: 0,
    _reserved: 0,
};

static mut MEMORY_MAP_BUF: [u8; MEMORY_MAP_LEN] = [0; MEMORY_MAP_LEN];

const MEMORY_MAP_LEN: usize = 0x4_0000;

fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

fn fail(msg: &str, status: EfiStatus) -> ! {
    serial::print("error: ");
    serial::print(msg);
    serial::print(" status=");
    serial::print_hex(status as u64);
    serial::println("");
    halt()
}

fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn u64_at(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}

/// 读满 `len` 字节到 `buf`；返回 0 表示成功。
unsafe fn read_exact(file: *mut EfiFileProtocol, buf: *mut u8, len: usize) -> EfiStatus {
    let mut done = 0usize;
    while done < len {
        let mut sz = len - done;
        let st = unsafe { ((*file).read)(file, &mut sz, buf.add(done) as *mut c_void) };
        if st != 0 {
            return st;
        }
        if sz == 0 {
            return EFI_ERROR_BIT | 9; // 提前 EOF
        }
        done += sz;
    }
    0
}

#[export_name = "efi_main"]
pub extern "efiapi" fn efi_main(_image_handle: EfiHandle, st: *mut EfiSystemTable) -> EfiStatus {
    serial::init();
    serial::println("Cerlesse bootloader v0.1");

    if st.is_null() {
        fail("null system table", 0);
    }

    unsafe {
        let bs = (*st).boot_services;
        if bs.is_null() {
            fail("null boot services", 0);
        }

        // 1. 打开简单文件系统协议
        let mut fs_ptr: *mut c_void = core::ptr::null_mut();
        let status = ((*bs).locate_protocol)(
            &EFI_SIMPLE_FILE_SYSTEM_GUID,
            core::ptr::null_mut(),
            &mut fs_ptr,
        );
        if status != 0 {
            fail("locate simple fs", status);
        }
        let fs = fs_ptr as *mut EfiSimpleFileSystem;

        // 2. 打开卷根目录
        let mut root: *mut EfiFileProtocol = core::ptr::null_mut();
        let status = ((*fs).open_volume)(fs, &mut root);
        if status != 0 {
            fail("open volume", status);
        }

        // 3. 打开 \KERNEL.ELF
        let mut kfile: *mut EfiFileProtocol = core::ptr::null_mut();
        let status = ((*root).open)(root, &mut kfile, KERNEL_PATH.as_ptr(), FILE_MODE_READ, 0);
        if status != 0 {
            fail("open \\KERNEL.ELF", status);
        }

        // 4. 文件大小
        let status = ((*kfile).set_position)(kfile, u64::MAX);
        if status != 0 {
            fail("seek end", status);
        }
        let mut ksize: u64 = 0;
        let status = ((*kfile).get_position)(kfile, &mut ksize);
        if status != 0 {
            fail("tell", status);
        }
        serial::print("kernel file size=");
        serial::print_hex(ksize);
        serial::println("");
        if ksize < 64 || ksize > 0x400_0000 {
            fail("bad kernel size", ksize as usize);
        }

        // 5. ELF 头
        let status = ((*kfile).set_position)(kfile, 0);
        if status != 0 {
            fail("seek start", status);
        }
        let mut ehdr = [0u8; 64];
        let status = read_exact(kfile, ehdr.as_mut_ptr(), 64);
        if status != 0 {
            fail("read elf header", status);
        }
        if &ehdr[0..4] != b"\x7fELF" || ehdr[4] != 2 || ehdr[5] != 1 {
            fail("not an ELF64 little-endian file", 0);
        }
        let entry = u64_at(&ehdr, 24);
        let phoff = u64_at(&ehdr, 32);
        let phentsize = u16_at(&ehdr, 54) as usize;
        let phnum = u16_at(&ehdr, 56) as usize;
        if phentsize != 56 || phnum == 0 || phnum > MAX_PHDRS {
            fail("bad program headers", phnum);
        }

        // 6. 程序头
        let mut phbuf = [0u8; MAX_PHDRS * 56];
        let status = ((*kfile).set_position)(kfile, phoff);
        if status != 0 {
            fail("seek phdr", status);
        }
        let status = read_exact(kfile, phbuf.as_mut_ptr(), phnum * 56);
        if status != 0 {
            fail("read phdr", status);
        }

        // 7. 计算装载区间（页对齐）
        // —— 诊断：自身映像位置与低地址内存布局
        let mut li_ptr: *mut c_void = core::ptr::null_mut();
        let status = ((*bs).handle_protocol)(
            _image_handle,
            &EFI_LOADED_IMAGE_GUID,
            &mut li_ptr,
        );
        if status == 0 {
            let li = li_ptr as *mut EfiLoadedImage;
            serial::print("boot.efi base=");
            serial::print_hex((*li).image_base as u64);
            serial::print(" size=");
            serial::print_hex((*li).image_size);
            serial::println("");
        }

        let mut min_addr = u64::MAX;
        let mut max_addr: u64 = 0;
        for i in 0..phnum {
            let p = &phbuf[i * 56..(i + 1) * 56];
            if u32_at(p, 0) != PT_LOAD {
                continue;
            }
            let paddr = u64_at(p, 24);
            let memsz = u64_at(p, 40);
            min_addr = min_addr.min(paddr);
            max_addr = max_addr.max(paddr.saturating_add(memsz));
        }
        if min_addr == u64::MAX || max_addr <= min_addr {
            fail("no PT_LOAD segments", 0);
        }
        let load_start = min_addr & !(PAGE_SIZE - 1);
        let load_end = (max_addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let pages = ((load_end - load_start) / PAGE_SIZE) as usize;

        // —— 诊断：打印低地址（<64MiB）内存区域类型
        {
            let map_buf = core::ptr::addr_of_mut!(MEMORY_MAP_BUF);
            let mut msize = MEMORY_MAP_LEN;
            let mut key = 0usize;
            let mut dsz = 0usize;
            let mut dver = 0u32;
            let st = ((*bs).get_memory_map)(
                &mut msize,
                map_buf.cast::<u8>(),
                &mut key,
                &mut dsz,
                &mut dver,
            );
            if st == 0 && dsz >= 40 {
                let count = msize / dsz;
                serial::print("memmap entries=");
                serial::print_hex(count as u64);
                serial::println("");
                for i in 0..count {
                    let p = map_buf.cast::<u8>().add(i * dsz);
                    let desc = core::slice::from_raw_parts(p, 40);
                    let typ = u32_at(desc, 0);
                    let phys = u64_at(desc, 8);
                    let npages = u64_at(desc, 24);
                    if phys < 0x4000_000 {
                        serial::print("  type=");
                        serial::print_hex(typ as u64);
                        serial::print(" phys=");
                        serial::print_hex(phys);
                        serial::print(" pages=");
                        serial::print_hex(npages);
                        serial::println("");
                    }
                }
            }
        }

        let mut load_base: u64 = load_start;
        let status =
            ((*bs).allocate_pages)(ALLOCATE_ADDRESS, LOADER_DATA, pages, &mut load_base);
        if status != 0 {
            serial::print("allocate kernel pages failed at ");
            serial::print_hex(load_start);
            serial::println("");
            fail("allocate_pages", status);
        }

        // 8. 装载各段并清零 bss
        for i in 0..phnum {
            let p = &phbuf[i * 56..(i + 1) * 56];
            if u32_at(p, 0) != PT_LOAD {
                continue;
            }
            let offset = u64_at(p, 8);
            let paddr = u64_at(p, 24);
            let filesz = u64_at(p, 32) as usize;
            let memsz = u64_at(p, 40) as usize;
            if filesz > 0 {
                let status = ((*kfile).set_position)(kfile, offset);
                if status != 0 {
                    fail("seek segment", status);
                }
                let status = read_exact(kfile, paddr as *mut u8, filesz);
                if status != 0 {
                    fail("read segment", status);
                }
            }
            if memsz > filesz {
                core::ptr::write_bytes(
                    (paddr + filesz as u64) as *mut u8,
                    0,
                    memsz - filesz,
                );
            }
        }

        let _ = ((*kfile).close)(kfile);
        let _ = ((*root).close)(root);

        serial::print("kernel loaded at ");
        serial::print_hex(load_start);
        serial::print(" entry=");
        serial::print_hex(entry);
        serial::println("");

        // 9. 内存映射 + ExitBootServices（键变化时重试）
        let map_buf = core::ptr::addr_of_mut!(MEMORY_MAP_BUF);
        let mut map_size = MEMORY_MAP_LEN;
        let mut map_key: usize = 0;
        let mut desc_size: usize = 0;
        let mut desc_version: u32 = 0;
        let mut exited = false;
        let mut last_status: EfiStatus = 0;
        for _ in 0..8 {
            map_size = MEMORY_MAP_LEN;
            let status = ((*bs).get_memory_map)(
                &mut map_size,
                map_buf.cast::<u8>(),
                &mut map_key,
                &mut desc_size,
                &mut desc_version,
            );
            if status != 0 {
                fail("get_memory_map", status);
            }
            let status = ((*bs).exit_boot_services)(_image_handle, map_key);
            if status == 0 {
                exited = true;
                break;
            }
            last_status = status;
            serial::print("ExitBootServices retry status=");
            serial::print_hex(last_status as u64);
            serial::println("");
        }
        if !exited {
            fail("ExitBootServices", last_status);
        }

        // 10. 填充 BootInfo（此后固件服务不可用，串口直接 port I/O 仍可用）
        let bi = core::ptr::addr_of_mut!(BOOT_INFO);
        (*bi).magic = shared::BOOT_MAGIC;
        (*bi).memory_map = core::ptr::addr_of!(MEMORY_MAP_BUF).cast::<u8>();
        (*bi).memory_map_size = map_size;
        (*bi).memory_descriptor_size = desc_size;
        (*bi).memory_descriptor_version = desc_version;

        serial::print("memory map=");
        serial::print_hex(map_size as u64);
        serial::print(" desc_size=");
        serial::print_hex(desc_size as u64);
        serial::print(" entries=");
        serial::print_hex((map_size / desc_size.max(1)) as u64);
        serial::println("");
        serial::print("bootinfo@=");
        serial::print_hex(bi as u64);
        serial::print(" magic=");
        serial::print_hex((*bi).magic);
        serial::println("");
        serial::println("ExitBootServices OK, jumping to kernel...");

        // 11. 跳转内核（RDI = &BootInfo）
        // 注意：boot crate 编译于 x86_64-unknown-uefi，其 `extern "C"` 是 Win64 ABI（参数走 RCX）；
        // 内核（x86_64-unknown-none）期望 SysV ABI（参数走 RDI），这里必须显式 sysv64。
        let entry_fn: extern "sysv64" fn(*const BootInfo) -> ! =
            core::mem::transmute(entry as usize);
        entry_fn(bi as *const BootInfo);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    serial::println("BOOTLOADER PANIC");
    halt()
}
