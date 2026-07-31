use thiserror::Error;

/// Druid 统一错误类型
#[derive(Error, Debug)]
pub enum DruidError {
    /// 连接池相关错误
    #[error("pool error: {0}")]
    Pool(String),

    /// SQL 解析错误
    #[error("sql parse error: {0}")]
    SqlParse(String),

    /// SQL 防火墙错误
    #[error("wall error: {0}")]
    Wall(String),

    /// 配置错误
    #[error("config error: {0}")]
    Config(String),

    /// 数据库驱动错误
    #[error("database error: {0}")]
    Database(String),

    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
