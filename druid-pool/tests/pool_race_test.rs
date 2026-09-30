//! 取消安全 / 关闭竞态 / 并发守恒 / Filter 接入 测试
//!
//! 这些路径正是串行 happy path 测试绕开的高风险区：
//! - 连接在途（connect/validate）被 timeout 取消
//! - 异步归还与 close() 交错
//! - 并发借还后的连接守恒
//! - Filter 注入与 statement 钩子生效

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{cfg, wait_until, MockDriver};
use druid_core::DruidError;
use druid_filter::{Filter, FilterContext};
use druid_pool::DruidDataSource;

/// 记录 Filter 钩子事件的共享日志
#[derive(Default)]
struct HookLog {
    events: Mutex<Vec<String>>,
    stmt_ids: Mutex<Vec<u64>>,
    conn_ids: Mutex<Vec<u64>>,
}

impl HookLog {
    fn push(&self, s: impl Into<String>) {
        self.events.lock().unwrap().push(s.into());
    }
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    fn has(&self, needle: &str) -> bool {
        self.events().iter().any(|e| e == needle)
    }
}

/// 记录 statement / connection 钩子顺序的 Filter（通过 Arc 在链外持有句柄）
struct HookFilter(Arc<HookLog>);

impl Filter for HookFilter {
    fn name(&self) -> &'static str {
        "hook"
    }

    fn connection_created(&self, ctx: &FilterContext) {
        self.0
            .conn_ids
            .lock()
            .unwrap()
            .push(ctx.connection_id.unwrap());
        self.0.push("conn_created");
    }
    fn connection_borrow_before(&self, _ctx: &FilterContext) {
        self.0.push("borrow_before");
    }
    fn connection_borrowed(&self, _ctx: &FilterContext, _wait_ms: u64) {
        self.0.push("borrowed");
    }
    fn connection_return_before(&self, _ctx: &FilterContext) {
        self.0.push("return_before");
    }
    fn connection_returned(&self, _ctx: &FilterContext) {
        self.0.push("returned");
    }
    fn connection_closed(&self, _ctx: &FilterContext) {
        self.0.push("conn_closed");
    }
    fn statement_created(&self, ctx: &FilterContext) {
        self.0
            .stmt_ids
            .lock()
            .unwrap()
            .push(ctx.statement_id.unwrap());
        self.0.push("stmt_created");
    }
    fn statement_execute_before(&self, ctx: &FilterContext) -> Result<(), DruidError> {
        self.0
            .push(format!("before:{}", ctx.sql.as_deref().unwrap_or("")));
        Ok(())
    }
    fn statement_execute_after(&self, ctx: &FilterContext, _ms: u64, rows: u64) {
        self.0
            .push(format!("after:{}:{rows}", ctx.sql.as_deref().unwrap_or("")));
    }
    fn statement_error(&self, _ctx: &FilterContext, error: &DruidError) {
        self.0.push(format!("stmt_error:{error}"));
    }
    fn statement_closed(&self, _ctx: &FilterContext) {
        self.0.push("stmt_closed");
    }
    fn resultset_open(&self, _ctx: &FilterContext) {
        self.0.push("rs_open");
    }
    fn resultset_closed(&self, _ctx: &FilterContext, rows_read: u64) {
        self.0.push(format!("rs_closed:{rows_read}"));
    }
}

/// 模拟 SQL 防火墙：拦截一切语句
struct DenyFilter;

impl Filter for DenyFilter {
    fn name(&self) -> &'static str {
        "deny"
    }
    fn statement_execute_before(&self, _ctx: &FilterContext) -> Result<(), DruidError> {
        Err(DruidError::Wall("blocked by wall".into()))
    }
}

fn hooked_ds(
    driver: MockDriver,
    config: druid_core::DruidConfig,
    log: &Arc<HookLog>,
) -> DruidDataSource<MockDriver> {
    DruidDataSource::with_filters(
        driver,
        config,
        vec![Box::new(HookFilter(log.clone())) as Box<dyn Filter>],
    )
}

// ── Filter 接入 ──

