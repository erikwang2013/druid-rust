//! 数据库代理层
//!
//! 包装 Connection/Statement/ResultSet，支持 Filter-Chain 拦截。
//! 对应 Java Druid 的 ProxyConnection/ProxyStatement/ProxyResultSet。

#![warn(missing_docs)]

use druid_core::DruidError;
use druid_filter::FilterChain;
use std::sync::{Arc, Mutex};

/// 代理连接 — 包装真实连接，Filter 回调自动触发
pub struct ProxyConnection {
    inner: Arc<dyn RawConnection>,
    filter_chain: Arc<FilterChain>,
    conn_id: u64,
    /// 已确认关闭（`inner.close()` 成功后才置位，失败保持 false 允许重试）
    closed: Mutex<bool>,
}

/// 代理 Statement
pub struct ProxyStatement {
    conn: Arc<ProxyConnection>,
    filter_chain: Arc<FilterChain>,
}

/// 原始连接 trait
pub trait RawConnection: Send + Sync {
    /// 执行 SQL，返回受影响行数
    fn execute(&self, sql: &str) -> Result<u64, DruidError>;
    /// 物理关闭连接
    fn close(&self) -> Result<(), DruidError>;
    /// 连接 ID
    fn id(&self) -> u64;
    /// 底层连接是否已关闭
    fn is_closed(&self) -> bool;
}

impl ProxyConnection {
    /// 包装一个原始连接（触发 `connection_created` 回调）
    pub fn new(inner: Arc<dyn RawConnection>, filter_chain: Arc<FilterChain>) -> Self {
        let id = inner.id();
        filter_chain.connection_created(id);
        ProxyConnection {
            inner,
            filter_chain,
            conn_id: id,
            closed: Mutex::new(false),
        }
    }

    /// 创建绑定到本连接的代理 Statement
    pub fn create_statement(self: &Arc<Self>) -> ProxyStatement {
        ProxyStatement {
            conn: self.clone(),
            filter_chain: self.filter_chain.clone(),
        }
    }

    /// 关闭底层连接（幂等）
    ///
    /// `inner.close()` 成功后才标记已关闭并回调 Filter；失败时不置位、不上报销毁，
    /// 错误上抛给调用方以便重试 —— 否则统计显示"已销毁"而物理连接仍打开，fd 泄漏被掩盖。
    pub fn close(&self) -> Result<(), DruidError> {
        let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        if *closed {
            return Ok(());
        }
        self.inner.close()?;
        *closed = true;
        drop(closed);
        self.filter_chain.connection_closed(self.conn_id);
        Ok(())
    }

    /// 连接 ID
    pub fn id(&self) -> u64 {
        self.conn_id
    }
}

impl ProxyStatement {
    /// 执行 SQL（带 Filter 拦截）
    pub fn execute(&self, sql: &str) -> Result<u64, DruidError> {
        self.filter_chain
            .statement_execute_before(sql, self.conn.conn_id)?;
        let start = std::time::Instant::now();
        let result = self.conn.inner.execute(sql);
        let elapsed = start.elapsed().as_millis() as u64;
        if let Ok(rows) = &result {
            self.filter_chain
                .statement_execute_after(sql, self.conn.conn_id, elapsed, *rows);
        }
        result
    }
}

