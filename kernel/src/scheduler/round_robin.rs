//! Round Robin 调度策略（v0.5 第一版唯一策略，ADR-005）
//!
//! 时间片以 tick 计：PIT 为 100Hz，故 20 tick = 200ms。
//! 后续可在此基础上扩展 Priority / CFS-like / Real-time，但不在 v0.5 实现。

#![allow(dead_code)]

/// 默认时间片（tick 数）。100Hz → 200ms。
pub const DEFAULT_QUANTUM_TICKS: u32 = 20;

/// Round Robin 策略参数。
#[derive(Clone, Copy)]
pub struct RoundRobin {
    quantum: u32,
}

impl RoundRobin {
    /// `quantum` 至少为 1，避免时间片为 0 造成每 tick 都切换。
    pub const fn new(quantum: u32) -> Self {
        Self {
            quantum: if quantum == 0 { 1 } else { quantum },
        }
    }

    pub const fn quantum(&self) -> u32 {
        self.quantum
    }

    /// 消耗一个 tick；返回 `true` 表示时间片用尽、应当让出 CPU。
    pub fn charge(&self, remaining: &mut u32) -> bool {
        if *remaining > 0 {
            *remaining -= 1;
        }
        *remaining == 0
    }

    /// 重新装满时间片。
    pub fn refill(&self, remaining: &mut u32) {
        *remaining = self.quantum;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantum_cannot_be_zero() {
        assert_eq!(RoundRobin::new(0).quantum(), 1);
        assert_eq!(RoundRobin::new(20).quantum(), 20);
    }

    #[test]
    fn charge_expires_exactly_at_quantum() {
        let policy = RoundRobin::new(3);
        let mut left = policy.quantum();
        assert!(!policy.charge(&mut left));
        assert!(!policy.charge(&mut left));
        assert!(policy.charge(&mut left));
        policy.refill(&mut left);
        assert_eq!(left, 3);
    }
}
