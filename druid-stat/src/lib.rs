//! Druid-Rust 监控统计
//!
//! - [`StatFilter`]：实现 `Filter` trait，采集 SQL 耗时/行数/错误与连接池生命周期事件；
//! - [`SqlStat`] / [`DataSourceStat`]：SQL 级与数据源级聚合统计；
//! - [`metrics::PoolMetrics`]：连接池运行时指标（lock-free）。
//!
//! # active/idle 计数
//!
//! `StatFilter` 自身按生命周期事件维护 active/idle 计数，并区分「借出中的连接被关闭」
//! 与「空闲连接被关闭」两种情况。若通过 [`StatFilter::bind_metrics`] 绑定了池的
//! [`metrics::PoolMetrics`]，[`StatFilter::get_datasource_stat`] 的 active/idle
//! 直接读取池内权威计数，单一数据源，不会再漂移。

#![warn(missing_docs)]

pub mod metrics;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use druid_core::DruidError;
use druid_filter::{Filter, FilterContext};
use druid_util::string::truncate_sql;

use crate::metrics::PoolMetrics;

/// SQL 展示文本最大长度（按字符边界安全截断，超出追加 `...`）
const MAX_SQL_DISPLAY_LEN: usize = 200;
/// 慢 SQL 日志中 SQL 的最大长度
const MAX_SQL_LOG_LEN: usize = 500;
/// SQL 统计表默认容量上限
const DEFAULT_MAX_SQL_SIZE: usize = 1000;

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
            sql: truncate_sql(sql, MAX_SQL_DISPLAY_LEN),
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
    /// 当前活跃连接（绑定 PoolMetrics 后取自池内权威计数）
    pub active_count: usize,
    /// 当前空闲连接（绑定 PoolMetrics 后取自池内权威计数）
    pub idle_count: usize,
}

/// 取得（必要时创建）SQL 统计条目；表满时先按 `total_time_ms` 淘汰最小项
///
/// 仅在新 key 且已满时做一次 O(n) 扫描，命中已有 key 的热路径无额外开销。
fn upsert_sql_stat<'a>(
    stats: &'a mut HashMap<String, SqlStat>,
    sql: &str,
    max_size: usize,
) -> &'a mut SqlStat {
    if max_size > 0 && !stats.contains_key(sql) && stats.len() >= max_size {
        if let Some(min_key) = stats
            .iter()
            .min_by_key(|(_, v)| v.total_time_ms)
            .map(|(k, _)| k.clone())
        {
            stats.remove(&min_key);
        }
    }
    stats
        .entry(sql.to_string())
        .or_insert_with(|| SqlStat::new(sql))
}

/// StatFilter — SQL 监控统计 Filter
///
/// 实现 Filter trait，实时采集 SQL 执行统计和连接池指标。
/// SQL 统计表有容量上限（默认 1000），防止 ORM 动态 SQL 撑爆常驻内存。
pub struct StatFilter {
    sql_stats: Mutex<HashMap<String, SqlStat>>,
    ds_stat: Mutex<DataSourceStat>,
    /// 当前处于借出状态的连接 ID，用于区分连接关闭时该减 active 还是 idle
    borrowed: Mutex<HashSet<u64>>,
    slow_sql_ms: u64,
    max_sql_size: usize,
    bound_metrics: OnceLock<Arc<PoolMetrics>>,
}

impl StatFilter {
    /// 创建统计 Filter，`slow_sql_ms` 为慢 SQL 阈值（ms）
    pub fn new(name: &str, slow_sql_ms: u64) -> Self {
        StatFilter {
            sql_stats: Mutex::new(HashMap::new()),
            ds_stat: Mutex::new(DataSourceStat {
                name: name.to_string(),
                ..Default::default()
            }),
            borrowed: Mutex::new(HashSet::new()),
            slow_sql_ms,
            max_sql_size: DEFAULT_MAX_SQL_SIZE,
            bound_metrics: OnceLock::new(),
        }
    }

    /// 设置 SQL 统计表容量上限（默认 1000；0 表示不限制）
    ///
    /// 超限后插入新 SQL 时淘汰 `total_time_ms` 最小的一条（对应 Java Druid 的 `maxSqlSize`）。
    pub fn with_max_sql_size(mut self, max: usize) -> Self {
        self.max_sql_size = max;
        self
    }

