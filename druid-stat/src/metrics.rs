use std::sync::atomic::{AtomicU64, Ordering};

/// 连接池运行时指标（lock-free）
#[derive(Debug, Default)]
pub struct PoolMetrics {
    /// 活跃连接数
    active_count: AtomicU64,
    /// 空闲连接数
    idle_count: AtomicU64,
    /// 等待获取连接的请求数
    waiting_count: AtomicU64,
    /// 总借用次数
    borrow_count: AtomicU64,
    /// 连接创建总数
    create_count: AtomicU64,
    /// 连接关闭总数
    destroy_count: AtomicU64,
    /// 从空闲池命中次数
    cache_hit_count: AtomicU64,
    /// 总等待时间(ns)
    total_wait_ns: AtomicU64,
}

impl PoolMetrics {
    pub fn new() -> Self {
        PoolMetrics::default()
    }

    pub fn set_active(&self, n: usize) {
        self.active_count.store(n as u64, Ordering::Relaxed);
    }
    pub fn set_idle(&self, n: usize) {
        self.idle_count.store(n as u64, Ordering::Relaxed);
    }
    pub fn inc_waiting(&self) {
        self.waiting_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec_waiting(&self) {
        // saturating：多余的解等待计数不会把计数回绕成 u64::MAX
        let _ = self
            .waiting_count
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }
    pub fn inc_borrow(&self) {
        self.borrow_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_cache_hit(&self) {
        self.cache_hit_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_create(&self) {
        self.create_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_destroy(&self) {
        self.destroy_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_wait_time_ns(&self, ns: u64) {
        self.total_wait_ns.fetch_add(ns, Ordering::Relaxed);
    }

    // Getters
    pub fn active(&self) -> u64 {
        self.active_count.load(Ordering::Relaxed)
    }
    pub fn idle(&self) -> u64 {
        self.idle_count.load(Ordering::Relaxed)
    }
    pub fn waiting(&self) -> u64 {
        self.waiting_count.load(Ordering::Relaxed)
    }
    pub fn borrow_count(&self) -> u64 {
        self.borrow_count.load(Ordering::Relaxed)
    }
    pub fn create_count(&self) -> u64 {
        self.create_count.load(Ordering::Relaxed)
    }
    pub fn cache_hit_count(&self) -> u64 {
        self.cache_hit_count.load(Ordering::Relaxed)
    }

    pub fn destroy_count(&self) -> u64 {
        self.destroy_count.load(Ordering::Relaxed)
    }
    pub fn avg_wait_ms(&self) -> f64 {
        let count = self.borrow_count();
        if count == 0 {
            0.0
        } else {
            self.total_wait_ns.load(Ordering::Relaxed) as f64 / count as f64 / 1_000_000.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let m = PoolMetrics::new();
        assert_eq!(m.active(), 0);
        assert_eq!(m.idle(), 0);
        assert_eq!(m.waiting(), 0);
        assert_eq!(m.borrow_count(), 0);
        assert_eq!(m.create_count(), 0);
        assert_eq!(m.destroy_count(), 0);
        assert_eq!(m.cache_hit_count(), 0);
        assert_eq!(m.avg_wait_ms(), 0.0);
    }

    #[test]
    fn test_setters_and_counters() {
        let m = PoolMetrics::new();
        m.set_active(3);
        m.set_idle(5);
        assert_eq!(m.active(), 3);
        assert_eq!(m.idle(), 5);

        m.inc_waiting();
        m.inc_waiting();
        m.dec_waiting();
        assert_eq!(m.waiting(), 1);

        m.inc_borrow();
        m.inc_cache_hit();
        m.inc_create();
        m.inc_destroy();
        assert_eq!(m.borrow_count(), 1);
        assert_eq!(m.cache_hit_count(), 1);
        assert_eq!(m.create_count(), 1);
        assert_eq!(m.destroy_count(), 1);
    }

    #[test]
    fn test_avg_wait_ms() {
        let m = PoolMetrics::new();
        // 2 次借用，共 2ms 等待 → 平均 1ms
        m.inc_borrow();
        m.inc_borrow();
        m.add_wait_time_ns(1_500_000);
        m.add_wait_time_ns(500_000);
        assert!((m.avg_wait_ms() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_dec_waiting_does_not_underflow() {
        let m = PoolMetrics::new();
        m.dec_waiting(); // 0 - 1 不再回绕
        m.dec_waiting();
        assert_eq!(m.waiting(), 0);
    }
}
