//! 线程上下文（v0.5）
//!
//! 寄存器保存/恢复与切换原语位于 `arch::x86_64::context`；本模块只做语义转发，
//! 让 process 层引用上下文而不直接依赖 arch 路径。
//!
//! 约定：
//! - `ThreadContext` 保存 callee-saved 寄存器区与栈指针（内核态切换）；
//! - `init_context` 为新线程准备初始内核栈；
//! - `switch_to` 完成一次上下文切换。

pub use crate::arch::x86_64::context::{init_context, switch_to, ThreadContext};