#[tokio::test]
async fn test_filters_injected_and_hooks_fire_in_order() {
    let log = Arc::new(HookLog::default());
    let ds = hooked_ds(MockDriver::new(), cfg("mock://a"), &log);
    assert_eq!(ds.filter_chain().len(), 1, "Filter 必须在构造期接入数据源");
    assert_eq!(ds.filter_chain().filter_names(), vec!["hook"]);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let conn_id = g.connection_id();
    assert_eq!(g.execute("INSERT INTO t VALUES (1)").await.unwrap(), 1);
    assert_eq!(g.execute("SELECT 2").await.unwrap(), 1);
    drop(g);

    assert_eq!(
        log.events(),
        vec![
            "conn_created",
            "borrow_before",
            "borrowed",
            "stmt_created",
            "before:INSERT INTO t VALUES (1)",
            "after:INSERT INTO t VALUES (1):1",
            "stmt_closed",
            "stmt_created",
            "before:SELECT 2",
            "after:SELECT 2:1",
            "stmt_closed",
            "return_before",
            "returned",
        ]
    );
    // 链外句柄看到的是同一个实例
    assert_eq!(log.conn_ids.lock().unwrap().as_slice(), &[conn_id]);
    // statement id 全局唯一
    let ids = log.stmt_ids.lock().unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn test_wall_filter_blocks_execution_before_driver() {
    let log = Arc::new(HookLog::default());
    let driver = MockDriver::new();
    let exec_calls = driver.exec_calls.clone();
    let ds = DruidDataSource::with_filters(
        driver,
        cfg("mock://a"),
        vec![
            Box::new(HookFilter(log.clone())) as Box<dyn Filter>,
            Box::new(DenyFilter),
        ],
    );
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let err = g.execute("DROP TABLE users").await.unwrap_err();
    assert!(matches!(err, DruidError::Wall(_)), "实际: {err:?}");
    assert_eq!(
        exec_calls.load(Ordering::SeqCst),
        0,
        "被拦截的 SQL 不得到达驱动"
    );

    let events = log.events();
    assert!(events.contains(&"before:DROP TABLE users".to_string()));
    assert!(events.contains(&"stmt_error:wall error: blocked by wall".to_string()));
    assert!(events.contains(&"stmt_closed".to_string()));
    assert!(
        !events.iter().any(|e| e.starts_with("after:")),
        "被拦截的语句不应触发 execute_after"
    );
    drop(g);
}

#[tokio::test]
async fn test_statement_error_hook_on_driver_failure() {
    let log = Arc::new(HookLog::default());
    let driver = MockDriver::new();
    let fail = driver.fail_execute.clone();
    let ds = hooked_ds(driver, cfg("mock://a"), &log);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    fail.store(true, Ordering::SeqCst);
    let err = g.execute("UPDATE t SET x = 1").await.unwrap_err();
    assert!(matches!(err, DruidError::Database(_)));

    let events = log.events();
    assert!(events.contains(&"stmt_error:database error: execute failed".to_string()));
    assert!(events.contains(&"stmt_closed".to_string()));
    assert!(!events.iter().any(|e| e.starts_with("after:")));

    // 恢复后同一连接可继续使用，且错误不会残留
    fail.store(false, Ordering::SeqCst);
    assert!(g.execute("UPDATE t SET x = 1").await.is_ok());
    assert!(log.has("after:UPDATE t SET x = 1:1"));
    drop(g);
}

#[tokio::test]
async fn test_query_triggers_resultset_hooks() {
    let log = Arc::new(HookLog::default());
    let ds = hooked_ds(MockDriver::new(), cfg("mock://a"), &log);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let rows = g.query("SELECT * FROM t").await.unwrap();
    assert_eq!(rows.len(), 1);

    let events = log.events();
    let rs_open = events.iter().position(|e| e == "rs_open").unwrap();
    let rs_closed = events.iter().position(|e| e == "rs_closed:1").unwrap();
    let after = events
        .iter()
        .position(|e| e == "after:SELECT * FROM t:1")
        .unwrap();
    assert!(rs_open < rs_closed && rs_closed < after);
    assert!(log.has("stmt_closed"));
}

// ── 取消安全 ──

/// 借用校验挂起时被 timeout 取消：计数归位，在途连接被物理关闭（不泄漏、不回池）
#[tokio::test]
async fn test_cancel_during_validate_closes_connection() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.test_on_borrow = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let result = tokio::time::timeout(Duration::from_millis(50), ds.get_connection()).await;
    assert!(result.is_err(), "get_connection 应被取消");

    // 计数立刻归位，不虚高固化
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0);
    assert_eq!(ds.metrics().create_count(), 1, "物理连接已创建");
    assert_eq!(ds.idle_count(), 0, "被取消的连接不得回池");

    wait_until("被取消的连接最终关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(ds.metrics().destroy_count(), 1);

    // 池仍可正常使用
    let g = ds.get_connection().await.unwrap();
    assert_eq!(g.execute("SELECT 1").await.unwrap(), 1);
}

/// connect 挂起时被取消：无物理连接产生，计数归位
#[tokio::test]
async fn test_cancel_during_connect_restores_counts() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let ds = DruidDataSource::new(driver, cfg("mock://a"));
    ds.init().await.unwrap();

    let result = tokio::time::timeout(Duration::from_millis(50), ds.get_connection()).await;
    assert!(result.is_err());
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0);
    assert_eq!(ds.metrics().create_count(), 0);
    assert_eq!(closed.load(Ordering::SeqCst), 0);
    assert_eq!(ds.idle_count(), 0);
}

