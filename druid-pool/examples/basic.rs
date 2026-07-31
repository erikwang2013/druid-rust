//! Druid-Rust 基本使用示例

use druid_core::DruidConfig;
use druid_pool::DruidDataSource;

mod mock {
    use druid_core::DruidError;
    use druid_pool::driver::{Connection, Driver};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[derive(Debug, Clone)]
    pub struct MockConn {
        pub id: u64,
        pub closed: Arc<std::sync::Mutex<bool>>,
    }

    #[async_trait::async_trait]
    impl Connection for MockConn {
        async fn execute(&self, _: &str) -> Result<u64, DruidError> {
            Ok(1)
        }
        async fn query(&self, _: &str) -> Result<Vec<Vec<String>>, DruidError> {
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

    #[derive(Debug)]
    pub struct MockDriver {
        pub count: AtomicU64,
    }

    #[async_trait::async_trait]
    impl Driver for MockDriver {
        type Connection = MockConn;
        async fn connect(&self, _: &str, _: &str, _: &str) -> Result<MockConn, DruidError> {
            let id = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(MockConn {
                id,
                closed: Arc::new(std::sync::Mutex::new(false)),
            })
        }
        fn name(&self) -> &'static str {
            "Mock"
        }
        async fn validate(&self, _: &MockConn) -> Result<(), DruidError> {
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = DruidConfig::new("mock://localhost/test", "user", "pass");
    config.initial_size = 2;
    config.max_active = 10;

    let ds = DruidDataSource::new(
        mock::MockDriver {
            count: Default::default(),
        },
        config,
    );
    ds.init().await?;
    println!(
        "Pool: active={}, idle={}",
        ds.active_count(),
        ds.idle_count()
    );

    let guard = ds.get_connection().await?;
    println!(
        "Borrowed #{} (active={})",
        guard.connection_id(),
        ds.active_count()
    );
    drop(guard);
    println!("Returned (active={})", ds.active_count());

    ds.close().await?;
    println!("Closed");
    Ok(())
}
