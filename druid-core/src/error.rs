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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_display_messages() {
        assert_eq!(
            DruidError::Pool("full".into()).to_string(),
            "pool error: full"
        );
        assert_eq!(
            DruidError::SqlParse("bad token".into()).to_string(),
            "sql parse error: bad token"
        );
        assert_eq!(
            DruidError::Wall("blocked".into()).to_string(),
            "wall error: blocked"
        );
        assert_eq!(
            DruidError::Config("missing url".into()).to_string(),
            "config error: missing url"
        );
        assert_eq!(
            DruidError::Database("conn refused".into()).to_string(),
            "database error: conn refused"
        );
    }

    #[test]
    fn test_display_empty_message() {
        assert_eq!(DruidError::Pool(String::new()).to_string(), "pool error: ");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionRefused, "socket down");
        let err: DruidError = io_err.into();
        assert_eq!(err.to_string(), "io error: socket down");
        assert!(matches!(err, DruidError::Io(_)));
    }

    #[test]
    fn test_from_io_error_source() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let err = DruidError::from(io_err);
        let src = std::error::Error::source(&err);
        assert!(src.is_some());
        assert_eq!(src.unwrap().to_string(), "no such file");
    }
}
