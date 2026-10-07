//! 全局分配器（v0.3 实现）：把内核堆接入 Rust `alloc`
//!
//! 实现 `GlobalAlloc` 后，内核中即可使用 `Box` / `Vec` / `String` 等
//! `alloc` 类型（验收项）。`realloc` / `alloc_zeroed` 使用 trait 默认实现
//! （基于 alloc + 拷贝 / alloc + 清零）。

use core::alloc::{GlobalAlloc, Layout};

use crate::memory::heap;

struct KernelAllocator;

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { heap::alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { heap::dealloc(ptr, layout) }
    }
}
