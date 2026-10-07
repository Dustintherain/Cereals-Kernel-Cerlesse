//! IRQ 控制器（v0.4 起，分 PIC/APIC 两条路线）
//!
//! 本模块是 v0.4 中断控制器任务的初始骨架与 PIC 第一版实现。
//! 当前阶段优先接通 PIC（8259A）：初始化、IRQ→向量映射、屏蔽/EOI 原语。
//! 后续可在同一模块中替换/扩展为 APIC。
//!!
//! 约定：
//! - 异常向量（0..31）已在 `arch::x86_64::idt` 中建立。
//! - IRQ（典型 0..15）是外设中断，需经控制器分发后再进入内核处理。
//! - v0.4 第一版优先 PIC（8259A），后续再迁移到 APIC。
//!!
//! PIC 端口（x86 标准）。
//! - Master: 0x20 (command), 0x21 (mask)
//! - Slave:  0xA0 (command), 0xA1 (mask)
//!!
//! 初始化序列（ICW 1..4）沿用常见兼容映射：
//! - IRQ0..IRQ7 → 向量 32..39（master）
//! - IRQ8..IRQ15 → 向量 40..47（slave）
//! 即 IRQ_BASE_VECTOR=32，slave 偏移从 8 开始。

#![allow(dead_code)]

use core::fmt;

/// 当前所选的中断控制器类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerKind {
    /// 可编程中断控制器（8259A），v0.4 第一版目标。
    Pic,
    /// 高级可编程中断控制器（APIC），后续路线。
    Apic,
}

/// 中断控制器的统一视图（v0.4 第一版仅含 PIC）。
pub struct Controller {
    kind: ControllerKind,
    /// 当前 PIC mask 寄存器快照（仅对 Pic 有效；Apic 尚未实现）。
    pic_mask: u16,
}

impl Controller {
    pub const fn new(kind: ControllerKind) -> Self {
        Self {
            kind,
            // 初始 mask 为空（全部 IRQ 屏蔽）。初始化完成前不能受外部中断。
            pic_mask: 0xFFFF,
        }
    }

    pub fn kind(&self) -> ControllerKind {
        self.kind
    }

    /// 屏蔽指定 IRQ（PIC mask 位图语义）。
    pub fn mask_irq(&mut self, irq: u8) {
        if irq >= 16 {
            return;
        }
        self.pic_mask |= 1u16.checked_shl(irq as u32).unwrap_or(0);
    }

    /// 解除屏蔽指定 IRQ（PIC mask 位图语义）。
    pub fn unmask_irq(&mut self, irq: u8) {
        if irq >= 16 {
            return;
        }
        self.pic_mask &= !1u16.checked_shl(irq as u32).unwrap_or(0);
    }

    /// 发送 PIC 非自动 EOI（目前仅对 Pic 有效；Apic 尚未实现）。
    /// 若当前控制器不是 Pic，则此调用为 no-op。
    pub fn eoi(&self) {
        if matches!(self.kind, ControllerKind::Pic) {
            // 非自动 EOI：直接向主片命令端口写 0x20。
            // 本内核暂不依赖自动 EOI 模式，故每次中断结束都显式 EOI。
            unsafe {
                outb(PIC1_CMD, 0x20);
            }
        }
    }

    /// 初始化 PIC（ICW 1..4）。
    /// 调用前提：
    /// - 串口已初始化（至少不需要依赖串口本身，但 kernel_main 通常已经调用过）。
    /// - 此函数在第一次启用 IRQ 之前调用。
    pub fn init_pic(&mut self) {
        if !matches!(self.kind, ControllerKind::Pic) {
            return;
        }

        // ICW 1: 初始化、级联、需要 ICW4
        //   bit0 = 1: 初始化
        //   bit1 = 0: 级联模式（主/从）
        //   bit2 = 1: 需要 ICW4
        //   bit4..7 = 0
        unsafe {
            outb(PIC1_CMD, 0x11);
            outb(PIC2_CMD, 0x11);

            // ICW 2: IRQ 基向量
            outb(PIC1_DATA, IRQ_BASE_VECTOR);
            outb(PIC2_DATA, IRQ_BASE_VECTOR + 8);

            // ICW 3: 级联关系
            // master 上 slave 连接到 IRQ2 → master 发送 0x04 (bit2)
            // slave 自身识别为 slave → 0x02
            outb(PIC1_DATA, 0x04);
            outb(PIC2_DATA, 0x02);

            // ICW 4: 8086 模式
            outb(PIC1_DATA, 0x01);
            outb(PIC2_DATA, 0x01);
        }

        // 初始化完成后，默认关闭所有 IRQ，防止立即受外设打扰。
        self.pic_mask = 0xFFFF;
        unsafe {
            outb(PIC1_DATA, self.pic_mask as u8);
            outb(PIC2_DATA, (self.pic_mask >> 8) as u8);
        }
    }

    /// 启用某 IRQ（先取消屏蔽，再同步写两片 mask）。
    ///
    /// 注意：这仅修改 PIC mask。处理器中断使能（IF）与否由架构层决定。
    pub fn enable_irq(&mut self, irq: u8) {
        if irq >= 16 {
            return;
        }
        self.unmask_irq(irq);
        self.sync_mask();
    }

    /// 禁用某 IRQ（屏蔽位图 + 同步写两片 mask）。
    pub fn disable_irq(&mut self, irq: u8) {
        if irq >= 16 {
            return;
        }
        self.mask_irq(irq);
        self.sync_mask();
    }

