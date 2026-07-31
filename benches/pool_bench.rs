use criterion::{Criterion, black_box, criterion_group, criterion_main};
use druid_core::DruidConfig;
use druid_pool::DruidDataSource;
use druid_pool::driver::{Connection, Driver};
use std::sync::atomic::{AtomicU64, Ordering};
use druid_core::DruidError;

#[derive(Debug, Clone)]
struct BenchConn { id: u64 }
impl BenchConn { fn new(id: u64) -> Self { BenchConn { id } } }

#[async_trait::async_trait]
impl Connection for BenchConn {
    async fn execute(&self, _: &str) -> Result<u64, DruidError> { Ok(1) }
    async fn query(&self, _: &str) -> Result<Vec<Vec<String>>, DruidError> { Ok(vec![]) }
    async fn close(&self) -> Result<(), DruidError> { Ok(()) }
    async fn ping(&self) -> Result<(), DruidError> { Ok(()) }
    fn connection_id(&self) -> u64 { self.id }
}

#[derive(Debug)]
struct BenchDriver { count: AtomicU64 }
impl BenchDriver { fn new() -> Self { BenchDriver { count: AtomicU64::new(0) } } }

#[async_trait::async_trait]
impl Driver for BenchDriver {
    type Connection = BenchConn;
    async fn connect(&self, _: &str, _: &str, _: &str, _: Option<std::time::Duration>) -> Result<BenchConn, DruidError> {
        let id = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(BenchConn::new(id))
    }
    fn name(&self) -> &'static str { "Bench" }
    async fn validate(&self, _: &BenchConn) -> Result<(), DruidError> { Ok(()) }
}

fn bench_pool_borrow_return(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("pool_borrow_return", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut config = DruidConfig::new("bench://local", "u", "p");
                config.initial_size = 4;
                config.max_active = 4;
                config.test_on_borrow = false;
                let ds = DruidDataSource::new(BenchDriver::new(), config);
                ds.init().await.unwrap();

                let g = ds.get_connection().await.unwrap();
                black_box(&g);
                drop(g);
            });
        })
    });
}

criterion_group!(benches, bench_pool_borrow_return);
criterion_main!(benches);
