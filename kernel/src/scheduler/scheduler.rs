//! 内核调度器（v0.5 第一版：Round Robin + 时间片抢占）
//!
//! 结构：
//! - 定长任务表 `TASKS`（pid 0 = 启动上下文，其余为内核任务），每个任务有独立内核栈；
//! - 就绪队列（[`ReadyQueue`]）保存「就绪但未运行」的任务下标；
//! - 策略（[`RoundRobin`]）负责时间片记账；
//! - PIT 100Hz → IRQ0 → [`on_tick`] → 时间片用尽则 [`schedule`] →上下文切换。
//!
//! 关键约定：
//! 1. [`on_tick`] 只在 IRQ0 中断上下文（IF=0）执行，绝不重入；
//! 2. 切换发生在中断上下文内，因此 IRQ 的 EOI **必须**在切换前发出
//!    （见 `interrupt::irq::irq_dispatch`），否则 PIC 不再递交 IRQ0，调度停摆；
//! 3. 被抢占的任务在自己的内核栈上保存了完整中断帧，被重新调度时自然继续执行。
//!
//! 实现说明：任务表/队列/策略都是 `static mut`，为避免 `static_mut_refs`
//! 语义陷阱，统一通过 `addr_of_mut!` 取裸指针后再访问，不直接对静态量取引用。

#![allow(dead_code)]

use core::ptr::addr_of_mut;

use super::queue::ReadyQueue;
use super::round_robin::{RoundRobin, DEFAULT_QUANTUM_TICKS};
use crate::driver::serial;
use crate::process::context::{init_context, switch_to};
use crate::process::pid;
use crate::process::thread::{Thread, ThreadState};

/// 任务表容量：pid 0（启动上下文 / 空闲）+ 3 个内核任务。
pub const MAX_TASKS: usize = 4;
/// 每个内核任务的内核栈大小。
pub const STACK_SIZE: usize = 16 * 1024;

/// 内核任务栈（.bss）。
static mut STACKS: [[u8; STACK_SIZE]; MAX_TASKS] = [[0; STACK_SIZE]; MAX_TASKS];
/// 任务表。
static mut TASKS: [Thread; MAX_TASKS] = [Thread::EMPTY; MAX_TASKS];
/// 已登记的任务数。
static mut TASK_COUNT: usize = 0;
/// 当前运行任务在任务表中的下标。
static mut CURRENT: usize = 0;
/// 就绪队列（保存就绪但未运行的任务下标）。
static mut READY: ReadyQueue = ReadyQueue::new();
/// 调度策略。
static mut POLICY: RoundRobin = RoundRobin::new(DEFAULT_QUANTUM_TICKS);
/// 是否已开始调度（`start` 之后）。
static mut STARTED: bool = false;
/// 累计上下文切换次数。
static mut SWITCHES: u64 = 0;

#[inline]
fn tasks_root() -> *mut Thread {
    addr_of_mut!(TASKS) as *mut Thread
}

#[inline]
fn task_ptr(index: usize) -> *mut Thread {
    unsafe { tasks_root().add(index) }
}

#[inline]
fn stacks_root() -> *mut u8 {
    addr_of_mut!(STACKS) as *mut u8
}

#[inline]
fn ready_ptr() -> *mut ReadyQueue {
    addr_of_mut!(READY)
}

#[inline]
fn policy_ptr() -> *mut RoundRobin {
    addr_of_mut!(POLICY)
}

#[inline]
fn count_ptr() -> *mut usize {
    addr_of_mut!(TASK_COUNT)
}

#[inline]
fn current_ptr() -> *mut usize {
    addr_of_mut!(CURRENT)
}

#[inline]
fn started_ptr() -> *mut bool {
    addr_of_mut!(STARTED)
}

#[inline]
fn switches_ptr() -> *mut u64 {
    addr_of_mut!(SWITCHES)
}

/// 调度器统计（验收/自检用）。
#[derive(Clone, Copy)]
pub struct Stats {
    pub tasks: usize,
    pub switches: u64,
    pub started: bool,
}

/// 初始化调度器，并把当前（启动）上下文登记为 pid 0 任务。
///
/// 调用时机：内核基础设施与内存初始化完成之后、[`spawn`] 之前。
pub fn init() {
    unsafe {
        *count_ptr() = 1;
        *current_ptr() = 0;
        *ready_ptr() = ReadyQueue::new();
        *policy_ptr() = RoundRobin::new(DEFAULT_QUANTUM_TICKS);
        *started_ptr() = false;
        *switches_ptr() = 0;
        pid::reset();

        let boot = task_ptr(0);
        *boot = Thread::EMPTY;
        (*boot).pid = pid::alloc(); // 0
        (*boot).name = "kernel_main";
        (*boot).state = ThreadState::Running;
        (*boot).quantum_ticks = (*policy_ptr()).quantum();
    }
}