    /// 返回当前 PIC mask 位图（仅对 Pic 有效；Apic 返回 0）。
    pub fn pic_mask(&self) -> u16 {
        self.pic_mask
    }

    /// 将当前 mask 同步到 PIC 两片的 mask 寄存器。
    fn sync_mask(&mut self) {
        if !matches!(self.kind, ControllerKind::Pic) {
            return;
        }
        unsafe {
            outb(PIC1_DATA, self.pic_mask as u8);
            outb(PIC2_DATA, (self.pic_mask >> 8) as u8);
        }
    }

    /// 带向量的 PIC EOI：若该向量属于 slave IRQ（IRQ8..15），
    /// 先 EOI slave，再 EOI master。否则仅 EOI master。
    pub(crate) unsafe fn eoi_with_vector(&mut self, vector: u8) {
        if vector >= IRQ_BASE_VECTOR + 8 {
            unsafe { outb(PIC2_CMD, 0x20); }
        }
        unsafe { outb(PIC1_CMD, 0x20); }
    }

    /// 以纯端口语义直接发送主片 EOI（不依赖控制器 mask 状态）。
    /// 与 `Controller::eoi()` 语义一致，供中断上下文中の直接调用备用。
    pub(crate) unsafe fn eoi_master() {
        unsafe { outb(PIC1_CMD, 0x20); }
    }
}

/// 以静态无状态方式发送 PIC EOI（仅对 Pic 生效；Apic 返回）。
/// 用于中断上下文中の临时 EOI 占位；后续应替换为全局/单例控制器视图。
pub(crate) unsafe fn eoi_with_vector_static(vector: u8) {
    if vector >= IRQ_BASE_VECTOR + 8 {
        unsafe { outb(PIC2_CMD, 0x20); }
    }
    unsafe { outb(PIC1_CMD, 0x20); }
}

impl fmt::Debug for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Controller")
            .field("kind", &self.kind)
            .field("pic_mask", &self.pic_mask)
            .finish()
    }
}

/// 模块结尾占位，不改变模块语义

/// IRQ 向量映射示例（PIC 传统做法：IRQ0 → 向量 32，随后顺延）。
/// 这里先写死一个常见映射，便于后续对齐文档里的“IRQ 分发”语义。
pub const IRQ_BASE_VECTOR: u8 = 32;

/// 返回 IRQ0 对应的向量号（PIC 映射下恒为 `IRQ_BASE_VECTOR`）。
pub const fn irq0_vector() -> u8 {
    IRQ_BASE_VECTOR
}

pub const fn irq_to_vector(irq: u8) -> Option<u8> {
    if irq < 16 {
        // 传统兼容映射：IRQ0..IRQ15 → IRQ_BASE_VECTOR..IRQ_BASE_VECTOR+15
        let v = IRQ_BASE_VECTOR + irq;

        // 结果必须落在有效向量空间内（u8 范围内）才可返回。
        // IRQ_BASE_VECTOR=32，irq<16，因此 v 恒在 u8 范围内。
        return Some(v);
    }
    None
}

// ---- PIC 端口操作（内联，不依赖串口驱动） ----

/// 主片命令端口
const PIC1_CMD: u16 = 0x20;
/// 主片屏蔽端口
const PIC1_DATA: u16 = 0x21;
/// 从片命令端口
const PIC2_CMD: u16 = 0xA0;
/// 从片屏蔽端口
const PIC2_DATA: u16 = 0xA1;

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack, preserves_flags)
        );
    }
}

// 不再在此模块使用 inb；仅 outb 对 PIC 初始化与 EOI 已足够。
// 若后续需要轮询 PIC 状态，可在此补充 inb。

/// 以纯端口语义直接发送主片 EOI（不依赖控制器 mask 状态）。
/// 与 `Controller::eoi()` 语义一致，供中断上下文中の直接调用备用。
pub(crate) unsafe fn eoi_master() {
    unsafe { outb(PIC1_CMD, 0x20); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irq_mapping_bounds() {
        assert!(irq_to_vector(0).is_some());
        assert!(irq_to_vector(15).is_some());
        assert_eq!(irq_to_vector(0), Some(IRQ_BASE_VECTOR));
        assert_eq!(irq_to_vector(15), Some(IRQ_BASE_VECTOR + 15));
        assert!(irq_to_vector(16).is_none());
    }

    #[test]
    fn controller_pic_mask_lifecycle() {
        let mut c = Controller::new(ControllerKind::Pic);
        // 初始应全屏蔽
        assert_eq!(c.pic_mask(), 0xFFFF);
        c.enable_irq(3);
        assert_eq!(c.pic_mask() & (1 << 3), 0);
        c.disable_irq(3);
        assert_eq!(c.pic_mask() & (1 << 3), 1 << 3);

        // 对非法 IRQ 保持静默
        c.enable_irq(16);
        c.disable_irq(255);
        assert_eq!(c.pic_mask(), 0xFFFF | (1 << 3)); // bit3 仍被置位（disable 补写）
    }

    #[test]
    fn controller_kind_when_apic_still_noop() {
        let c = Controller::new(ControllerKind::Apic);
        assert_eq!(c.pic_mask(), 0xFFFF);
        // Apic 当前不提供 mask 操作语义（未来再扩展）。
    }
}