/// connect 失败：错误返回且计数完全恢复
#[tokio::test]
async fn test_connect_failure_restores_counts() {
    let driver = MockDriver::new();
    let fail = driver.connect_fail.clone();
    fail.store(true, Ordering::SeqCst);
    let ds = DruidDataSource::new(driver, cfg("mock://a"));
    ds.init().await.unwrap();

    let err = ds.get_connection().await.err().unwrap();
    assert!(err.to_string().contains("connect refused"));
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0);
    assert_eq!(ds.metrics().create_count(), 0);

    // 恢复后仍可正常借用（许可未被吞掉）
    fail.store(false, Ordering::SeqCst);
    assert!(ds.get_connection().await.is_ok());
}

// ── 关闭竞态 ──

/// test_on_return 的异步归还任务与 close() 交错：连接不得进入已关闭池
#[tokio::test]
async fn test_close_during_async_return_does_not_leak() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    drop(g); // 校验+归还被 spawn 到后台任务
    ds.close().await.unwrap(); // 立刻关闭

    wait_until("归还任务结束", || ds.metrics().destroy_count() == 1).await;
    assert_eq!(ds.idle_count(), 0, "已关闭的池不得再接收连接");
    assert_eq!(ds.metrics().create_count(), 1);
    assert_eq!(closed.load(Ordering::SeqCst), 1, "恰好关闭一次");
}

/// 等待并发许可期间数据源被关闭：唤醒后不得再创建/交付连接
#[tokio::test]
async fn test_close_while_waiting_for_permit_returns_error() {
    let mut c = cfg("mock://a");
    c.max_active = 1;
    let ds = Arc::new(DruidDataSource::new(MockDriver::new(), c));
    ds.init().await.unwrap();

    let g1 = ds.get_connection().await.unwrap();
    let ds2 = ds.clone();
    let waiter = tokio::spawn(async move { ds2.get_connection().await });
    tokio::time::sleep(Duration::from_millis(50)).await; // 等待者已在信号量上排队
    assert_eq!(ds.metrics().waiting(), 1);

    ds.close().await.unwrap();
    drop(g1); // 释放许可，唤醒等待者

    let result = waiter.await.unwrap();
    assert!(result.is_err(), "已关闭的数据源不得再交付连接");
    assert_eq!(
        ds.metrics().create_count(),
        1,
        "不得为已关闭的数据源新建连接"
    );
    assert_eq!(ds.metrics().waiting(), 0);
}

/// 归还校验期间并发许可不得提前释放：max_active=1 时第二次借用必须等连接真正归位
#[tokio::test]
async fn test_permit_held_until_connection_returned() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(150); // 归还校验耗时
    let mut c = cfg("mock://a");
    c.max_active = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let conn_id = g.connection_id();
    drop(g); // 归还校验在后台进行

    let start = std::time::Instant::now();
    let g2 = ds.get_connection().await.unwrap(); // 必须等 permit 释放
    assert!(
        start.elapsed() >= Duration::from_millis(100),
        "并发许可被提前释放（max_active 闸门失效）"
    );
    assert_eq!(
        ds.metrics().create_count(),
        1,
        "不得出现超出 max_active 的物理连接"
    );
    assert_eq!(g2.connection_id(), conn_id, "应复用同一条物理连接");
}

// ── 并发守恒 ──

/// 并发借还压力下的守恒不变量：create - destroy == idle + active
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_stress_conservation() {
    let mut c = cfg("mock://a");
    c.max_active = 4;
    c.initial_size = 2;
    let ds = Arc::new(DruidDataSource::new(MockDriver::new(), c));
    ds.init().await.unwrap();

    let mut tasks = Vec::new();
    for i in 0..8 {
        let ds = ds.clone();
        tasks.push(tokio::spawn(async move {
            for j in 0..10 {
                let guard = ds.get_connection().await.unwrap();
                let sql = format!("SELECT {i}-{j}");
                assert_eq!(guard.execute(&sql).await.unwrap(), 1);
                drop(guard);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    let created = ds.metrics().create_count();
    let destroyed = ds.metrics().destroy_count();
    assert!(created >= 2);
    assert_eq!(destroyed, 0, "未开启驱逐/校验时不应销毁连接");
    assert_eq!(
        created - destroyed,
        (ds.idle_count() + ds.active_count()) as u64,
        "连接守恒不变量被破坏"
    );
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.metrics().waiting(), 0, "成功交付后 waiting 必须归零");
    assert!(ds.idle_count() <= ds.max_active());
    ds.close().await.unwrap();
}