/// 创建一个内核任务：分配 PID、准备独立内核栈与入口上下文，并加入就绪队列。
///
/// `entry` 由 [`init_context`] 通过切换原语的 `ret` 进入，
/// 因此必须是 `extern "C"` 且不返回（或以 `halt` 收尾）的函数。
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !) -> Option<u64> {
    unsafe {
        let index = *count_ptr();
        if index >= MAX_TASKS {
            return None;
        }

        let stack_top = stacks_root().add(index * STACK_SIZE).add(STACK_SIZE);

        let t = task_ptr(index);
        *t = Thread::EMPTY;
        (*t).pid = pid::alloc();
        (*t).name = name;
        (*t).state = ThreadState::Ready;
        (*t).quantum_ticks = (*policy_ptr()).quantum();
        (*t).stack_top = stack_top as u64;
        init_context(&mut (*t).ctx, stack_top, entry as *const () as usize);

        *count_ptr() = index + 1;
        let _ = (*ready_ptr()).push_back(index);

        let pid = (*t).pid;
        let _ = serial_debug(|| {
            serial::print("sched: spawn pid=");
            serial::print_dec(pid);
            serial::print(" name=");
            serial::println(name);
        });

        Some(pid)
    }
}

/// 开始调度：从启动上下文切出，进入就绪队列中的第一个任务。
///
/// 该函数会在**本轮 Round Robin 绕回 pid 0** 时返回；
/// 因此 `start()` 返回即证明所有任务都被调度过一轮。
pub fn start() {
    unsafe {
        if *started_ptr() || *count_ptr() <= 1 {
            return;
        }
        *started_ptr() = true;
        if let Some(next) = (*ready_ptr()).pop_front() {
            // 启动上下文切出后即变为「就绪」，放回队尾，
            // 这样 Round Robin 会绕回 pid 0（`start()` 也才能返回）。
            let _ = (*ready_ptr()).push_back(0);
            switch(0, next);
        }
    }
}

/// PIT（IRQ0）驱动的调度入口：时间片记账 + 到期轮转。
///
/// # Safety
///
/// 仅在 IRQ0 中断上下文（IF=0）调用。
pub unsafe fn on_tick() {
    unsafe {
        if !*started_ptr() || *count_ptr() <= 1 {
            return;
        }
        let current = task_ptr(*current_ptr());
        let policy = &mut *policy_ptr();
        if policy.charge(&mut (*current).quantum_ticks) {
            schedule();
        }
    }
}

/// 轮转到下一个就绪任务。
///
/// # Safety
///
/// 仅在中断上下文（或调度器启动路径）调用；不得重入。
unsafe fn schedule() {
    unsafe {
        let from = *current_ptr();
        let policy = &mut *policy_ptr();
        let ready = &mut *ready_ptr();

        let to = match ready.pop_front() {
            Some(next) => next,
            None => {
                // 没有其它就绪任务：当前任务继续跑一个完整时间片。
                policy.refill(&mut (*task_ptr(from)).quantum_ticks);
                return;
            }
        };

        if to == from {
            let _ = ready.push_back(from);
            policy.refill(&mut (*task_ptr(from)).quantum_ticks);
            return;
        }

        // 被抢占的任务回到就绪队列尾部（Round Robin）。
        let _ = ready.push_back(from);
        switch(from, to);
    }
}

/// 执行一次真正的上下文切换（`from` → `to`），并更新记账与日志。
///
/// # Safety
///
/// `from` / `to` 必须是有效且不同的任务下标；`to` 必须处于可运行状态。
unsafe fn switch(from: usize, to: usize) {
    unsafe {
        if from == to || from >= *count_ptr() || to >= *count_ptr() {
            return;
        }

        let a = task_ptr(from);
        let b = task_ptr(to);
        let policy = &mut *policy_ptr();

        (*a).state = ThreadState::Ready;
        (*a).switches = (*a).switches.wrapping_add(1);
        policy.refill(&mut (*a).quantum_ticks);

        (*b).state = ThreadState::Running;
        (*b).runs = (*b).runs.wrapping_add(1);
        policy.refill(&mut (*b).quantum_ticks);

        *current_ptr() = to;
        *switches_ptr() = (*switches_ptr()).wrapping_add(1);

        let from_pid = (*a).pid;
        let to_pid = (*b).pid;
        let to_name = (*b).name;
        let _ = serial_debug(|| {
            serial::print("sched: switch pid=");
            serial::print_dec(from_pid);
            serial::print(" -> ");
            serial::print_dec(to_pid);
            serial::print(" name=");
            serial::println(to_name);
        });

        switch_to(&mut (*a).ctx, &mut (*b).ctx);
    }
}

/// 取调度器统计。
pub fn stats() -> Stats {
    unsafe {
        Stats {
            tasks: *count_ptr(),
            switches: *switches_ptr(),
            started: *started_ptr(),
        }
    }
}

/// 打印一次调度器统计（供 `make test-scheduler` 断言）。
pub fn report() {
    let s = stats();
    let _ = serial_debug(|| {
        serial::print("sched: tasks=");
        serial::print_dec(s.tasks as u64);
        serial::print(" switches=");
        serial::print_dec(s.switches);
        serial::print(" started=");
        serial::print_dec(if s.started { 1 } else { 0 });
        serial::println("");
    });
}

/// 在串口重入保护下执行一次日志输出；保护被占用时跳过本次输出。
fn serial_debug(f: impl FnOnce()) -> bool {
    unsafe {
        if !crate::interrupt::irq::serial_debug_enter() {
            return false;
        }
        f();
        crate::interrupt::irq::serial_debug_exit();
        true
    }
}