impl Drop for ProxyConnection {
    fn drop(&mut self) {
        let mut closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        if *closed {
            return;
        }
        match self.inner.close() {
            Ok(()) => {
                *closed = true;
                drop(closed);
                self.filter_chain.connection_closed(self.conn_id);
            }
            // 关闭失败必须显式告警：此时 fd 可能泄漏，且 Drop 无法重试
            Err(e) => tracing::error!(
                "ProxyConnection drop: 关闭连接 {} 失败: {}（fd 可能泄漏）",
                self.conn_id,
                e
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    struct MockConn {
        id: u64,
        exec_count: AtomicU64,
    }
    impl RawConnection for MockConn {
        fn execute(&self, _: &str) -> Result<u64, DruidError> {
            self.exec_count.fetch_add(1, Ordering::SeqCst);
            Ok(1)
        }
        fn close(&self) -> Result<(), DruidError> {
            Ok(())
        }
        fn id(&self) -> u64 {
            self.id
        }
        fn is_closed(&self) -> bool {
            false
        }
    }

    #[test]
    fn test_proxy_execute() {
        let inner = Arc::new(MockConn {
            id: 1,
            exec_count: AtomicU64::new(0),
        });
        let fc = Arc::new(FilterChain::new("test"));
        let conn = Arc::new(ProxyConnection::new(inner, fc));
        let stmt = conn.create_statement();
        assert!(stmt.execute("SELECT 1").is_ok());
    }

    #[test]
    fn test_proxy_close() {
        let inner = Arc::new(MockConn {
            id: 2,
            exec_count: AtomicU64::new(0),
        });
        let fc = Arc::new(FilterChain::new("test"));
        let conn = ProxyConnection::new(inner, fc);
        assert!(conn.close().is_ok());
    }

    /// 可观察 close 行为的 Mock
    struct TrackingConn {
        id: u64,
        closed: std::sync::atomic::AtomicBool,
        fail_execute: bool,
    }

    impl TrackingConn {
        fn new(id: u64, fail_execute: bool) -> Self {
            TrackingConn {
                id,
                closed: std::sync::atomic::AtomicBool::new(false),
                fail_execute,
            }
        }
    }

    impl RawConnection for TrackingConn {
        fn execute(&self, _: &str) -> Result<u64, DruidError> {
            if self.fail_execute {
                Err(DruidError::Database("query failed".into()))
            } else {
                Ok(3)
            }
        }
        fn close(&self) -> Result<(), DruidError> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn id(&self) -> u64 {
            self.id
        }
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
    }

    /// 记录 Filter 回调的测试 Filter（共享计数器，filter 本身被 Box 移入链中）
    #[derive(Default)]
    struct Counters {
        created: AtomicU64,
        closed: AtomicU64,
        before: AtomicU64,
        after: AtomicU64,
    }

    struct CountingFilter {
        counters: Arc<Counters>,
    }

    impl druid_filter::Filter for CountingFilter {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn connection_created(&self, _: &druid_filter::FilterContext) {
            self.counters.created.fetch_add(1, Ordering::SeqCst);
        }
        fn connection_closed(&self, _: &druid_filter::FilterContext) {
            self.counters.closed.fetch_add(1, Ordering::SeqCst);
        }
        fn statement_execute_before(
            &self,
            _: &druid_filter::FilterContext,
        ) -> Result<(), DruidError> {
            self.counters.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn statement_execute_after(&self, _: &druid_filter::FilterContext, _: u64, _: u64) {
            self.counters.after.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_proxy_close_idempotent() {
        let inner = Arc::new(TrackingConn::new(1, false));
        let conn = ProxyConnection::new(inner.clone(), Arc::new(FilterChain::new("test")));
        assert!(conn.close().is_ok());
        assert!(conn.close().is_ok()); // 二次 close 幂等
        assert!(inner.is_closed());
        assert_eq!(conn.id(), 1);
    }

    #[test]
    fn test_proxy_drop_closes_inner() {
        let inner = Arc::new(TrackingConn::new(1, false));
        {
            let _conn = ProxyConnection::new(inner.clone(), Arc::new(FilterChain::new("test")));
        }
        assert!(inner.is_closed()); // 未显式 close，drop 时应关闭底层连接
    }

    #[test]
    fn test_proxy_execute_error_propagates() {
        let inner = Arc::new(TrackingConn::new(1, true));
        let conn = Arc::new(ProxyConnection::new(
            inner,
            Arc::new(FilterChain::new("test")),
        ));
        let stmt = conn.create_statement();
        let err = stmt.execute("SELECT BAD").unwrap_err();
        assert!(err.to_string().contains("query failed"));
    }

    #[test]
    fn test_proxy_filter_callbacks_fire() {
        let counters = Arc::new(Counters::default());
        let mut fc = FilterChain::new("test");
        fc.add_filter(Box::new(CountingFilter {
            counters: counters.clone(),
        }));
        let fc = Arc::new(fc);

        let inner = Arc::new(TrackingConn::new(9, false));
        let conn = Arc::new(ProxyConnection::new(inner, fc.clone()));
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);

        let stmt = conn.create_statement();
        assert_eq!(stmt.execute("SELECT 1").unwrap(), 3);
        assert_eq!(counters.before.load(Ordering::SeqCst), 1);
        assert_eq!(counters.after.load(Ordering::SeqCst), 1);

        conn.close().unwrap();
        assert_eq!(counters.closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_proxy_statement_on_closed_connection_no_panic() {
        let inner = Arc::new(TrackingConn::new(1, false));
        let conn = Arc::new(ProxyConnection::new(
            inner,
            Arc::new(FilterChain::new("test")),
        ));
        let stmt = conn.create_statement();
        conn.close().unwrap();
        // close 后执行不 panic，底层调用照常转发
        assert!(stmt.execute("SELECT 1").is_ok());
    }

    /// close 行为可控的连接（用于失败路径测试）
    struct FlakyConn {
        id: u64,
        fail: AtomicBool,
        close_attempts: AtomicU64,
        closed: AtomicBool,
    }

    impl FlakyConn {
        fn new(id: u64, fail: bool) -> Self {
            FlakyConn {
                id,
                fail: AtomicBool::new(fail),
                close_attempts: AtomicU64::new(0),
                closed: AtomicBool::new(false),
            }
        }
    }

    impl RawConnection for FlakyConn {
        fn execute(&self, _: &str) -> Result<u64, DruidError> {
            Ok(1)
        }
        fn close(&self) -> Result<(), DruidError> {
            self.close_attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err(DruidError::Database("server has gone away".into()))
            } else {
                self.closed.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
        fn id(&self) -> u64 {
            self.id
        }
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
    }

    fn counting_chain(counters: &Arc<Counters>) -> FilterChain {
        let mut fc = FilterChain::new("test");
        fc.add_filter(Box::new(CountingFilter {
            counters: counters.clone(),
        }));
        fc
    }

    #[test]
    fn test_proxy_close_failure_not_counted_and_retryable() {
        let counters = Arc::new(Counters::default());
        let inner = Arc::new(FlakyConn::new(7, true));
        let conn = ProxyConnection::new(inner.clone(), Arc::new(counting_chain(&counters)));

        // 首次 close 失败：错误上抛、销毁未记账、未标记已关闭
        let err = conn.close().unwrap_err();
        assert!(err.to_string().contains("server has gone away"));
        assert_eq!(counters.closed.load(Ordering::SeqCst), 0);
        assert_eq!(inner.close_attempts.load(Ordering::SeqCst), 1);

        // 二次 close 不被"已置位"吞掉，真正重试底层关闭
        assert!(conn.close().is_err());
        assert_eq!(inner.close_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(counters.closed.load(Ordering::SeqCst), 0);

        // 故障恢复后重试成功：置位 + 记账；再次 close 幂等
        inner.fail.store(false, Ordering::SeqCst);
        assert!(conn.close().is_ok());
        assert!(inner.is_closed());
        assert_eq!(counters.closed.load(Ordering::SeqCst), 1);
        assert!(conn.close().is_ok());
        assert_eq!(inner.close_attempts.load(Ordering::SeqCst), 3); // 幂等：不再触碰底层
    }

    #[test]
    fn test_proxy_drop_close_failure_not_counted() {
        let counters = Arc::new(Counters::default());
        let inner = Arc::new(FlakyConn::new(8, true));
        {
            let _conn = ProxyConnection::new(inner.clone(), Arc::new(counting_chain(&counters)));
        } // Drop 关闭失败：不 panic、不上报销毁
        assert_eq!(counters.closed.load(Ordering::SeqCst), 0);
        assert!(!inner.is_closed());
    }
}
