//! 最小 UEFI FFI 定义（v0.1，x86_64）
//!
//! 只声明 bootloader 用到的协议与 Boot Services 条目；
//! 所有函数指针按 `repr(C)` 精确排布（UEFI 表布局）。

#![allow(non_camel_case_types)]

use core::ffi::c_void;

pub type EfiStatus = usize;
pub type EfiHandle = *mut c_void;

pub const EFI_ERROR_BIT: usize = 1usize << (usize::BITS - 1);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EfiGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// EFI_SIMPLE_FILE_SYSTEM_PROTOCOL GUID
pub const EFI_SIMPLE_FILE_SYSTEM_GUID: EfiGuid = EfiGuid {
    data1: 0x964E_5B22,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

/// EFI_FILE_MODE_READ
pub const FILE_MODE_READ: u64 = 0x1;

/// EFI_LOADED_IMAGE_PROTOCOL GUID
pub const EFI_LOADED_IMAGE_GUID: EfiGuid = EfiGuid {
    data1: 0x5B1B_31A1,
    data2: 0x9562,
    data3: 0x11D2,
    data4: [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

/// EFI_LOADED_IMAGE_PROTOCOL（只取 image_base/image_size）
#[repr(C)]
pub struct EfiLoadedImage {
    pub revision: u64,
    pub parent_handle: EfiHandle,
    pub system_table: *mut EfiSystemTable,
    pub device_handle: EfiHandle,
    pub file_path: *mut c_void,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: *mut c_void,
}

/// ELF PT_LOAD
pub const PT_LOAD: u32 = 1;

#[repr(C)]
pub struct EfiTableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct EfiSystemTable {
    pub hdr: EfiTableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: EfiHandle,
    pub con_in: *mut c_void,
    pub console_out_handle: EfiHandle,
    pub con_out: *mut c_void,
    pub standard_error_handle: EfiHandle,
    pub std_err: *mut c_void,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut EfiBootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

/// Boot Services 表（44 个条目，UEFI 2.x 布局）。
/// 仅使用到的条目为强类型函数指针，其余用 usize 占位保证偏移正确。
#[repr(C)]
pub struct EfiBootServices {
    pub hdr: EfiTableHeader, // 0..24
    pub raise_tpl: usize,    // 0
    pub restore_tpl: usize,  // 1
    pub allocate_pages: unsafe extern "efiapi" fn(
        alloc_type: u32,
        mem_type: u32,
        pages: usize,
        memory: *mut u64,
    ) -> EfiStatus, // 2
    pub free_pages: usize,   // 3
    pub get_memory_map: unsafe extern "efiapi" fn(
        map_size: *mut usize,
        map_buffer: *mut u8,
        map_key: *mut usize,
        desc_size: *mut usize,
        desc_version: *mut u32,
    ) -> EfiStatus, // 4
    pub allocate_pool: usize, // 5
    pub free_pool: usize,     // 6
    pub create_event: usize,  // 7
    pub set_timer: usize,     // 8
    pub wait_for_event: usize, // 9
    pub signal_event: usize,   // 10
    pub close_event: usize,    // 11
    pub check_event: usize,    // 12
    pub install_protocol_interface: usize,    // 13
    pub reinstall_protocol_interface: usize,  // 14
    pub uninstall_protocol_interface: usize,  // 15
    pub handle_protocol: unsafe extern "efiapi" fn(
        handle: EfiHandle,
        protocol: *const EfiGuid,
        interface: *mut *mut c_void,
    ) -> EfiStatus, // 16
    pub reserved: usize,                      // 17
    pub register_protocol_notify: usize,      // 18
    pub locate_handle: usize,                 // 19
    pub locate_device_path: usize,            // 20
    pub install_configuration_table: usize,   // 21
    pub load_image: usize,                    // 22
    pub start_image: usize,                   // 23
    pub exit_image: usize,                    // 24
    pub unload_image: usize,                  // 25
    pub exit_boot_services:
        unsafe extern "efiapi" fn(image_handle: EfiHandle, map_key: usize) -> EfiStatus, // 26
    pub get_next_monotonic_count: usize, // 27
    pub stall: usize,                    // 28
    pub set_watchdog_timer: usize,       // 29
    pub connect_controller: usize,       // 30
    pub disconnect_controller: usize,    // 31
    pub open_protocol: usize,            // 32
    pub close_protocol: usize,           // 33
    pub open_protocol_information: usize, // 34
    pub protocols_per_handle: usize,     // 35
    pub locate_handle_buffer: usize,     // 36
    pub locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const EfiGuid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> EfiStatus,                            // 37
    pub install_multiple_protocol_interfaces: usize,   // 38
    pub uninstall_multiple_protocol_interfaces: usize, // 39
    pub calculate_crc32: usize,                        // 40
    pub copy_mem: usize,                               // 41
    pub set_mem: usize,                                // 42
    pub create_event_ex: usize,                        // 43
}

/// EFI_SIMPLE_FILE_SYSTEM_PROTOCOL
#[repr(C)]
pub struct EfiSimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(
        this: *mut EfiSimpleFileSystem,
        root: *mut *mut EfiFileProtocol,
    ) -> EfiStatus,
}

/// EFI_FILE_PROTOCOL（前 7 个条目强类型，其余占位）
#[repr(C)]
pub struct EfiFileProtocol {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(
        this: *mut EfiFileProtocol,
        new_handle: *mut *mut EfiFileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
    ) -> EfiStatus,
    pub close: unsafe extern "efiapi" fn(this: *mut EfiFileProtocol) -> EfiStatus,
    pub delete: usize,
    pub read: unsafe extern "efiapi" fn(
        this: *mut EfiFileProtocol,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> EfiStatus,
    pub write: usize,
    pub get_position: unsafe extern "efiapi" fn(
        this: *mut EfiFileProtocol,
        position: *mut u64,
    ) -> EfiStatus,
    pub set_position: unsafe extern "efiapi" fn(this: *mut EfiFileProtocol, position: u64)
        -> EfiStatus,
    pub get_info: usize,
    pub set_info: usize,
    pub flush: usize,
    pub open_ex: usize,
    pub read_ex: usize,
    pub write_ex: usize,
    pub flush_ex: usize,
}

/// AllocateAddress（按指定物理地址分配页）
pub const ALLOCATE_ADDRESS: u32 = 2;
/// EfiLoaderData
pub const LOADER_DATA: u32 = 2;
