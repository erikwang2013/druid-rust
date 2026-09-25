use druid_core::DruidError;
use std::fmt::Debug;
use std::time::Duration;

/// 数据库驱动接口 — Rust 版 JDBC 抽象
///
/// 各数据库实现此 trait 以提供真实的数据库连接。
#[async_trait::async_trait]
pub trait Driver: Send + Sync + 'static {
    type Connection: Connection;

    /// 创建新连接
    async fn connect(
        &self,
        url: &str,
        username: &str,
        password: &str,
        timeout: Option<Duration>,
    ) -> Result<Self::Connection, DruidError>;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MockConn {
        id: u64,
    }

    #[async_trait::async_trait]
    impl Connection for MockConn {
        async fn execute(&self, _: &str) -> Result<u64, DruidError> {
            Ok(7)
        }
        async fn query(&self, _: &str) -> Result<Vec<Vec<String>>, DruidError> {
            Ok(vec![vec!["a".into()]])
        }
        async fn close(&self) -> Result<(), DruidError> {
            Ok(())
        }
        async fn ping(&self) -> Result<(), DruidError> {
            Ok(())
        }
        fn connection_id(&self) -> u64 {
            self.id
        }
    }

    struct MockDriver;

    #[async_trait::async_trait]
    impl Driver for MockDriver {
        type Connection = MockConn;
        async fn connect(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<Duration>,
        ) -> Result<MockConn, DruidError> {
            Ok(MockConn { id: 42 })
        }
        fn name(&self) -> &'static str {
            "MockDriver"
        }
        async fn validate(&self, _: &MockConn) -> Result<(), DruidError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_driver_connection_contract() {
        let driver = MockDriver;
        let conn = driver
            .connect("url", "u", "p", Some(Duration::from_secs(1)))
            .await
            .unwrap();
        assert_eq!(driver.name(), "MockDriver");
        assert_eq!(conn.connection_id(), 42);
        assert!(driver.validate(&conn).await.is_ok());
        assert!(conn.ping().await.is_ok());
        assert_eq!(conn.execute("SELECT 1").await.unwrap(), 7);
        assert_eq!(
            conn.query("SELECT 1").await.unwrap(),
            vec![vec!["a".to_string()]]
        );
        assert!(conn.close().await.is_ok());
    }

    #[tokio::test]
    async fn test_connect_error_propagates() {
        struct FailingDriver;
        #[async_trait::async_trait]
        impl Driver for FailingDriver {
            type Connection = MockConn;
            async fn connect(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: Option<Duration>,
            ) -> Result<MockConn, DruidError> {
                Err(DruidError::Pool("connect refused".into()))
            }
            fn name(&self) -> &'static str {
                "FailingDriver"
            }
            async fn validate(&self, _: &MockConn) -> Result<(), DruidError> {
                Ok(())
            }
        }
        let err = FailingDriver
            .connect("url", "u", "p", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("connect refused"));
    }
}
