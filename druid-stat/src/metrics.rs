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
        self.waiting_count.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn inc_borrow(&self) {
        self.borrow_count.fetch_add(1, Ordering::Relaxed);
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
