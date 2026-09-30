use super::*;
use druid_pool::driver::{Connection, Driver};
use std::sync::atomic::AtomicU64;

#[derive(Debug, Clone)]
struct MockHaConn {
    id: u64,
    closed: Arc<Mutex<bool>>,
}
impl MockHaConn {
    fn new(id: u64) -> Self {
        MockHaConn {
            id,
            closed: Arc::new(Mutex::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl Connection for MockHaConn {
    async fn execute(&self, _: &str) -> Result<u64, DruidError> {
        Ok(1)
    }
    async fn query(&self, _: &str) -> Result<Vec<Vec<String>>, DruidError> {
        Ok(vec![])
    }
    async fn close(&self) -> Result<(), DruidError> {
        *self.closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
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
struct MockHaDriver {
    counter: Arc<AtomicU64>,
}
impl MockHaDriver {
    fn new() -> Self {
        MockHaDriver {
            counter: Arc::new(AtomicU64::new(0)),
        }
    }
    fn with_counter(counter: Arc<AtomicU64>) -> Self {
        MockHaDriver { counter }
    }
}

#[async_trait::async_trait]
impl Driver for MockHaDriver {
    type Connection = MockHaConn;
    async fn connect(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<std::time::Duration>,
    ) -> Result<MockHaConn, DruidError> {
        let id = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(MockHaConn::new(id))
    }
    fn name(&self) -> &'static str {
        "MockHaDriver"
    }
    async fn validate(&self, _: &MockHaConn) -> Result<(), DruidError> {
        Ok(())
    }
}

/// 连接总是失败的驱动（记录 connect 尝试次数）
struct FailingHaDriver {
    attempts: Arc<AtomicU64>,
}

impl FailingHaDriver {
    fn new() -> Self {
        FailingHaDriver {
            attempts: Arc::new(AtomicU64::new(0)),
        }
    }
    fn with_counter(attempts: Arc<AtomicU64>) -> Self {
        FailingHaDriver { attempts }
    }
}

#[async_trait::async_trait]
impl Driver for FailingHaDriver {
    type Connection = MockHaConn;
    async fn connect(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<std::time::Duration>,
    ) -> Result<MockHaConn, DruidError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(DruidError::Database("backend down".into()))
    }
    fn name(&self) -> &'static str {
        "FailingHaDriver"
    }
    async fn validate(&self, _: &MockHaConn) -> Result<(), DruidError> {
        Ok(())
    }
}

/// 可挂在同一 HA 里的混合驱动：hang=true 连接永久挂起，否则连接总是失败
struct MixedHaDriver {
    hang: bool,
}

#[async_trait::async_trait]
impl Driver for MixedHaDriver {
    type Connection = MockHaConn;
    async fn connect(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<std::time::Duration>,
    ) -> Result<MockHaConn, DruidError> {
        if self.hang {
            return std::future::pending::<Result<MockHaConn, DruidError>>().await;
        }
        Err(DruidError::Database("backend down".into()))
    }
    fn name(&self) -> &'static str {
        "MixedHaDriver"
    }
    async fn validate(&self, _: &MockHaConn) -> Result<(), DruidError> {
        Ok(())
    }
}

/// 首次 connect 永久挂起，之后正常建连（用于制造一次被取消的探测）
struct HangFirstDriver {
    calls: AtomicU64,
}

#[async_trait::async_trait]
impl Driver for HangFirstDriver {
    type Connection = MockHaConn;
    async fn connect(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<std::time::Duration>,
    ) -> Result<MockHaConn, DruidError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return std::future::pending::<Result<MockHaConn, DruidError>>().await;
        }
        Ok(MockHaConn::new(1))
    }
    fn name(&self) -> &'static str {
        "HangFirstDriver"
    }
    async fn validate(&self, _: &MockHaConn) -> Result<(), DruidError> {
        Ok(())
    }
}

fn ha_cfg(url: &str) -> druid_core::DruidConfig {
    let mut c = druid_core::DruidConfig::new(url, "u", "p");
    c.initial_size = 0;
    c.max_active = 4;
    c.test_on_borrow = false;
    c.time_between_eviction_runs_ms = 0;
    c
}

#[tokio::test]
async fn test_ha_round_robin() {
    let mut ha = HighAvailableDataSource::new();
    let mut cfg1 = druid_core::DruidConfig::new("mock://n1", "u", "p");
    cfg1.initial_size = 0;
    cfg1.max_active = 2;
    cfg1.test_on_borrow = false;
    let ds1 = DruidDataSource::new(MockHaDriver::new(), cfg1);
    let mut cfg2 = druid_core::DruidConfig::new("mock://n2", "u", "p");
    cfg2.initial_size = 0;
    cfg2.max_active = 2;
    cfg2.test_on_borrow = false;
    let ds2 = DruidDataSource::new(MockHaDriver::new(), cfg2);
    let _ = ds1.init().await;
    let _ = ds2.init().await;
    ha.add_node("node-1", ds1, 1);
    ha.add_node("node-2", ds2, 1);

    let result = ha.get_datasource().await;
    assert!(result.is_ok());
    assert_eq!(ha.active_count(), 2);
    assert_eq!(ha.node_count(), 2);
}

