//! druid-pool 集成测试共享的 Mock 驱动与辅助函数
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use druid_core::{DruidConfig, DruidError};
use druid_pool::driver::{Connection, Driver};

/// Mock 连接
#[derive(Debug)]
pub struct MockConn {
    pub id: u64,
    /// 本连接是否已被物理关闭
    pub closed: AtomicBool,
    /// 全部连接的物理关闭计数（驱动侧观测）
    pub driver_closed: Arc<AtomicU64>,
    pub exec_calls: Arc<AtomicU64>,
    pub query_calls: Arc<AtomicU64>,
    pub fail_execute: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Connection for MockConn {
    async fn execute(&self, _sql: &str) -> Result<u64, DruidError> {
        self.exec_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_execute.load(Ordering::SeqCst) {
            return Err(DruidError::Database("execute failed".into()));
        }
        Ok(1)
    }

    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        self.query_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![vec!["row".to_string()]])
    }

    async fn close(&self) -> Result<(), DruidError> {
        // 重复 close 只计一次（与真实驱动一致）
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.driver_closed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn ping(&self) -> Result<(), DruidError> {
        Ok(())
    }

    fn connection_id(&self) -> u64 {
        self.id
    }
}

/// 可配置的 Mock 驱动
#[derive(Debug)]
pub struct MockDriver {
    pub connect_count: AtomicU64,
    /// 全部连接的物理关闭计数
    pub closed: Arc<AtomicU64>,
    pub validate_ok: AtomicBool,
    pub connect_fail: Arc<AtomicBool>,
    pub fail_execute: Arc<AtomicBool>,
    pub exec_calls: Arc<AtomicU64>,
    pub query_calls: Arc<AtomicU64>,
    /// connect 耗时（用于取消安全测试）
    pub connect_latency: Duration,
    /// validate 耗时（用于取消安全 / 校验超时测试）
    pub validate_latency: Duration,
}

impl Default for MockDriver {
    fn default() -> Self {
        MockDriver {
            connect_count: AtomicU64::new(0),
            closed: Arc::new(AtomicU64::new(0)),
            validate_ok: AtomicBool::new(true),
            connect_fail: Arc::new(AtomicBool::new(false)),
            fail_execute: Arc::new(AtomicBool::new(false)),
            exec_calls: Arc::new(AtomicU64::new(0)),
            query_calls: Arc::new(AtomicU64::new(0)),
            connect_latency: Duration::ZERO,
            validate_latency: Duration::ZERO,
        }
    }
}

impl MockDriver {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn closed_count(&self) -> u64 {
        self.closed.load(Ordering::SeqCst)
    }
    pub fn exec_count(&self) -> u64 {
        self.exec_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Driver for MockDriver {
    type Connection = MockConn;

    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
        _timeout: Option<Duration>,
    ) -> Result<MockConn, DruidError> {
        if !self.connect_latency.is_zero() {
            tokio::time::sleep(self.connect_latency).await;
        }
        if self.connect_fail.load(Ordering::SeqCst) {
            return Err(DruidError::Pool("connect refused".into()));
        }
        let id = self.connect_count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(MockConn {
            id,
            closed: AtomicBool::new(false),
            driver_closed: self.closed.clone(),
            exec_calls: self.exec_calls.clone(),
            query_calls: self.query_calls.clone(),
            fail_execute: self.fail_execute.clone(),
        })
    }

    fn name(&self) -> &'static str {
        "MockDriver"
    }

    async fn validate(&self, _conn: &MockConn) -> Result<(), DruidError> {
        if !self.validate_latency.is_zero() {
            tokio::time::sleep(self.validate_latency).await;
        }
        if self.validate_ok.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(DruidError::Pool("invalid".into()))
        }
    }
}

/// 基础配置：禁用后台驱逐循环、关闭借用校验
pub fn cfg(url: &str) -> DruidConfig {
    let mut c = DruidConfig::new(url, "u", "p");
    c.time_between_eviction_runs_ms = 0;
    c.test_on_borrow = false;
    c
}

/// 轮询等待条件成立（最多 2s），超时 panic
pub async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("等待超时: {}", what);
}
