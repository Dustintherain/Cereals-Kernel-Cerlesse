//! 就绪队列（v0.5）
//!
//! 定长环形队列，保存**就绪但未运行**的任务表下标。
//! 不使用堆分配，便于在中断上下文（`on_tick` → `schedule`）里安全操作。
//!
//! 不变式：队列中不会出现重复下标（`push_back` 前由调用方保证）。

#![allow(dead_code)]

/// 队列容量（任务表下标，见 `scheduler::MAX_TASKS`）。
pub const QUEUE_CAP: usize = 8;

/// 定长环形就绪队列。
#[derive(Clone, Copy)]
pub struct ReadyQueue {
    items: [usize; QUEUE_CAP],
    head: usize,
    len: usize,
}

impl ReadyQueue {
    pub const fn new() -> Self {
        Self {
            items: [0; QUEUE_CAP],
            head: 0,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 是否已在队列中（防止重复入队）。
    pub fn contains(&self, id: usize) -> bool {
        let mut i = 0;
        while i < self.len {
            if self.items[(self.head + i) % QUEUE_CAP] == id {
                return true;
            }
            i += 1;
        }
        false
    }

    /// 入队到队尾；队列满时返回 `false`。
    pub fn push_back(&mut self, id: usize) -> bool {
        if self.len >= QUEUE_CAP {
            return false;
        }
        self.items[(self.head + self.len) % QUEUE_CAP] = id;
        self.len += 1;
        true
    }

    /// 从队首取一个任务；空队列返回 `None`。
    pub fn pop_front(&mut self) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let id = self.items[self.head];
        self.head = (self.head + 1) % QUEUE_CAP;
        self.len -= 1;
        Some(id)
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}
