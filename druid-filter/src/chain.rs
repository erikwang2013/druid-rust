use druid_core::DruidError;
use crate::{Filter, FilterContext};

/// FilterChain — 有序的 Filter 列表
///
/// 按注册顺序依次调用各 Filter 的钩子方法。
/// 对应 Java 的 FilterChainImpl。
pub struct FilterChain {
    filters: Vec<Box<dyn Filter>>,
    data_source_name: String,
}

impl FilterChain {
    pub fn new(name: &str) -> Self {
        FilterChain {
            filters: Vec::new(),
            data_source_name: name.to_string(),
        }
    }

    /// 添加 Filter 到链尾
    pub fn add_filter(&mut self, filter: Box<dyn Filter>) -> &mut Self {
        self.filters.push(filter);
        self
    }

    /// 获取 Filter 数量
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// 获取所有 Filter 名称
    pub fn filter_names(&self) -> Vec<String> {
        self.filters.iter().map(|f| f.name().to_string()).collect()
    }

    fn ctx(&self) -> FilterContext {
        FilterContext::new(&self.data_source_name)
    }

    // ── 便利方法：遍历所有 Filter ──

    pub fn data_source_inited(&self) {
        let ctx = self.ctx();
        for f in &self.filters {
            f.data_source_inited(&ctx);
        }
    }

    // 连接生命周期
    pub fn connection_created(&self, conn_id: u64) {
        let ctx = self.ctx().with_connection(conn_id);
        for f in &self.filters {
            f.connection_created(&ctx);
        }
    }

    pub fn connection_borrowed(&self, conn_id: u64, wait_ms: u64) {
        let ctx = self.ctx().with_connection(conn_id);
        for f in &self.filters {
            f.connection_borrowed(&ctx, wait_ms);
        }
    }

    pub fn connection_returned(&self, conn_id: u64) {
        let ctx = self.ctx().with_connection(conn_id);
        for f in &self.filters {
            f.connection_returned(&ctx);
        }
    }

    pub fn connection_closed(&self, conn_id: u64) {
        let ctx = self.ctx().with_connection(conn_id);
        for f in &self.filters {
            f.connection_closed(&ctx);
        }
    }

    pub fn connection_error(&self, conn_id: u64, error: &DruidError) {
        let ctx = self.ctx().with_connection(conn_id);
        for f in &self.filters {
            f.connection_error(&ctx, error);
        }
    }

    // Statement 生命周期
    pub fn statement_execute_before(&self, sql: &str, stmt_id: u64) -> Result<(), DruidError> {
        let ctx = self.ctx().with_sql(sql);
        for f in &self.filters {
            f.statement_execute_before(&ctx)?;
        }
        let _ = stmt_id;
        Ok(())
    }

    pub fn statement_execute_after(&self, sql: &str, stmt_id: u64, elapsed_ms: u64, rows: u64) {
        let ctx = self.ctx().with_sql(sql);
        for f in &self.filters {
            f.statement_execute_after(&ctx, elapsed_ms, rows);
        }
        let _ = stmt_id;
    }

    // ResultSet 生命周期
    pub fn resultset_open(&self, sql: &str) {
        let ctx = self.ctx().with_sql(sql);
        for f in &self.filters {
            f.resultset_open(&ctx);
        }
    }

    pub fn resultset_closed(&self, sql: &str, rows_read: u64) {
        let ctx = self.ctx().with_sql(sql);
        for f in &self.filters {
            f.resultset_closed(&ctx, rows_read);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilterAdapter;
    use std::sync::Mutex;

    /// 测试用 Filter — 记录调用次数
    struct CountingFilter {
        pub borrow_count: Mutex<u64>,
        pub execute_count: Mutex<u64>,
    }

    impl CountingFilter {
        fn new() -> Self {
            CountingFilter {
                borrow_count: Mutex::new(0),
                execute_count: Mutex::new(0),
            }
        }
    }

    impl Filter for CountingFilter {
        fn name(&self) -> &'static str { "CountingFilter" }

        fn connection_borrowed(&self, _ctx: &FilterContext, _wait_ms: u64) {
            *self.borrow_count.lock().unwrap() += 1;
        }

        fn statement_execute_after(&self, _ctx: &FilterContext, _elapsed_ms: u64, _rows: u64) {
            *self.execute_count.lock().unwrap() += 1;
        }
    }

    #[test]
    fn test_filter_chain_calls() {
        let filter = CountingFilter::new();
        let mut chain = FilterChain::new("test-ds");
        chain.add_filter(Box::new(filter));

        chain.connection_borrowed(1, 0);
        chain.connection_borrowed(2, 5);
        chain.statement_execute_after("SELECT 1", 1, 10, 100);

        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_filter_chain_empty() {
        let chain = FilterChain::new("test");
        assert!(chain.is_empty());
        // 空链调用不应 panic
        chain.connection_created(1);
        chain.statement_execute_before("SELECT 1", 1).unwrap();
    }

    #[test]
    fn test_filter_names() {
        let mut chain = FilterChain::new("test");
        chain.add_filter(Box::new(FilterAdapter::new("filter_a")));
        chain.add_filter(Box::new(FilterAdapter::new("filter_b")));
        assert_eq!(chain.filter_names(), vec!["filter_a", "filter_b"]);
    }
}
