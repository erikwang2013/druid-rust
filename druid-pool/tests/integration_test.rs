use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use druid_core::{DruidConfig, DruidError};
use druid_pool::driver::{Connection, Driver};
use druid_pool::DruidDataSource;

/// Mock 数据库驱动
#[derive(Debug)]
struct MockDriver {
    connect_latency: Duration,
    connect_count: AtomicU64,
    validate_ok: bool,
}

impl MockDriver {
    fn new() -> Self {
        MockDriver {
            connect_latency: Duration::from_millis(1),
            connect_count: AtomicU64::new(0),
            validate_ok: true,
        }
    }
}

/// Mock 连接
#[derive(Debug, Clone)]
struct MockConnection {
    id: u64,
    closed: std::sync::Arc<Mutex<bool>>,
}

impl MockConnection {
    fn new(id: u64) -> Self {
        MockConnection {
            id,
            closed: std::sync::Arc::new(Mutex::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl Driver for MockDriver {
    type Connection = MockConnection;

    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
    ) -> Result<MockConnection, DruidError> {
        tokio::time::sleep(self.connect_latency).await;
        let id = self.connect_count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(MockConnection::new(id))
    }

    fn name(&self) -> &'static str {
        "MockDriver"
    }

    async fn validate(&self, _conn: &MockConnection) -> Result<(), DruidError> {
        if self.validate_ok {
            Ok(())
        } else {
            Err(DruidError::Pool("validation failed".into()))
        }
    }
}

#[async_trait::async_trait]
impl Connection for MockConnection {
    async fn execute(&self, _sql: &str) -> Result<u64, DruidError> {
        Ok(1)
    }
    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        Ok(vec![])
    }
    async fn close(&self) -> Result<(), DruidError> {
        *self.closed.lock().unwrap() = true;
        Ok(())
    }
    async fn ping(&self) -> Result<(), DruidError> {
        Ok(())
    }
    fn connection_id(&self) -> u64 {
        self.id
    }
}

#[tokio::test]
async fn test_datasource_init() {
    let config = DruidConfig::new("mock://localhost/test", "user", "pass");
    let driver = MockDriver::new();
    let ds = DruidDataSource::new(driver, config);
    assert!(ds.init().await.is_ok());
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.idle_count(), 0); // initial_size = 0
}

#[tokio::test]
async fn test_datasource_init_with_pool() {
    let mut config = DruidConfig::new("mock://localhost/test", "user", "pass");
    config.initial_size = 2;
    config.max_active = 4;
    let ds = DruidDataSource::new(MockDriver::new(), config);
    ds.init().await.unwrap();
    assert_eq!(ds.idle_count(), 2);
}

#[tokio::test]
async fn test_get_connection() {
    let mut config = DruidConfig::new("mock://localhost/test", "user", "pass");
    config.initial_size = 1;
    config.max_active = 4;
    config.test_on_borrow = false;
    let ds = DruidDataSource::new(MockDriver::new(), config);
    ds.init().await.unwrap();

    let guard = ds.get_connection().await.unwrap();
    assert_eq!(ds.active_count(), 1);
    assert_eq!(ds.idle_count(), 0);
    drop(guard);
    // Drop 后连接归还到空闲队列
    assert_eq!(ds.active_count(), 0);
}

#[tokio::test]
async fn test_max_active_limit() {
    let mut config = DruidConfig::new("mock://localhost/test", "user", "pass");
    config.max_active = 2;
    config.initial_size = 0;
    config.max_wait_ms = 1000;
    config.test_on_borrow = false;
    let ds = std::sync::Arc::new(DruidDataSource::new(MockDriver::new(), config));
    ds.init().await.unwrap();

    // 获取 2 个连接填满池
    let g1 = ds.get_connection().await.unwrap();
    let g2 = ds.get_connection().await.unwrap();
    assert_eq!(ds.active_count(), 2);

    // 第 3 个应该超时
    let ds2 = ds.clone();
    let result = tokio::time::timeout(Duration::from_millis(1500), ds2.get_connection()).await;
    assert!(result.is_err() || result.unwrap().is_err());

    drop(g1);
    drop(g2);
}

#[tokio::test]
async fn test_close_datasource() {
    let config = DruidConfig::new("mock://localhost/test", "user", "pass");
    let ds = DruidDataSource::new(MockDriver::new(), config);
    ds.init().await.unwrap();
    assert!(ds.close().await.is_ok());
}
