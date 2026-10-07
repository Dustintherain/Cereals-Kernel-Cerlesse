//! 虚拟内存基础类型：Page 抽象与地址换算（v0.3 实现）

/// 页大小（4KiB）
pub const PAGE_SIZE: u64 = 4096;
/// 2MiB 大页
pub const PAGE_SIZE_2M: u64 = 2 * 1024 * 1024;
/// 1GiB 大页
pub const PAGE_SIZE_1G: u64 = 1024 * 1024 * 1024;

/// 向上对齐（align 必须是 2 的幂）
pub const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

/// 向下对齐（align 必须是 2 的幂）
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// 4KiB 页（虚拟地址空间中的一页）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Page {
    number: u64,
}

impl Page {
    /// 包含该虚拟地址的页（向下对齐）
    pub const fn containing_address(addr: u64) -> Self {
        Self {
            number: addr / PAGE_SIZE,
        }
    }

    pub const fn from_number(number: u64) -> Self {
        Self { number }
    }

    pub const fn start_address(self) -> u64 {
        self.number * PAGE_SIZE
    }

    /// `count` 页的迭代器（从本页开始）
    pub const fn range(self, count: u64) -> PageRange {
        PageRange {
            next: self.number,
            end: self.number + count,
        }
    }
}

/// 连续页迭代器
pub struct PageRange {
    next: u64,
    end: u64,
}

impl Iterator for PageRange {
    type Item = Page;

    fn next(&mut self) -> Option<Page> {
        if self.next >= self.end {
            return None;
        }
        let page = Page::from_number(self.next);
        self.next += 1;
        Some(page)
    }
}

