use druid_core::DruidError;
use std::fmt::Debug;

/// 数据库驱动接口 — Rust 版 JDBC 抽象
///
/// 各数据库实现此 trait 以提供真实的数据库连接。
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    type Connection: Connection;

    /// 创建新连接
    async fn connect(&self, url: &str, username: &str, password: &str) -> Result<Self::Connection, DruidError>;

    /// 驱动名称
    fn name(&self) -> &'static str;

    /// 验证连接是否有效
    async fn validate(&self, conn: &Self::Connection) -> Result<(), DruidError>;
}

/// 数据库连接 trait
#[async_trait::async_trait]
pub trait Connection: Send + Sync + Debug + 'static {
    /// 执行 SQL 查询
    async fn execute(&self, sql: &str) -> Result<u64, DruidError>;

    /// 执行查询并获取结果（简化的 rows）
    async fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, DruidError>;

    /// 关闭连接
    async fn close(&self) -> Result<(), DruidError>;

    /// 检查连接是否存活
    async fn ping(&self) -> Result<(), DruidError>;

    /// 获取连接 ID
    fn connection_id(&self) -> u64;
}
