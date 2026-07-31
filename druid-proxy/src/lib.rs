//! 数据库代理层
//!
//! 包装 Connection/Statement/ResultSet，支持 Filter-Chain 拦截。
//! 对应 Java Druid 的 ProxyConnection/ProxyStatement/ProxyResultSet。

use druid_core::DruidError;
use druid_filter::FilterChain;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 代理连接 — 包装真实连接，Filter 回调自动触发
pub struct ProxyConnection {
    inner: Arc<dyn RawConnection>,
    filter_chain: Arc<FilterChain>,
    conn_id: u64,
    closed: AtomicBool,
}

/// 代理 Statement
pub struct ProxyStatement {
    conn: Arc<ProxyConnection>,
    filter_chain: Arc<FilterChain>,
}

/// 原始连接 trait
pub trait RawConnection: Send + Sync {
    fn execute(&self, sql: &str) -> Result<u64, DruidError>;
    fn close(&self) -> Result<(), DruidError>;
    fn id(&self) -> u64;
    fn is_closed(&self) -> bool;
}

impl ProxyConnection {
    pub fn new(inner: Arc<dyn RawConnection>, filter_chain: Arc<FilterChain>) -> Self {
        let id = inner.id();
        filter_chain.connection_created(id);
        ProxyConnection {
            inner,
            filter_chain,
            conn_id: id,
            closed: AtomicBool::new(false),
        }
    }

    pub fn create_statement(self: &Arc<Self>) -> ProxyStatement {
        ProxyStatement {
            conn: self.clone(),
            filter_chain: self.filter_chain.clone(),
        }
    }

    pub fn close(&self) -> Result<(), DruidError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.filter_chain.connection_closed(self.conn_id);
        self.inner.close()
    }

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
        if !self.closed.load(Ordering::SeqCst) {
            self.filter_chain.connection_closed(self.conn_id);
            let _ = self.inner.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

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
}
