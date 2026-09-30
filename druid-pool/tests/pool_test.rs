//! 连接池生命周期测试：借用归还、校验、驱逐、KeepAlive、关闭与销毁

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::{cfg, wait_until, MockDriver};
use druid_core::DruidError;
use druid_pool::driver::Connection;
use druid_pool::DruidDataSource;

#[tokio::test]
async fn test_init_twice_fails() {
    let ds = DruidDataSource::new(MockDriver::new(), cfg("mock://a"));
    assert!(ds.init().await.is_ok());
    let err = ds.init().await.unwrap_err();
    assert!(err.to_string().contains("already initialized"));
}

#[tokio::test]
async fn test_init_after_close_fails() {
    let ds = DruidDataSource::new(MockDriver::new(), cfg("mock://a"));
    ds.init().await.unwrap();
    ds.close().await.unwrap();
    // 关闭后 init 不得往已关闭池塞连接、也不得启动无人可停的循环
    let err = ds.init().await.unwrap_err();
    assert!(err.to_string().contains("closed"), "实际: {err}");
    assert_eq!(ds.idle_count(), 0);
}

#[tokio::test]
async fn test_get_connection_before_init_fails() {
    let ds = DruidDataSource::new(MockDriver::new(), cfg("mock://a"));
    let err = ds.get_connection().await.err().unwrap();
    assert!(err.to_string().contains("not initialized"));
}

#[tokio::test]
async fn test_borrow_return_cycle_and_metrics() {
    let ds = DruidDataSource::new(MockDriver::new(), cfg("mock://a"));
    ds.init().await.unwrap();

    let g1 = ds.get_connection().await.unwrap();
    assert_eq!(ds.active_count(), 1);
    assert_eq!(ds.idle_count(), 0);
    drop(g1);
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.idle_count(), 1);

    // 第二次借用命中空闲池：不新建连接
    let g2 = ds.get_connection().await.unwrap();
    assert_eq!(ds.metrics().create_count(), 1);
    assert_eq!(ds.metrics().borrow_count(), 2);
    assert_eq!(ds.metrics().cache_hit_count(), 1);
    drop(g2);
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.idle_count(), 1);
}

#[tokio::test]
async fn test_max_lifetime_expiry_on_borrow() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.max_lifetime_ms = 20;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    assert_eq!(ds.idle_count(), 1);

    // 空闲超过 max_lifetime 后借用 → 过期连接被销毁并新建
    tokio::time::sleep(Duration::from_millis(60)).await;
    let g = ds.get_connection().await.unwrap();
    assert_eq!(ds.metrics().create_count(), 2);
    assert_eq!(ds.metrics().destroy_count(), 1);
    assert_eq!(ds.metrics().cache_hit_count(), 0);
    drop(g);
    wait_until("过期连接被物理关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
}

/// max_lifetime 按物理创建时刻判定：频繁借还的热点连接同样会到期回收
#[tokio::test]
async fn test_hot_connection_recycled_by_created_at() {
    let mut c = cfg("mock://a");
    c.max_lifetime_ms = 50;
    let ds = DruidDataSource::new(MockDriver::new(), c);
    ds.init().await.unwrap();

    // 借还之间只等 20ms：最近使用时刻一直在刷新，但创建时刻没变
    for _ in 0..4 {
        let g = ds.get_connection().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(g);
    }
    assert!(
        ds.metrics().create_count() >= 2,
        "热点连接应因超过 max_lifetime 被回收重建，实际 create={}",
        ds.metrics().create_count()
    );
    assert!(ds.metrics().destroy_count() >= 1);
}

#[tokio::test]
async fn test_test_on_borrow_failure_closes_connection() {
    let driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.test_on_borrow = true; // 本测试需要借用校验
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    assert!(ds.get_connection().await.is_err());
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0);
    assert_eq!(ds.metrics().destroy_count(), 1);
    // 新建连接校验失败也必须被 close
    wait_until("校验失败的连接被关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(ds.idle_count(), 0);
}