#[tokio::test]
async fn test_mark_down_up() {
    let mut ha = HighAvailableDataSource::new();
    let mut cfg = druid_core::DruidConfig::new("mock://n1", "u", "p");
    cfg.initial_size = 0;
    cfg.max_active = 1;
    cfg.test_on_borrow = false;
    let ds = DruidDataSource::new(MockHaDriver::new(), cfg);
    let _ = ds.init().await;
    ha.add_node("node-1", ds, 1);

    assert_eq!(ha.node_count(), 1);
    assert_eq!(ha.active_count(), 1);

    ha.mark_down("node-1");
    assert_eq!(ha.active_count(), 0);

    ha.mark_up("node-1");
    assert_eq!(ha.active_count(), 1);

    // mark non-existent node should not panic
    ha.mark_down("node-x");
    ha.mark_up("node-x");
    assert_eq!(ha.node_count(), 1);
}

#[tokio::test]
async fn test_ha_no_active_node_returns_error() {
    let ha = HighAvailableDataSource::<MockHaDriver>::new();
    let err = ha.get_datasource().await.err().unwrap();
    assert!(err.to_string().contains("no active"));
    assert_eq!(ha.active_count(), 0);
    assert!(ha.node_names().is_empty());
}

#[tokio::test]
async fn test_ha_weighted_round_robin_distribution() {
    let c1 = Arc::new(AtomicU64::new(0));
    let c2 = Arc::new(AtomicU64::new(0));
    let mut ha = HighAvailableDataSource::new();
    let ds1 = DruidDataSource::new(MockHaDriver::with_counter(c1.clone()), ha_cfg("mock://n1"));
    let ds2 = DruidDataSource::new(MockHaDriver::with_counter(c2.clone()), ha_cfg("mock://n2"));
    ds1.init().await.unwrap();
    ds2.init().await.unwrap();
    ha.add_node("a", ds1, 2);
    ha.add_node("b", ds2, 1);
    assert_eq!(ha.node_names(), vec!["a", "b"]);

    // 权重 2:1，6 次借用 → a 4 次、b 2 次（借用期间保持 guard 不归还，强制新建连接）
    let mut guards: Vec<_> = Vec::new();
    for _ in 0..6 {
        let ds = ha.get_datasource().await.unwrap();
        guards.push(ds.get_connection().await.unwrap());
    }
    assert_eq!(c1.load(Ordering::SeqCst), 4);
    assert_eq!(c2.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_ha_zero_weight_no_panic() {
    let mut ha = HighAvailableDataSource::new();
    let ds = DruidDataSource::new(MockHaDriver::new(), ha_cfg("mock://n1"));
    ds.init().await.unwrap();
    ha.add_node("only", ds, 0); // 修复前：total_weight=0 取模除零 panic
    let ds = ha.get_datasource().await.unwrap();
    let _g = ds.get_connection().await.unwrap();
}

#[tokio::test]
async fn test_ha_health_check_recovers_down_node() {
    let mut ha = HighAvailableDataSource::new(); // success_threshold 默认 2
    let ds = DruidDataSource::new(MockHaDriver::new(), ha_cfg("mock://n1"));
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    ha.mark_down("n1");
    assert_eq!(ha.active_count(), 0);

    let ha = Arc::new(ha);
    ha.run_health_check().await; // 第 1 次成功：未达 success_threshold，仍保持 Down
    assert_eq!(ha.active_count(), 0);
    ha.run_health_check().await; // 连续第 2 次成功 → 恢复 Active
    assert_eq!(ha.active_count(), 1);
}

#[tokio::test]
async fn test_ha_health_check_keeps_down_node_down() {
    let mut ha = HighAvailableDataSource::new();
    let ds = DruidDataSource::new(FailingHaDriver::new(), ha_cfg("mock://n1"));
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    ha.mark_down("n1");

    let ha = Arc::new(ha);
    ha.run_health_check().await; // 探测失败 → 保持 Down
    assert_eq!(ha.active_count(), 0);
}

#[tokio::test]
async fn test_ha_health_check_marks_failing_active_down() {
    // 连续失败达到 failure_threshold 才摘除，单次失败不摘除
    let mut ha = HighAvailableDataSource::new();
    ha.set_failure_threshold(2);
    let ds = DruidDataSource::new(FailingHaDriver::new(), ha_cfg("mock://n1"));
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    assert_eq!(ha.active_count(), 1);

    let ha = Arc::new(ha);
    ha.run_health_check().await; // 第 1 次失败 → 仍在阈值内
    assert_eq!(ha.active_count(), 1);
    ha.run_health_check().await; // 连续第 2 次失败 → 标记 Down
    assert_eq!(ha.active_count(), 0);
}

/// 回归测试：探测被 timeout/select! 取消后，节点不得卡在中间状态。
///
/// 修复前 Down 节点先置为 Testing 再探测，取消恰好落在探测的 await 上时
/// 节点永久停在 Testing，之后每轮巡检都跳过它 → 健康节点被永久摘除。
#[tokio::test]
async fn test_cancelled_probe_does_not_strand_down_node() {
    let mut ha = HighAvailableDataSource::new();
    ha.set_success_threshold(1);
    let ds = DruidDataSource::new(
        HangFirstDriver {
            calls: AtomicU64::new(0),
        },
        ha_cfg("mock://hang"),
    );
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    ha.mark_down("n1");
    let ha = Arc::new(ha);

    // 探测永久挂起，调用方（异步里最常见的用法）用 timeout 取消整个巡检
    let cancelled = tokio::time::timeout(Duration::from_millis(50), ha.run_health_check()).await;
    assert!(cancelled.is_err(), "挂起的探测应被取消");
    assert_eq!(ha.active_count(), 0); // 仍为 Down，未被冻结

    // 下一轮探测成功即恢复，说明节点没有被卡死
    ha.run_health_check().await;
    assert_eq!(ha.active_count(), 1);
}

/// 回归测试：单个节点挂住不得阻塞整轮巡检。
///
/// 修复前巡检串行且无单点超时，一个节点卡死会让整个巡检任务永久挂起，
/// 其它节点的宕机/恢复全部检测不到。
#[tokio::test]
async fn test_hung_node_does_not_block_others() {
    let mut ha = HighAvailableDataSource::new();
    ha.set_probe_timeout(Duration::from_millis(50));
    let ds_hang = DruidDataSource::new(MixedHaDriver { hang: true }, ha_cfg("mock://hang"));
    let ds_fail = DruidDataSource::new(MixedHaDriver { hang: false }, ha_cfg("mock://fail"));
    ds_hang.init().await.unwrap();
    ds_fail.init().await.unwrap();
    ha.add_node("hang", ds_hang, 1);
    ha.add_node("fail", ds_fail, 1); // 排在挂起节点之后，必须仍能被探测到
    let ha = Arc::new(ha);

    // 默认 failure_threshold=3：连续 3 轮失败后两个节点都应被摘除
    tokio::time::timeout(Duration::from_secs(2), async {
        for _ in 0..3 {
            ha.run_health_check().await;
        }
    })
    .await
    .expect("巡检被挂起节点卡死");

    assert_eq!(
        ha.active_count(),
        0,
        "挂起节点超时应计入失败，后续节点仍应被检测"
    );
}

/// 故障节点进入退避窗口后不再每轮无脑重试
#[tokio::test]
async fn test_down_node_backoff_skips_retry() {
    let attempts = Arc::new(AtomicU64::new(0));
    let mut ha = HighAvailableDataSource::new();
    ha.set_failure_threshold(1);
    ha.set_check_interval(Duration::from_secs(60)); // 退避基数 = check_interval
    let ds = DruidDataSource::new(
        FailingHaDriver::with_counter(attempts.clone()),
        ha_cfg("mock://n1"),
    );
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    let ha = Arc::new(ha);

    ha.run_health_check().await; // 第 1 次失败 → Down，退避 60s
    assert_eq!(ha.active_count(), 0);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    ha.run_health_check().await; // 退避窗口内 → 跳过探测
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "退避窗口内不应重试");
}

/// 健康检查循环必须能停止，并且停止后不再持有 HA 对象的 Arc
#[tokio::test]
async fn test_health_check_loop_shutdown() {
    let mut ha = HighAvailableDataSource::new();
    ha.set_check_interval(Duration::from_millis(10));
    let ds = DruidDataSource::new(MockHaDriver::new(), ha_cfg("mock://n1"));
    ds.init().await.unwrap();
    ha.add_node("n1", ds, 1);
    let ha = Arc::new(ha);

    let weak = Arc::downgrade(&ha);
    let handle = ha.spawn_health_check_loop();
    tokio::time::sleep(Duration::from_millis(30)).await;

    ha.shutdown();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("shutdown 后健康检查循环应退出")
        .unwrap();

    drop(ha);
    assert!(weak.upgrade().is_none(), "循环退出后 HA 对象应可被释放");
}