    /// 绑定连接池指标，绑定后 [`Self::get_datasource_stat`] 的 active/idle 取自池内权威计数
    ///
    /// 返回 `false` 表示此前已绑定（保留首次绑定，不做替换）。
    pub fn bind_metrics(&self, metrics: Arc<PoolMetrics>) -> bool {
        self.bound_metrics.set(metrics).is_ok()
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

    /// 从已有统计切片中筛选慢 SQL（复用已取到的结果，避免重复加锁/克隆）
    pub fn get_slow_sql_from(&self, stats: &[SqlStat]) -> Vec<SqlStat> {
        stats
            .iter()
            .filter(|s| s.max_time_ms >= self.slow_sql_ms)
            .cloned()
            .collect()
    }

    /// 获取慢 SQL 列表
    pub fn get_slow_sql(&self) -> Vec<SqlStat> {
        self.get_slow_sql_from(&self.get_sql_stats())
    }

    /// 获取数据源级别统计
    ///
    /// 已绑定 [`PoolMetrics`] 时 active/idle 以池内计数为准。
    pub fn get_datasource_stat(&self) -> DataSourceStat {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned").clone();
        if let Some(m) = self.bound_metrics.get() {
            stat.active_count = m.active() as usize;
            stat.idle_count = m.idle() as usize;
        }
        stat
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

    fn connection_borrowed(&self, ctx: &FilterContext, wait_ms: u64) {
        if let Some(id) = ctx.connection_id {
            self.borrowed.lock().expect("stat lock poisoned").insert(id);
        }
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.borrow_count += 1;
        stat.total_wait_time_ms += wait_ms;
        stat.active_count += 1;
        stat.idle_count = stat.idle_count.saturating_sub(1);
    }

    fn connection_returned(&self, ctx: &FilterContext) {
        if let Some(id) = ctx.connection_id {
            self.borrowed
                .lock()
                .expect("stat lock poisoned")
                .remove(&id);
        }
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.return_count += 1;
        stat.active_count = stat.active_count.saturating_sub(1);
        stat.idle_count += 1;
    }

    fn connection_closed(&self, ctx: &FilterContext) {
        // 借出中的连接关闭（test_on_borrow 校验失败、池关闭时未归还的连接）→ 减 active；
        // 空闲连接关闭（驱逐/keepalive/过期）→ 减 idle。
        // 带 connection_id 时按借出集合精确区分；不带 id 时无法区分，按空闲处理。
        let was_borrowed = ctx.connection_id.is_some_and(|id| {
            self.borrowed
                .lock()
                .expect("stat lock poisoned")
                .remove(&id)
        });
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.destroy_count += 1;
        if was_borrowed {
            stat.active_count = stat.active_count.saturating_sub(1);
        } else {
            stat.idle_count = stat.idle_count.saturating_sub(1);
        }
    }

    fn statement_execute_before(&self, _ctx: &FilterContext) -> Result<(), DruidError> {
        let mut stat = self.ds_stat.lock().expect("stat lock poisoned");
        stat.execute_count += 1;
        Ok(())
    }

    fn statement_execute_after(&self, ctx: &FilterContext, elapsed_ms: u64, rows: u64) {
        let sql = ctx.sql.as_deref().unwrap_or("UNKNOWN");
        {
            let mut stats = self.sql_stats.lock().expect("stat lock poisoned");
            let entry = upsert_sql_stat(&mut stats, sql, self.max_sql_size);
            entry.execute_count += 1;
            entry.total_time_ms += elapsed_ms;
            entry.max_time_ms = entry.max_time_ms.max(elapsed_ms);
            entry.rows_read += rows;
            entry.last_execute_time =
                Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
        }
        // 慢 SQL 日志在锁外打印：WARN 可能阻塞在文件/网络 sink 上，不能拖住全局统计锁
        if elapsed_ms >= self.slow_sql_ms {
            tracing::warn!(
                "SLOW SQL [{}ms]: {}",
                elapsed_ms,
                truncate_sql(sql, MAX_SQL_LOG_LEN)
            );
        }
    }

    fn statement_error(&self, ctx: &FilterContext, _error: &DruidError) {
        self.ds_stat.lock().expect("stat lock poisoned").error_count += 1;
        if let Some(sql) = &ctx.sql {
            let mut stats = self.sql_stats.lock().expect("stat lock poisoned");
            // 从未成功执行过的 SQL 也需记录错误数，缺条目时创建
            let entry = upsert_sql_stat(&mut stats, sql, self.max_sql_size);
            entry.error_count += 1;
        }
    }
}

#[cfg(test)]
mod tests;