#[tokio::test]
async fn test_test_on_return_failure_destroys_connection() {
    let driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    drop(g); // 归还校验失败 → 连接被销毁而不是回到空闲池
    wait_until("归还校验失败的连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    assert_eq!(ds.idle_count(), 0);
    assert_eq!(ds.active_count(), 0);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_close_closes_idle_connections_and_rejects_borrows() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 2;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    ds.close().await.unwrap();
    assert_eq!(ds.idle_count(), 0);
    assert_eq!(closed.load(Ordering::SeqCst), 2); // 空闲连接同步关闭

    let err = ds.get_connection().await.err().unwrap();
    assert!(err.to_string().contains("closed"));
}

#[tokio::test]
async fn test_guard_dropped_after_close_closes_connection() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    ds.close().await.unwrap();
    drop(g); // close 之后归还 → 连接被关闭而非回池
    wait_until("归还的连接被关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(ds.idle_count(), 0);
}

#[tokio::test]
async fn test_eviction_loop_evicts_idle_connections() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 2;
    c.time_between_eviction_runs_ms = 20;
    c.max_evictable_idle_time_ms = 1;
    c.min_idle = 0;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    assert_eq!(ds.idle_count(), 2);

    wait_until("驱逐循环清空空闲连接", || ds.idle_count() == 0).await;
    assert!(ds.metrics().destroy_count() >= 2);
    assert!(closed.load(Ordering::SeqCst) >= 2);
    ds.close().await.unwrap(); // 终止后台循环
}

#[tokio::test]
async fn test_keepalive_evicts_invalid_idle_connections() {
    let driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 20;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    assert_eq!(ds.idle_count(), 1);

    wait_until("KeepAlive 驱逐无效空闲连接", || {
        ds.idle_count() == 0
    })
    .await;
    // 驱逐必须同步更新指标并通告 filter chain
    assert_eq!(ds.metrics().idle(), 0);
    assert!(ds.metrics().destroy_count() >= 1);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    ds.close().await.unwrap();
}

/// KeepAlive 校验在途的连接**不可被业务借走**：与借用路径走同一套状态语义
/// （取许可 → 计入 active → 摘出 idle）。业务借用只能拿到另一条物理连接，
/// 校验失败只销毁被校验的那条，业务连接完好可用。
#[tokio::test]
async fn test_keepalive_validating_connection_is_not_borrowable() {
    let mut driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst);
    driver.validate_latency = Duration::from_millis(400); // 拉长校验窗口：给「观测摘出」留足余量，避免调度抖动变 flaky
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 20;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    // init 回填 1 条；此刻 KeepAlive 首轮 tick 可能已把它摘出校验，
    // 故只断言「池内共 1 条物理连接」（单锁快照，两种状态都成立）
    assert_eq!(ds.pool_size(), 1);

    // 等 KeepAlive 把唯一那条空闲连接摘出并进入 400ms 校验：
    // 摘出期间它计入 active、不再出现在 idle（借用方与 close() 都看得见）
    wait_until("KeepAlive 摘出空闲连接", || {
        ds.active_count() == 1 && ds.idle_count() == 0
    })
    .await;

    // 校验仍在途时借用：拿到的必须是**另一条**物理连接（init 建成的是 1 号）
    let g = ds.get_connection().await.unwrap();
    assert_ne!(
        g.connection().connection_id(),
        1,
        "业务借用拿到了正在被 KeepAlive 校验的连接"
    );

    // 校验失败：被校验的那条被销毁，业务连接完好可用
    wait_until("校验失败的空闲连接被销毁", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(
        !g.connection().closed.load(Ordering::SeqCst),
        "业务正在使用的连接不能被 KeepAlive 关闭"
    );
    assert!(g.execute("SELECT 1").await.is_ok()); // 连接仍然可用

    // 持着业务连接关闭数据源：归还时发现池已关闭 → 直接物理销毁（不入 idle）
    ds.close().await.unwrap();
    drop(g);
    wait_until("归还到已关闭池的连接应被物理销毁", || {
        closed.load(Ordering::SeqCst) == 2
    })
    .await;
}

/// 未显式 close 就丢弃数据源：后台任务停止，空闲连接被关闭
#[tokio::test]
async fn test_datasource_drop_closes_idle_connections() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 2;
    {
        let ds = DruidDataSource::new(driver, c);
        ds.init().await.unwrap();
        assert_eq!(ds.idle_count(), 2);
    } // Drop
    wait_until("Drop 后空闲连接被关闭", || {
        closed.load(Ordering::SeqCst) == 2
    })
    .await;
}

