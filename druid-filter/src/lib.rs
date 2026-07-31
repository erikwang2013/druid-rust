//! Druid Filter-Chain 可插拔架构
//!
//! 提供 Filter trait、默认适配器和责任链实现，
//! 用于在连接池操作中插入自定义行为（监控、防火墙、日志等）。

pub mod adapter;
pub mod chain;
pub mod manager;

pub use adapter::FilterAdapter;
pub use chain::FilterChain;

use druid_core::DruidError;

/// 连接池事件上下文
#[derive(Debug, Clone)]
pub struct FilterContext {
    /// 数据源名称
    pub data_source_name: String,
    /// 数据库类型
    pub db_type: Option<String>,
    /// SQL 文本（如果适用）
    pub sql: Option<String>,
    /// 连接 ID
    pub connection_id: Option<u64>,
    /// 语句 ID
    pub statement_id: Option<u64>,
}

impl FilterContext {
    pub fn new(name: &str) -> Self {
        FilterContext {
            data_source_name: name.to_string(),
            db_type: None,
            sql: None,
            connection_id: None,
            statement_id: None,
        }
    }

    pub fn with_sql(mut self, sql: &str) -> Self {
        self.sql = Some(sql.to_string());
        self
    }

    pub fn with_connection(mut self, id: u64) -> Self {
        self.connection_id = Some(id);
        self
    }

    pub fn with_statement(mut self, id: u64) -> Self {
        self.statement_id = Some(id);
        self
    }
}

/// Filter 核心 trait — 连接池生命周期钩子
///
/// 每个方法都有默认空实现，Filter 只需覆写关心的钩子。
/// 参考 Druid Java Filter 接口设计。
pub trait Filter: Send + Sync {
    /// Filter 初始化
    fn init(&mut self) -> Result<(), DruidError> {
        Ok(())
    }

    /// Filter 销毁
    fn destroy(&mut self) {}

    /// 获取 Filter 名称
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    // ── 连接生命周期 ──

    /// 连接创建时
    fn connection_created(&self, _ctx: &FilterContext) {}
    /// 连接从池中借出前
    fn connection_borrow_before(&self, _ctx: &FilterContext) {}
    /// 连接从池中借出后
    fn connection_borrowed(&self, _ctx: &FilterContext, _wait_ms: u64) {}
    /// 连接归还到池前
    fn connection_return_before(&self, _ctx: &FilterContext) {}
    /// 连接归还到池后
    fn connection_returned(&self, _ctx: &FilterContext) {}
    /// 连接关闭时（物理关闭）
    fn connection_closed(&self, _ctx: &FilterContext) {}
    /// 连接发生错误时
    fn connection_error(&self, _ctx: &FilterContext, _error: &DruidError) {}

    // ── Statement 生命周期 ──

    /// Statement 创建时
    fn statement_created(&self, _ctx: &FilterContext) {}
    /// SQL 执行前
    fn statement_execute_before(&self, _ctx: &FilterContext) -> Result<(), DruidError> {
        Ok(())
    }
    /// SQL 执行后
    fn statement_execute_after(&self, _ctx: &FilterContext, _elapsed_ms: u64, _rows: u64) {}
    /// Statement 关闭时
    fn statement_closed(&self, _ctx: &FilterContext) {}
    /// Statement 错误
    fn statement_error(&self, _ctx: &FilterContext, _error: &DruidError) {}

    // ── ResultSet 生命周期 ──

    /// ResultSet 打开时
    fn resultset_open(&self, _ctx: &FilterContext) {}
    /// ResultSet 关闭时
    fn resultset_closed(&self, _ctx: &FilterContext, _rows_read: u64) {}

    // ── 数据源生命周期 ──

    /// 数据源初始化完成
    fn data_source_inited(&self, _ctx: &FilterContext) {}
}

/// 可克隆的 Filter 包装
impl std::fmt::Debug for dyn Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filter")
            .field("name", &self.name())
            .finish()
    }
}
