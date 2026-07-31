pub mod metrics;

use std::collections::HashMap;
use std::sync::Mutex;

use druid_core::DruidError;
use druid_filter::{Filter, FilterContext};

/// 单条 SQL 的执行统计
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SqlStat {
    /// SQL 文本（截断后）
    pub sql: String,
    /// 执行次数
    pub execute_count: u64,
    /// 总耗时(ms)
    pub total_time_ms: u64,
    /// 最大耗时(ms)
    pub max_time_ms: u64,
    /// 错误次数
    pub error_count: u64,
    /// 最后执行时间
    pub last_execute_time: Option<String>,
    /// 读取行数
    pub rows_read: u64,
}

impl SqlStat {
    fn new(sql: &str) -> Self {
        SqlStat {
            sql: sql.chars().take(200).collect(),
            ..Default::default()
        }
    }
}

/// 数据源级别统计
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct DataSourceStat {
    /// 数据源名称
    pub name: String,
    /// 连接创建数
    pub create_count: u64,
    /// 连接关闭数
    pub destroy_count: u64,
    /// 连接借用次数
    pub borrow_count: u64,
    /// 连接归还次数
    pub return_count: u64,
    /// 总等待时间(ms)
    pub total_wait_time_ms: u64,
    /// SQL 执行次数
    pub execute_count: u64,
    /// SQL 错误次数
    pub error_count: u64,
    /// 当前活跃连接
    pub active_count: usize,
    /// 当前空闲连接
    pub idle_count: usize,
}

/// StatFilter — SQL 监控统计 Filter
///
/// 实现 Filter trait，实时采集 SQL 执行统计和连接池指标。
pub struct StatFilter {
    #[allow(dead_code)]
    name: String,
    sql_stats: Mutex<HashMap<String, SqlStat>>,
    ds_stat: Mutex<DataSourceStat>,
    slow_sql_ms: u64,
}

impl StatFilter {
    pub fn new(name: &str, slow_sql_ms: u64) -> Self {
        StatFilter {
            name: name.to_string(),
            sql_stats: Mutex::new(HashMap::new()),
            ds_stat: Mutex::new(DataSourceStat {
                name: name.to_string(),
                ..Default::default()
            }),
            slow_sql_ms,
        }
    }

    /// 获取所有 SQL 统计（按总耗时降序排列）
    pub fn get_sql_stats(&self) -> Vec<SqlStat> {
        let mut stats: Vec<SqlStat> = self
            .sql_stats
            .lock()
            .expect("stat lock poisoned")
            .values()
            .cloned()
            .collect();
        stats.sort_by_key(|b| std::cmp::Reverse(b.total_time_ms));
        stats
    }

    /// 获取慢 SQL 列表
    pub fn get_slow_sql(&self) -> Vec<SqlStat> {
        self.get_sql_stats()
            .into_iter()
            .filter(|s| s.max_time_ms >= self.slow_sql_ms)
            .collect()
    }

    /// 获取数据源级别统计
    pub fn get_datasource_stat(&self) -> DataSourceStat {
        self.ds_stat.lock().expect("stat lock poisoned").clone()
    }

    /// 获取总执行次数
    pub fn execute_count(&self) -> u64 {
        self.ds_stat
            .lock()
            .expect("stat lock poisoned")
            .execute_count
    }
}

impl Filter for StatFilter {
    fn name(&self) -> &'static str {
        "stat"
    }

    fn init(&mut self) -> Result<(), DruidError> {
        tracing::info!("StatFilter initialized (slow_sql_ms={})", self.slow_sql_ms);
        Ok(())
    }

    fn connection_created(&self, _ctx: &FilterContext) {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.create_count += 1;
        stat.idle_count += 1;
    }

    fn connection_borrowed(&self, _ctx: &FilterContext, wait_ms: u64) {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.borrow_count += 1;
        stat.total_wait_time_ms += wait_ms;
        stat.active_count += 1;
        stat.idle_count = stat.idle_count.saturating_sub(1);
    }

    fn connection_returned(&self, _ctx: &FilterContext) {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.return_count += 1;
        stat.active_count = stat.active_count.saturating_sub(1);
        stat.idle_count += 1;
    }

    fn connection_closed(&self, _ctx: &FilterContext) {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.destroy_count += 1;
        stat.active_count = stat.active_count.saturating_sub(1);
        stat.idle_count = stat.idle_count.saturating_sub(1);
    }

    fn statement_execute_before(&self, _ctx: &FilterContext) -> Result<(), DruidError> {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.execute_count += 1;
        Ok(())
    }

    fn statement_execute_after(&self, ctx: &FilterContext, elapsed_ms: u64, rows: u64) {
        let sql = ctx.sql.as_deref().unwrap_or("UNKNOWN");
        let mut stats = self.sql_stats.lock().expect("stat lock poisoned");
        let entry = if let Some(e) = stats.get_mut(sql) {
            e
        } else {
            stats
                .entry(sql.to_string())
                .or_insert_with(|| SqlStat::new(sql))
        };
        entry.execute_count += 1;
        entry.total_time_ms += elapsed_ms;
        entry.max_time_ms = entry.max_time_ms.max(elapsed_ms);
        entry.rows_read += rows;
        entry.last_execute_time =
            Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());

        // 慢 SQL 日志
        if elapsed_ms >= self.slow_sql_ms {
            tracing::warn!("SLOW SQL [{}ms]: {}", elapsed_ms, sql);
        }
    }

    fn statement_error(&self, ctx: &FilterContext, _error: &DruidError) {
        self.ds_stat.lock().expect("stat lock poisoned").error_count += 1;
        if let Some(sql) = &ctx.sql {
            let mut stats = self.sql_stats.lock().expect("stat lock poisoned");
            if let Some(entry) = stats.get_mut(sql.as_str()) {
                entry.error_count += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stat_collection() {
        let filter = StatFilter::new("test-ds", 1000);
        let ctx = FilterContext::new("test").with_sql("SELECT 1");

        filter.statement_execute_before(&ctx).unwrap();
        filter.statement_execute_after(&ctx, 50, 1);
        filter.statement_execute_before(&ctx).unwrap();
        filter.statement_execute_after(&ctx, 200, 10);

        let stats = filter.get_sql_stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].execute_count, 2);
        assert_eq!(stats[0].max_time_ms, 200);
    }

    #[test]
    fn test_slow_sql_detection() {
        let filter = StatFilter::new("test-ds", 100);
        let ctx = FilterContext::new("test").with_sql("SELECT SLEEP(1)");
        filter.statement_execute_before(&ctx).unwrap();
        filter.statement_execute_after(&ctx, 500, 0);

        let slow = filter.get_slow_sql();
        assert_eq!(slow.len(), 1);
    }

    #[test]
    fn test_datasource_stat() {
        let filter = StatFilter::new("ds1", 1000);
        filter.connection_created(&FilterContext::new("ds1"));
        filter.connection_borrowed(&FilterContext::new("ds1"), 10);

        let stat = filter.get_datasource_stat();
        assert_eq!(stat.create_count, 1);
        assert_eq!(stat.borrow_count, 1);
    }
}