/// initial_size 超过 max_active：init 直接拒绝，不再静默钳制/浪费物理连接
#[tokio::test]
async fn test_initial_size_over_max_active_rejected() {
    let mut c = cfg("mock://a");
    c.initial_size = 10;
    c.max_active = 2;
    let ds = DruidDataSource::new(MockDriver::new(), c);

    let err = ds.init().await.unwrap_err();
    assert!(
        matches!(err, DruidError::Config(_)),
        "应返回配置错误，实际: {err:?}"
    );
    assert_eq!(ds.idle_count(), 0);
    assert_eq!(ds.metrics().create_count(), 0, "非法配置下不得建连");

    // 未进入已初始化状态：改正配置后仍可 init
    let mut c2 = cfg("mock://b");
    c2.initial_size = 2;
    c2.max_active = 2;
    let ds2 = DruidDataSource::new(MockDriver::new(), c2);
    ds2.init().await.unwrap();
    assert_eq!(ds2.idle_count(), 2);
}

/// 安全配置的静默失效：`config.filters` 未接线，必须在 init 硬报错而不是只打日志
///
/// 旧行为：`filters = ["wall"]` 是空操作——用户以为防火墙生效了，`DROP TABLE` 照样到驱动。
#[tokio::test]
async fn test_config_filters_rejected_at_init() {
    let mut c = cfg("mock://a");
    c.filters = vec!["wall".into()];
    let ds = DruidDataSource::new(MockDriver::new(), c);

    let err = ds.init().await.unwrap_err();
    assert!(
        matches!(err, DruidError::Config(_)),
        "应返回配置错误，实际: {err:?}"
    );
    assert!(
        err.to_string().contains("with_filters"),
        "错误信息必须指出替代 API: {err}"
    );
    assert_eq!(ds.idle_count(), 0);
    assert_eq!(ds.metrics().create_count(), 0, "拒绝配置时不得建连");
    assert!(
        ds.get_connection().await.is_err(),
        "被拒的配置不得留下半初始化状态"
    );

    // 改用 with_filters 注入即可正常启用（同一份 config 带 filters 字段也能跑通）
    let mut c2 = cfg("mock://b");
    c2.filters.clear();
    let ds2 = DruidDataSource::with_filters(MockDriver::new(), c2, vec![]);
    ds2.init().await.unwrap();
    assert_eq!(ds2.filter_chain().len(), 0);
}

/// 其余非法配置同样在 init 被 validate() 拦下
#[tokio::test]
async fn test_invalid_configs_rejected_at_init() {
    for (mut c, want) in [
        (cfg("mock://a"), "max_active is 0"),
        (cfg("mock://b"), "min_idle > max_active"),
        (cfg("mock://c"), "connect_timeout_secs is 0"),
    ] {
        match want {
            "max_active is 0" => c.max_active = 0,
            "min_idle > max_active" => {
                c.max_active = 2;
                c.min_idle = 3;
            }
            _ => c.connect_timeout_secs = 0,
        }
        let ds = DruidDataSource::new(MockDriver::new(), c);
        let err = ds.init().await.unwrap_err();
        assert!(matches!(err, DruidError::Config(_)), "{want}: {err:?}");
        assert!(err.to_string().contains(want), "错误信息应说明原因: {err}");
    }
}

/// validation_query_timeout_secs 生效：校验挂起时按超时失败
#[tokio::test]
async fn test_validation_query_timeout_effective() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_secs(5);
    let mut c = cfg("mock://a");
    c.test_on_borrow = true;
    c.validation_query_timeout_secs = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let start = Instant::now();
    let err = ds.get_connection().await.err().unwrap();
    let elapsed = start.elapsed();
    assert!(err.to_string().contains("timeout"), "实际: {err}");
    assert!(elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(3));
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0);
}

#[tokio::test]
async fn test_pscache_exposed_with_config() {
    let mut c = cfg("mock://a");
    c.pool_prepared_statements = true;
    c.max_pool_prepared_statement_per_connection_size = 2;
    let ds = DruidDataSource::new(MockDriver::new(), c);
    let mut cache = ds.pscache().lock().unwrap();
    assert!(!cache.get("SELECT 1"));
    cache.put("SELECT 1");
    assert!(cache.get("SELECT 1"));
}
