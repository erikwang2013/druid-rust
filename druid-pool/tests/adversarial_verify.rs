//! 对抗验证：取消安全 / 关闭竞态 / 计数守恒 / Filter 接入的攻击面
//!
//! 目的不是复述修复方已有的用例，而是尽力证伪。相比 `pool_race_test.rs`，
//! 本文件补的是修复方未覆盖的交错点：
//!   1. 取消点穷举 —— permit 排队、复用连接在 borrow-validate 中、select! 落选分支、嵌套 timeout
//!   2. close() 与 connect 在途 / guard 归还 / init 回填 / semaphore 排队者的交错
//!   3. 同一物理连接被并发借出两次（conn_id 断言）+ 高并发守恒
//!   4. Filter 链端到端 —— 逃生通道 `guard.connection()` 必须绕过 filter；取消时 statement 生命周期
//!   5. 反例检查 —— with_filters(vec![]) == new、test_on_borrow=false、max_active=1
//!
//! 第一轮探测到的缺陷（close×connect 交错交付、归还校验窗口破坏守恒）已修复，
//! 相应用例已翻转为对期望行为的断言，绿即代表修复未被回退。
//!
//! 第二轮改攻修复方新增的并发不变量：is_closed() 交付前复查、归还校验窗口内
//! 连接保持 active、close() 关闭 semaphore、StmtScope 兜底闭合、config.filters
//! 硬报错、pool_size() 单锁快照。
//!
//! 第三轮攻出的两个缺陷（query 双闭合、KeepAlive 校验与业务借用共用同一连接）
//! 已修复，留证用例在「0.」节转正为正常断言：query 三条路径各闭合一次；
//! KeepAlive 校验走与借用相同的状态语义（取许可 → 计入 active → 摘出 idle）。
//!
//! 最终轮（针对 KeepAlive 状态语义重写）：「7.」节 —— 摘出窗口守恒与不可借用、
//! close()/Drop abort 的 Validating RAII 结算（恰好一次）、try_acquire 跳过分支、
//! 与归还校验窗口叠加、min_idle/驱逐交互、独立 30 次连跑 flaky 检查。

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::{cfg, wait_until, MockDriver};
use druid_core::DruidError;
use druid_filter::{Filter, FilterContext};
use druid_pool::driver::{Connection, Driver};
use druid_pool::DruidDataSource;

// ── 断言与工具 ──

/// 守恒不变量：create - destroy == idle + active，且 waiting 归零
///
/// ⚠️ 采样纪律（上一轮踩过的坑）：锁保护的两个分量必须**单次加锁**取样 ——
/// 分别调用 `idle_count()` / `active_count()` 会读两个瞬间，并发归还时把
/// 「total(idle)+total(active)」读成池内总数超上界的假象。这里统一用 `pool_size()`。
/// 另外 create/destroy 是原子计数、与锁快照之间没有全局一致性快照，
/// 因此本断言**只在静止点调用**（无在途操作）才是可靠的。
fn assert_conserved<D: Driver>(ds: &DruidDataSource<D>, ctx: &str) {
    let m = ds.metrics();
    let (created, destroyed) = (m.create_count(), m.destroy_count());
    let total = ds.pool_size() as u64; // 单锁快照：active + idle
    assert_eq!(
        created - destroyed,
        total,
        "{ctx}: 守恒不变量被破坏（create={created} destroy={destroyed} pool_size={total} \
         idle={} active={}）",
        ds.idle_count(),
        ds.active_count()
    );
    assert_eq!(m.waiting(), 0, "{ctx}: waiting 未归零");
}

/// 统计某条 SQL 的 closed 事件次数（statement 生命周期闭合次数）
fn count_closed(log: &EventLog, sql: &str) -> usize {
    let needle = format!("closed:{sql}");
    log.events().iter().filter(|e| **e == needle).count()
}

/// 钩子顺序断言：`first` 必须早于 `then`（事件序号比较）
fn assert_order(ev: &[String], first: &str, then: &str) {
    let i = ev
        .iter()
        .position(|e| e == first)
        .unwrap_or_else(|| panic!("缺少事件 {first}: {ev:?}"));
    let j = ev
        .iter()
        .position(|e| e == then)
        .unwrap_or_else(|| panic!("缺少事件 {then}: {ev:?}"));
    assert!(i < j, "{first} 应早于 {then}: {ev:?}");
}

/// 借用，且把「permit 泄漏导致永久阻塞」变成明确失败
async fn borrow_within<D: Driver>(
    ds: &DruidDataSource<D>,
    ms: u64,
    ctx: &str,
) -> druid_pool::PoolGuard<D> {
    tokio::time::timeout(Duration::from_millis(ms), ds.get_connection())
        .await
        .unwrap_or_else(|_| panic!("{ctx}: 借用被永久阻塞（permit 疑似泄漏）"))
        .unwrap()
}

#[derive(Default)]
struct EventLog(Mutex<Vec<String>>);
impl EventLog {
    fn push(&self, s: impl Into<String>) {
        self.0.lock().unwrap().push(s.into());
    }
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn has(&self, needle: &str) -> bool {
        self.events().iter().any(|e| e == needle)
    }
}

/// 记录 statement / connection 钩子的 Filter
struct LogFilter(Arc<EventLog>);
impl Filter for LogFilter {
    fn name(&self) -> &'static str {
        "log"
    }
    fn connection_created(&self, _c: &FilterContext) {
        self.0.push("conn_created");
    }
    fn connection_closed(&self, _c: &FilterContext) {
        self.0.push("conn_closed");
    }
    fn connection_borrow_before(&self, _c: &FilterContext) {
        self.0.push("borrow_before");
    }
    fn statement_created(&self, ctx: &FilterContext) {
        self.0
            .push(format!("created:{}", ctx.sql.as_deref().unwrap_or("")));
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
    fn statement_error(&self, ctx: &FilterContext, e: &DruidError) {
        self.0.push(format!("err:{e}"));
        let _ = ctx;
    }
    fn statement_closed(&self, ctx: &FilterContext) {
        self.0
            .push(format!("closed:{}", ctx.sql.as_deref().unwrap_or("")));
    }
    fn resultset_open(&self, ctx: &FilterContext) {
        self.0
            .push(format!("rs_open:{}", ctx.sql.as_deref().unwrap_or("")));
    }
}

/// 模拟 SQL 防火墙：拦截 DDL（其余放行，便于同一条链上做对照）
struct DenyFilter;
impl Filter for DenyFilter {
    fn name(&self) -> &'static str {
        "deny"
    }
    fn statement_execute_before(&self, ctx: &FilterContext) -> Result<(), DruidError> {
        match ctx.sql.as_deref() {
            Some(sql) if sql.contains("DROP") => Err(DruidError::Wall("blocked by wall".into())),
            _ => Ok(()),
        }
    }
}

// ══════════════ 0. 历史留证缺陷（均已修复，转正为正常断言） ══════════════

/// 缺陷 ③（已修复）：`query()` 曾因 `StmtScope` 与函数末尾残留的显式调用叠加，
/// 每条语句触发**两次** `statement_closed`（`execute()` 一次），成功与拦截路径重复、
/// 取消路径只一次 —— `statement_created` : `statement_closed` 的 1:1 配对承诺被打破，
/// 按配对计数的 Filter（统计时长、审计、资源表）会随 query 数量漂移。
///
/// 修法：删掉残留的显式调用，`StmtScope` 成为**唯一**闭合路径。本用例守住三条路径
/// 各恰好闭合一次 + 全量配对守恒；取消路径见 `statement_lifecycle_balanced_on_cancel_query`。
#[tokio::test]
async fn defect_03_query_fires_statement_closed_twice() {
    let log = Arc::new(EventLog::default());
    let ds = DruidDataSource::with_filters(
        MockDriver::new(),
        cfg("mock://a"),
        vec![
            Box::new(LogFilter(log.clone())) as Box<dyn Filter>,
            Box::new(DenyFilter), // 提供拦截路径，覆盖出错分支
        ],
    );
    ds.init().await.unwrap();
    let g = ds.get_connection().await.unwrap();

    // execute 成功路径恰好闭合一次
    assert_eq!(g.execute("SELECT e").await.unwrap(), 1);
    assert_eq!(
        count_closed(&log, "SELECT e"),
        1,
        "execute 成功路径应闭合一次: {:?}",
        log.events()
    );
    // execute 被拦截路径也恰好闭合一次
    assert!(g.execute("DROP TABLE e").await.is_err());
    assert_eq!(
        count_closed(&log, "DROP TABLE e"),
        1,
        "execute 拦截路径应闭合一次: {:?}",
        log.events()
    );

    // query 成功路径恰好闭合一次（曾为 2）
    assert_eq!(g.query("SELECT q").await.unwrap().len(), 1);
    assert_eq!(
        count_closed(&log, "SELECT q"),
        1,
        "query 成功路径应闭合一次: {:?}",
        log.events()
    );
    // query 被拦截路径恰好闭合一次（曾为 2）
    assert!(g.query("DROP TABLE q").await.is_err());
    assert_eq!(
        count_closed(&log, "DROP TABLE q"),
        1,
        "query 拦截路径应闭合一次: {:?}",
        log.events()
    );

    // 配对守恒：4 条语句 = 4 次 created / 4 次 closed
    let ev = log.events();
    let created = ev.iter().filter(|e| e.starts_with("created:")).count();
    let closed = ev.iter().filter(|e| e.starts_with("closed:")).count();
    assert_eq!(
        (created, closed),
        (4, 4),
        "created/closed 必须 1:1 配对: {ev:?}"
    );
}

/// 缺陷 ④（已修复）：KeepAlive 的校验曾可与业务借用**并发使用同一条物理连接**
///
/// 旧实现把 idle 快照成 `Arc<D::Connection>` 后在锁外校验，校验在途期间那条连接
/// **仍留在 idle**，借用方可以正常摘走它并执行语句 —— keepalive 的 ping 与业务 SQL
/// 落在同一个 socket 上（真实 MySQL 客户端协议下同一连接不允许两个请求并发）。
///
/// 修法：与借用路径走**同一套状态语义** —— 先取并发许可，再在同一把锁内
/// 「计入 active + 摘出 idle」，校验结束后归还或销毁。因此：
///   - 借用方拿不到被校验的连接（只能另建/等待许可），`overlaps` 永远为 0；
///   - `close()` 能看见它（计入 active + 持有一条许可）；
///   - max_active=1 时不会凭空多出第二条物理连接（见下方 pool_size 断言）。
///
/// 复现（validate=300ms，execute=100ms）：`cargo test -p druid-pool --test adversarial_verify defect_04`
#[tokio::test]
async fn defect_04_keepalive_validates_connection_in_use() {
    let driver = OverlapDriver::new(300, 100);
    let overlaps = driver.overlaps.clone();
    let validating = driver.validating.clone();
    let mut c = cfg("mock://a");
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 50;
    c.initial_size = 1;
    c.max_active = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    // init 回填 1 条；首轮 tick（50ms）可能已把它摘出校验，
    // 故断言「池内共 1 条物理连接」而不是「idle 恰为 1」（避免调度抖动假红）
    assert_eq!(ds.pool_size(), 1, "应有一条物理连接供 KeepAlive 校验");

    wait_until("KeepAlive 校验开始", || {
        validating.load(Ordering::SeqCst) > 0
    })
    .await;

    // 校验仍在途（300ms）：连接已被摘出 idle 且占用唯一许可，
    // 借用方要么等待许可、要么另建连接，绝不可能拿到被校验的那条
    let g = ds.get_connection().await.unwrap();
    assert_eq!(
        validating.load(Ordering::SeqCst),
        0,
        "拿到连接时 KeepAlive 校验必须已结束（校验中的连接不可借用）"
    );
    assert_eq!(
        ds.pool_size(),
        1,
        "max_active=1：校验期间不得凭空多出第二条物理连接"
    );
    assert_eq!(g.execute("SELECT business").await.unwrap(), 1);
    assert_eq!(
        overlaps.load(Ordering::SeqCst),
        0,
        "同一物理连接不得在 KeepAlive 校验进行中被业务使用"
    );

    drop(g);
    ds.close().await.unwrap();
}

// ══════════════ 1. 取消安全穷举 ══════════════

/// 取消点 ①：在 semaphore 上排队等 permit 时被 abort
#[tokio::test]
async fn cancel_at_permit_wait_restores_counts() {
    let mut c = cfg("mock://a");
    c.max_active = 1;
    let ds = Arc::new(DruidDataSource::new(MockDriver::new(), c));
    ds.init().await.unwrap();

    let holder = ds.get_connection().await.unwrap();
    let ds2 = ds.clone();
    let waiter = tokio::spawn(async move { ds2.get_connection().await });
    wait_until("等待者已在 semaphore 排队", || {
        ds.metrics().waiting() == 1
    })
    .await;

    waiter.abort();
    let joined = waiter.await; // 任务被取消 → JoinError
    assert!(joined.is_err(), "abort 后任务应被取消");
    assert!(joined.err().unwrap().is_cancelled());

    // 取消发生在拿到 permit 之前：无连接产生，计数与 waiting 归位
    assert_eq!(ds.active_count(), 1, "只应剩持有者自己的 1 条");
    assert_eq!(
        ds.metrics().waiting(),
        0,
        "被取消的等待者必须归还 waiting 计数"
    );
    assert_eq!(ds.metrics().create_count(), 1);

    drop(holder);
    let g = borrow_within(&ds, 500, "取消点①").await;
    assert_eq!(ds.metrics().create_count(), 1, "应复用空闲连接，不得新建");
    drop(g);
    assert_conserved(&ds, "取消点①");
}

/// 取消点 ②：connect 在途时取消 —— 无物理连接产生，permit 立即释放
#[tokio::test]
async fn cancel_during_connect_releases_permit() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.max_active = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let r = tokio::time::timeout(Duration::from_millis(50), ds.get_connection()).await;
    assert!(r.is_err(), "get_connection 应被取消");
    assert_eq!(ds.metrics().create_count(), 0, "connect 被取消不得计数创建");
    assert_eq!(closed.load(Ordering::SeqCst), 0, "无物理连接被创建");
    assert_conserved(&ds, "取消点②");

    let g = borrow_within(&ds, 500, "取消点②").await;
    assert!(g.execute("SELECT 1").await.is_ok());
    drop(g);
}

/// 取消点 ③：复用「空闲池中已存在」的连接，在 borrow-validate 期间取消
///
/// 修复方只测了新建连接在 validate 中取消；这里攻的是复用路径：
/// 被摘出的连接若不归还且不关闭，就是一条永久泄漏的物理连接。
#[tokio::test]
async fn cancel_during_reused_validate_destroys_connection() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1; // 预置一条空闲连接（init 创建时不校验）
    c.test_on_borrow = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    assert_eq!(ds.idle_count(), 1);

    let r = tokio::time::timeout(Duration::from_millis(50), ds.get_connection()).await;
    assert!(r.is_err(), "validate 期间应被取消");

    // 被摘出的复用连接既不能留在 active，也不能回 idle
    assert_eq!(ds.active_count(), 0);
    assert_eq!(ds.idle_count(), 0, "被取消的复用连接不得回池");
    assert_eq!(ds.metrics().destroy_count(), 1);
    wait_until("被取消的复用连接被物理关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_conserved(&ds, "取消点③");
}

/// 取消点 ④：select! 的落选分支（仍在 connect 中）被 drop
#[tokio::test]
async fn select_losing_branch_dropped_safely() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(150);
    let ds = DruidDataSource::new(driver, cfg("mock://a"));
    ds.init().await.unwrap();

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(30)) => {}
        r = ds.get_connection() => panic!(
            "connect_latency=150ms，不应在 30ms 内返回: {:?}",
            r.map(|g| g.connection_id())
        ),
    }
    // 落选分支此刻已被 drop
    assert_conserved(&ds, "取消点④");
    let g = borrow_within(&ds, 500, "取消点④").await;
    drop(g);
}

/// 取消点 ⑤：嵌套取消 —— 外层 timeout 包住一个内层 timeout 的 get_connection
#[tokio::test]
async fn nested_timeout_cancel() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(400);
    let ds = DruidDataSource::new(driver, cfg("mock://a"));
    ds.init().await.unwrap();

    let outer = tokio::time::timeout(Duration::from_millis(60), async {
        // 内层超时更长：外层先触发，内层 timeout 与其持有的 get_connection 一起被 drop
        let _ = tokio::time::timeout(Duration::from_millis(400), ds.get_connection()).await;
    })
    .await;
    assert!(outer.is_err(), "外层超时应先触发");
    assert_conserved(&ds, "取消点⑤");
    let g = borrow_within(&ds, 1000, "取消点⑤").await;
    drop(g);
}

/// 计数漂移穷举：连续 30 次在不同 await 点取消，waiting/active 不得累积漂移
#[tokio::test]
async fn repeated_cancellation_does_not_drift_counters() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(30);
    driver.validate_latency = Duration::from_millis(30);
    let mut c = cfg("mock://a");
    c.max_active = 2;
    c.initial_size = 1;
    c.test_on_borrow = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    for i in 0..30u64 {
        // 轮换取消窗口：5ms → connect 在途；40ms → validate 在途
        let wait = if i % 5 < 2 { 5 } else { 40 };
        let _ = tokio::time::timeout(Duration::from_millis(wait), ds.get_connection()).await;
        assert_eq!(ds.metrics().waiting(), 0, "第 {i} 轮取消后 waiting 漂移");
        assert_eq!(ds.active_count(), 0, "第 {i} 轮取消后 active 漂移");
        assert_eq!(ds.idle_count(), 0, "第 {i} 轮取消后连接回池");
    }
    assert_conserved(&ds, "重复取消");
}

// ══════════════ 2. 关闭竞态 ══════════════

/// 数据源被 drop（而非 close）时仍有在途 guard：归还时必须物理关闭、不得回池、不得 panic
#[tokio::test]
async fn datasource_drop_with_active_guard_closes_connection() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let m = ds.metrics_arc(); // 数据源析构后仍可观测指标
    drop(ds);
    drop(g);
    wait_until("在途连接被物理关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        m.create_count() - m.destroy_count(),
        0,
        "数据源析构后不得残留连接"
    );
}

/// close() 与 connect 在途交错：close() 返回后不得再交付连接
///
/// 复现时序（connect_latency=200ms）：
///   t=0    借用方进入 get_connection：拿到 permit → is_closed()=false → 摘取空闲（无）→ connect().await
///   t=60   close() 返回：closed=true、idle 已排空
///   t=200  connect 返回：复查 closed → 拒绝交付，刚建的连接由 Drop 销毁
#[tokio::test]
async fn close_during_connect_rejects_delivery() {
    let log = Arc::new(EventLog::default());
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let ds = Arc::new(DruidDataSource::with_filters(
        driver,
        cfg("mock://a"),
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    ));
    ds.init().await.unwrap();

    let ds2 = ds.clone();
    let borrower = tokio::spawn(async move { ds2.get_connection().await });
    tokio::time::sleep(Duration::from_millis(60)).await; // 借用方已在 connect 中
    ds.close().await.unwrap();
    assert_eq!(
        ds.metrics().create_count(),
        0,
        "close() 时刻尚不该有物理连接"
    );

    let r = borrower.await.unwrap();
    assert!(
        r.is_err(),
        "close() 返回后不得交付连接，实际: {:?}",
        r.map(|g| g.connection_id())
    );
    // 尽早退出：connect 返回时即发现池已关闭，不得再为注定销毁的连接走借出钩子
    assert!(
        !log.has("borrow_before"),
        "close() 之后不得触发 connection_borrow_before: {:?}",
        log.events()
    );

    // 为在途 connect 而建立的连接必须被销毁，且计数守恒（不泄漏、不残留）
    wait_until("close 后建立的连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    wait_until("物理关闭", || closed.load(Ordering::SeqCst) == 1).await;
    assert_eq!(ds.idle_count(), 0);
    assert_conserved(&ds, "close×connect 交错");
}

/// 同一窗口在 borrow-validate 上：check-then-await 之后必须再复查一次 closed
/// （窗口不局限于 connect，而是「is_closed() 检查之后还存在 await」这件事本身）
#[tokio::test]
async fn close_during_borrow_validate_rejects_delivery() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(200);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.test_on_borrow = true;
    let ds = Arc::new(DruidDataSource::new(driver, c));
    ds.init().await.unwrap();

    let ds2 = ds.clone();
    let borrower = tokio::spawn(async move { ds2.get_connection().await });
    tokio::time::sleep(Duration::from_millis(60)).await; // 借用方停在 borrow-validate 中
    ds.close().await.unwrap();

    let r = borrower.await.unwrap();
    assert!(
        r.is_err(),
        "close() 返回后不得交付连接，实际: {:?}",
        r.map(|g| g.connection_id())
    );
    // 被摘出的复用连接既不能交付，也不能回池，必须物理关闭
    wait_until("close 后交付的连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    wait_until("物理关闭", || closed.load(Ordering::SeqCst) == 1).await;
    assert_eq!(ds.idle_count(), 0);
    assert_conserved(&ds, "close×borrow-validate 交错");
}

/// close() 与 guard Drop 并发：test_on_return 开/关两条归还路径各跑一遍
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_races_guard_drop() {
    for test_on_return in [false, true] {
        let driver = MockDriver::new();
        let closed = driver.closed.clone();
        let mut c = cfg("mock://a");
        c.initial_size = 2;
        c.max_active = 2;
        c.test_on_return = test_on_return;
        let ds = Arc::new(DruidDataSource::new(driver, c));
        ds.init().await.unwrap();

        let g1 = ds.get_connection().await.unwrap();
        let g2 = ds.get_connection().await.unwrap();

        let ds_close = ds.clone();
        let closer = tokio::spawn(async move { ds_close.close().await });
        drop(g1);
        drop(g2);
        closer.await.unwrap().unwrap();

        wait_until("全部连接被销毁", || {
            ds.metrics().destroy_count() == 2
        })
        .await;
        wait_until("全部连接物理关闭", || {
            closed.load(Ordering::SeqCst) == 2
        })
        .await;
        assert_eq!(
            ds.idle_count(),
            0,
            "已关闭的池不得残留空闲连接 (test_on_return={test_on_return})"
        );
        assert_conserved(&ds, &format!("close×drop(test_on_return={test_on_return})"));
    }
}

/// close() 与 init() 回填并发：任何交错下都要守恒，且不得有连接残留
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_races_init_refill() {
    let mut driver = MockDriver::new();
    driver.connect_latency = Duration::from_millis(30);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 4;
    c.max_active = 4;
    let ds = Arc::new(DruidDataSource::new(driver, c));

    let ds_init = ds.clone();
    let init = tokio::spawn(async move { ds_init.init().await });
    tokio::time::sleep(Duration::from_millis(45)).await; // init 正在回填途中
    ds.close().await.unwrap();
    init.await.unwrap().unwrap(); // close 后 init 应停止回填并正常返回

    wait_until("全部回填连接被关闭", || {
        closed.load(Ordering::SeqCst) == ds.metrics().create_count()
    })
    .await;
    assert_eq!(ds.idle_count(), 0, "已关闭的池不得残留空闲连接");
    assert_eq!(
        ds.metrics().destroy_count(),
        ds.metrics().create_count(),
        "回填连接必须全部销毁"
    );
    assert_conserved(&ds, "close×init");
}

/// close() 必须唤醒挂在 semaphore 上的等待者（Druid Java 的 notifyEmptyWaiters）
///
/// 默认 max_wait_ms=0（无限等待）时，排队者只能靠别人归还 permit 才醒；
/// 若持有者被泄漏或长期占用，close() 不唤醒就是永久挂起。
#[tokio::test]
async fn close_wakes_queued_waiter() {
    let mut c = cfg("mock://a");
    c.max_active = 1; // max_wait_ms 默认 0 = 无限等待
    let ds = Arc::new(DruidDataSource::new(MockDriver::new(), c));
    ds.init().await.unwrap();

    let holder = ds.get_connection().await.unwrap(); // 故意长期占用唯一 permit
    let ds2 = ds.clone();
    let waiter = tokio::spawn(async move { ds2.get_connection().await });
    wait_until("等待者已排队", || ds.metrics().waiting() == 1).await;

    ds.close().await.unwrap();
    // 持有者不归还 permit，close() 也必须让等待者落地
    let result = tokio::time::timeout(Duration::from_millis(300), waiter)
        .await
        .expect("close() 未唤醒排队者（等待者仍挂在 semaphore 上）")
        .unwrap();
    let e = result.err().unwrap();
    assert!(
        matches!(e, DruidError::Pool(_)),
        "已关闭的数据源不得交付连接: {e:?}"
    );
    // 观察项（非缺陷）：唤醒后由 acquire_permit 的 map_err 产生 "semaphore closed"，
    // 与顶层 is_closed() 检查的 "datasource is closed" 文案不一致；类型与语义一致。
    assert!(
        e.to_string().contains("semaphore closed")
            || e.to_string().contains("datasource is closed"),
        "意外错误文案: {e}"
    );
    wait_until("等待者归还计数", || ds.metrics().waiting() == 0).await;
    assert_eq!(
        ds.metrics().create_count(),
        1,
        "不得为已关闭的数据源新建连接"
    );

    drop(holder);
    wait_until("持有者归还的连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    assert_conserved(&ds, "close 唤醒排队者");
}

/// ⚠️ 已知残余风险（固化当前行为）：test_on_return 的归还校验没有超时兜底
///
/// validation_query_timeout_secs 默认 0 = 无超时，校验任务挂起（网络黑洞）时：
///   - 唯一 permit 被后台任务吞掉，后续借用全部超时（max_active 容量永久缺失）
///   - 连接不会入池（defect_02 修复后它挂在 active 上，close() 看得见、回收不到；
///     校验一旦返回就会被销毁，不会静默留在池里）
#[tokio::test]
async fn hung_return_validation_swallows_permit() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_secs(30); // 模拟网络黑洞
    let mut c = cfg("mock://a");
    c.max_active = 1;
    c.test_on_return = true;
    c.validation_query_timeout_secs = 0; // 默认值：无超时
    c.max_wait_ms = 300; // 便于观察而不是真的挂死
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    drop(g); // 归还校验进入 30s 挂起
    tokio::time::sleep(Duration::from_millis(50)).await;

    let err = ds.get_connection().await.err().unwrap();
    assert!(
        err.to_string().contains("connection wait timeout"),
        "实际: {err}"
    );
    assert_eq!(
        ds.active_count(),
        1,
        "校验在途的连接仍计入 active（defect_02 修复后守恒）"
    );
    assert_eq!(ds.idle_count(), 0, "连接未回到 idle（在途）");
    assert_conserved(&ds, "归还校验挂起时");
    ds.close().await.unwrap();
    assert_eq!(ds.metrics().destroy_count(), 0, "close() 回收不到在途连接");
    // 该连接不会入池：校验返回时池已 closed，走销毁路径；permit 也随之释放
}

/// 归还校验窗口内守恒不变量必须成立（defect_02 修复后）
///
/// guard Drop 把连接交给后台任务做归还校验；校验期间连接"在途"，
/// 修复后它仍计入 active（而不是从所有计数中消失），
/// 因此 `create - destroy == idle + active` 在窗口内也成立。
///
/// 复现：test_on_return=true，validate_latency=400ms；drop 后 80ms 采样。
#[tokio::test]
async fn return_validation_window_conserved() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(400); // 拉长窗口
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    drop(g); // 连接进入后台归还校验

    tokio::time::sleep(Duration::from_millis(80)).await; // 归还校验仍在途
    assert_eq!(ds.idle_count(), 0, "校验未完成，连接不应已在 idle");
    assert_eq!(ds.active_count(), 1, "在途连接必须可观测（计入 active）");
    assert_conserved(&ds, "归还校验窗口内");

    // 校验结束后连接回到 idle，指标依然守恒
    wait_until("归还校验收敛", || ds.idle_count() == 1).await;
    assert_eq!(ds.active_count(), 0);
    assert_conserved(&ds, "归还校验完成后");
}

// ══════════════ 3. 并发守恒 / 重复借出 ══════════════

/// 高并发压测：同一物理连接绝不并发借出两次（conn_id 断言）+ 守恒
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_no_double_lend_and_conserved() {
    let mut c = cfg("mock://a");
    c.max_active = 4;
    c.initial_size = 2;
    let ds = Arc::new(DruidDataSource::new(MockDriver::new(), c));
    ds.init().await.unwrap();

    let live: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let max_live = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for i in 0..8 {
        let (ds, live, max_live) = (ds.clone(), live.clone(), max_live.clone());
        tasks.push(tokio::spawn(async move {
            for j in 0..25 {
                let g = ds.get_connection().await.unwrap();
                let id = g.connection_id();
                {
                    let mut set = live.lock().unwrap();
                    assert!(set.insert(id), "同一物理连接被并发借出两次: conn_id={id}");
                    max_live.fetch_max(set.len() as u64, Ordering::SeqCst);
                }
                // 单次快照取样：分别读 active/idle 会跨越两次锁，把正常的并发归还
                // 读成「总数超过 max_active」的假象（实测 create 数并未突破上界）
                assert!(
                    ds.pool_size() <= ds.max_active(),
                    "池内物理连接数超过 max_active: pool_size={} active={} idle={} create={}",
                    ds.pool_size(),
                    ds.active_count(),
                    ds.idle_count(),
                    ds.metrics().create_count()
                );
                tokio::time::sleep(Duration::from_micros(200)).await; // 放大持有窗口
                assert_eq!(g.execute(&format!("SELECT {i}-{j}")).await.unwrap(), 1);
                live.lock().unwrap().remove(&id);
                drop(g);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    assert!(
        max_live.load(Ordering::SeqCst) <= 4,
        "并发持有数超过 max_active"
    );
    assert!(ds.metrics().create_count() >= 2);
    assert!(ds.idle_count() <= ds.max_active());
    assert_conserved(&ds, "并发压测");
    assert_eq!(ds.active_count(), 0);
    ds.close().await.unwrap();
}

/// test_on_return 异步归还路径下的压测：permit 顺序 + 物理连接数不超过 max_active
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_test_on_return_conserved() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(2); // 放大异步归还窗口
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.max_active = 3;
    c.initial_size = 1;
    c.test_on_return = true;
    let ds = Arc::new(DruidDataSource::new(driver, c));
    ds.init().await.unwrap();

    let mut tasks = Vec::new();
    for i in 0..6 {
        let ds = ds.clone();
        tasks.push(tokio::spawn(async move {
            for j in 0..15 {
                let g = ds.get_connection().await.unwrap();
                let _ = g.execute(&format!("SELECT {i}-{j}")).await.unwrap();
                drop(g);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // 归还校验是异步的：先等「无在途连接」收敛再验守恒。
    // 注意在途窗口本身即 defect_02 —— 此刻 create-destroy > idle+active，
    // 若连接被永久丢在窗口里，本 wait_until 会以超时报错而不是静默放过。
    // 采样同样走 pool_size() 单锁快照（不跨锁读两个瞬间）。
    wait_until("归还校验全部收敛（无在途连接）", || {
        ds.pool_size() as u64 == ds.metrics().create_count() - ds.metrics().destroy_count()
    })
    .await;
    wait_until("物理关闭收敛", || {
        closed.load(Ordering::SeqCst) == ds.metrics().destroy_count()
    })
    .await;
    let live_physical = ds.metrics().create_count() - closed.load(Ordering::SeqCst);
    assert!(
        live_physical <= 3,
        "物理连接上界被突破: 存活 {live_physical} > max_active=3"
    );
    assert_conserved(&ds, "test_on_return 压测");
    ds.close().await.unwrap();
}

/// 重复借还（max_active=1）：permit 若先于连接归位释放，就会多开物理连接
#[tokio::test]
async fn permit_not_released_before_connection_returned() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(5);
    let mut c = cfg("mock://a");
    c.max_active = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    for _ in 0..20 {
        let g = borrow_within(&ds, 2000, "permit 顺序").await;
        drop(g);
    }
    assert_eq!(
        ds.metrics().create_count(),
        1,
        "permit 提前释放导致物理连接超过 max_active"
    );
    wait_until("归还校验收敛", || ds.idle_count() == 1).await;
    assert_conserved(&ds, "permit 顺序");
}

// ══════════════ 4. Filter 链端到端 ══════════════

/// 逃生通道 `guard.connection()` 必须直达驱动、绕过全部 filter 钩子
#[tokio::test]
async fn escape_hatch_bypasses_filter_chain() {
    let log = Arc::new(EventLog::default());
    let driver = MockDriver::new();
    let exec_calls = driver.exec_calls.clone();
    let ds = DruidDataSource::with_filters(
        driver,
        cfg("mock://a"),
        vec![
            Box::new(LogFilter(log.clone())) as Box<dyn Filter>,
            Box::new(DenyFilter),
        ],
    );
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();

    // 受控通道：被防火墙拦截，且不得到达驱动
    assert!(matches!(
        g.execute("DROP TABLE t").await.unwrap_err(),
        DruidError::Wall(_)
    ));
    assert_eq!(
        exec_calls.load(Ordering::SeqCst),
        0,
        "被拦截的 SQL 不得到达驱动"
    );
    assert!(log.has("before:DROP TABLE t"));
    assert!(log.has("err:wall error: blocked by wall"));
    assert!(
        log.has("closed:DROP TABLE t"),
        "拦截路径同样要闭合 statement"
    );

    // 逃生通道：绕过 filter 直达驱动（文档承诺的差异必须真实存在）
    let before = log.events().len();
    assert_eq!(g.connection().execute("DROP TABLE t").await.unwrap(), 1);
    assert_eq!(exec_calls.load(Ordering::SeqCst), 1, "逃生通道必须直达驱动");
    assert_eq!(
        log.events().len(),
        before,
        "逃生通道不得触发任何 filter 钩子（含 statement/resultset）"
    );
    let _ = g.connection().query("SELECT 1").await.unwrap();
    assert_eq!(
        log.events().len(),
        before,
        "逃生通道 query 同样不得触发钩子"
    );

    // 受控 query 触发 resultset 钩子，作为对照
    assert_eq!(g.query("SELECT 1").await.unwrap().len(), 1);
    assert!(log.has("rs_open:SELECT 1"));
    drop(g);
}

/// execute 在途被取消时 statement 生命周期同样必须闭合
///
/// 顺序承诺是 created → before → after/error → closed；取消（超时/断连）时
/// 也必须触发 `statement_closed`，否则依赖 created/closed 配对的统计类 Filter 会漂移。
#[tokio::test]
async fn statement_lifecycle_balanced_on_cancel() {
    let log = Arc::new(EventLog::default());
    let ds = DruidDataSource::with_filters(
        SlowDriver::new(Duration::from_millis(200)),
        cfg("mock://a"),
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    );
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(50), g.execute("SELECT slow")).await;
    assert!(r.is_err(), "execute 应被取消");

    let ev = log.events();
    assert!(ev.iter().any(|e| e == "created:SELECT slow"));
    assert!(ev.iter().any(|e| e == "before:SELECT slow"));
    assert_eq!(
        ev.iter().filter(|e| e.starts_with("closed:")).count(),
        1,
        "取消时 statement 也必须闭合，且恰好一次（created/closed 配对不漂移）: {ev:?}"
    );
    // 连接本身没有被泄漏：guard 仍在，池状态不变
    assert_eq!(ds.active_count(), 1);
    drop(g);
    assert_conserved(&ds, "取消中途 execute");
}

/// query 在途被取消：statement 同样闭合，且不得伪造 resultset 钩子
#[tokio::test]
async fn statement_lifecycle_balanced_on_cancel_query() {
    let log = Arc::new(EventLog::default());
    let ds = DruidDataSource::with_filters(
        SlowDriver::new(Duration::from_millis(200)),
        cfg("mock://a"),
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    );
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let r = tokio::time::timeout(Duration::from_millis(50), g.query("SELECT slow")).await;
    assert!(r.is_err(), "query 应被取消");

    let ev = log.events();
    assert!(ev.iter().any(|e| e == "created:SELECT slow"));
    assert!(ev.iter().any(|e| e == "before:SELECT slow"));
    assert_eq!(
        ev.iter().filter(|e| e.starts_with("closed:")).count(),
        1,
        "取消时 query 的 statement 也必须恰好闭合一次: {ev:?}"
    );
    assert!(
        !ev.iter().any(|e| e.starts_with("rs_open:")),
        "查询未成功，不得触发 resultset_open: {ev:?}"
    );
    assert_eq!(ds.active_count(), 1);
    drop(g);
    assert_conserved(&ds, "取消中途 query");
}

// ══════════════ 5. 第二轮新增不变量攻击 ══════════════

/// `StmtScope` 兜底闭合：execute 各路径恰好闭合一次 + 钩子顺序 + 未 poll 的 future
///
/// 覆盖：成功 / 被拦截 / `mem::forget`（future 从未 poll，任何钩子都不该触发）。
/// 取消与 panic 展开见各自的用例。
#[tokio::test]
async fn stmt_scope_closes_exactly_once_and_in_order() {
    let log = Arc::new(EventLog::default());
    let ds = DruidDataSource::with_filters(
        MockDriver::new(),
        cfg("mock://a"),
        vec![
            Box::new(LogFilter(log.clone())) as Box<dyn Filter>,
            Box::new(DenyFilter),
        ],
    );
    ds.init().await.unwrap();
    let g = ds.get_connection().await.unwrap();

    // 成功路径：created → before → after → closed，且各一次
    assert_eq!(g.execute("SELECT ok").await.unwrap(), 1);
    let ev = log.events();
    assert_order(&ev, "created:SELECT ok", "before:SELECT ok");
    assert_order(&ev, "before:SELECT ok", "after:SELECT ok:1");
    assert_order(&ev, "after:SELECT ok:1", "closed:SELECT ok");
    assert_eq!(count_closed(&log, "SELECT ok"), 1, "{ev:?}");

    // 被拦截路径：created → before → err → closed
    assert!(g.execute("DROP TABLE t").await.is_err());
    let ev = log.events();
    assert_order(
        &ev,
        "before:DROP TABLE t",
        "err:wall error: blocked by wall",
    );
    assert_order(
        &ev,
        "err:wall error: blocked by wall",
        "closed:DROP TABLE t",
    );
    assert_eq!(count_closed(&log, "DROP TABLE t"), 1, "{ev:?}");

    // 未 poll 就被 forget 的 future：create 钩子都还没跑（async fn 体在首次 poll 才执行），
    // 因此不存在"未闭合的 statement"——forget 泄漏的是 future 本身，与生命周期无关
    let before_len = log.events().len();
    std::mem::forget(g.execute("FORGOTTEN"));
    assert_eq!(
        log.events().len(),
        before_len,
        "未 poll 的 future 不得触发任何钩子: {:?}",
        log.events()
    );

    // forget 之后 guard 仍可正常使用（future 只借用 &self，没有拿走任何状态）
    assert_eq!(g.execute("SELECT after").await.unwrap(), 1);
    assert_eq!(count_closed(&log, "SELECT after"), 1);

    // 配对：3 条真正执行的语句 → 3 created / 3 closed
    let ev = log.events();
    assert_eq!(
        (
            ev.iter().filter(|e| e.starts_with("created:")).count(),
            ev.iter().filter(|e| e.starts_with("closed:")).count()
        ),
        (3, 3),
        "created/closed 必须 1:1 配对: {ev:?}"
    );
    drop(g);
    assert_conserved(&ds, "StmtScope 兜底闭合");
}

/// 驱动 panic（unwind）时 statement 仍必须恰好闭合一次，且连接不泄漏
///
/// `StmtScope` 在 Drop 里闭合，正是为了这条路径：panic 展开会跑 Drop，
/// 而函数末尾的显式调用不会被执行。
#[tokio::test]
async fn stmt_closed_exactly_once_on_panic_unwind() {
    let log = Arc::new(EventLog::default());
    let ds = Arc::new(DruidDataSource::with_filters(
        PanicDriver,
        cfg("mock://a"),
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    ));
    ds.init().await.unwrap();
    let g = ds.get_connection().await.unwrap();

    let joined = tokio::spawn(async move { g.execute("BOOM").await }).await;
    assert!(joined.is_err(), "驱动 panic 应传播为 JoinError");
    assert!(joined.err().unwrap().is_panic(), "应为 panic 而非取消");

    let ev = log.events();
    assert!(ev.iter().any(|e| e == "created:BOOM"), "{ev:?}");
    assert_eq!(
        count_closed(&log, "BOOM"),
        1,
        "panic 展开也必须让 statement 恰好闭合一次: {ev:?}"
    );
    // guard 随 future 一起被 drop：连接归池，不随 panic 泄漏
    wait_until("panic 后连接归池", || ds.idle_count() == 1).await;
    assert_conserved(&ds, "panic 展开");
}

/// 归还校验窗口内：第二个借用者必须**等待**（而非报错/超时），且不得新建连接
///
/// 攻的是修复方的核心主张：窗口内连接保持 active 且 permit 到真正归还后才释放。
/// 若 permit 提前释放或连接被算作「不可用」，max_active=1 下第二个借用者会拿到
/// Err/超时；若 permit 释放早于连接归位，则会新建第二条物理连接。
#[tokio::test]
async fn return_window_second_borrower_waits_then_succeeds() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(150);
    let mut c = cfg("mock://a");
    c.max_active = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    let t0 = std::time::Instant::now();
    drop(g); // 进入归还校验窗口（约 150ms），permit 未释放

    let g2 = borrow_within(&ds, 2000, "归还窗口内第二个借用者").await;
    let waited = t0.elapsed();
    assert!(
        waited >= Duration::from_millis(100),
        "第二个借用者不应在归还校验完成前拿到连接（实际等待 {waited:?}）"
    );
    assert_eq!(
        ds.metrics().create_count(),
        1,
        "应复用同一条物理连接，不得因窗口计数错位而新建"
    );
    assert_eq!(
        ds.metrics().destroy_count(),
        0,
        "归还校验成功，不得销毁连接"
    );
    drop(g2);
    wait_until("归还校验收敛", || ds.idle_count() == 1).await;
    assert_conserved(&ds, "归还窗口内第二个借用者");
}

/// close() 落在归还校验窗口内：窗口结束时连接必须被**销毁**而不是入池
///
/// 窗口内连接既不在 idle（close() 排空不到它）也没交付给任何人，
/// 只可能由后台校验任务在结束时发现 `closed` 并物理关闭。
#[tokio::test]
async fn close_during_return_window_destroys_not_pools() {
    let mut driver = MockDriver::new();
    driver.validate_latency = Duration::from_millis(300);
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.test_on_return = true;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    drop(g); // 进入归还校验窗口
    tokio::time::sleep(Duration::from_millis(60)).await;

    // 窗口内：连接可观测（active），池守恒；close() 的 idle 排空碰不到它
    assert_eq!(ds.active_count(), 1, "窗口内连接必须仍可观测");
    assert_eq!(ds.idle_count(), 0);
    assert_conserved(&ds, "归还窗口内（close 前）");

    ds.close().await.unwrap();
    assert_eq!(
        ds.metrics().destroy_count(),
        0,
        "close() 排空的是 idle，窗口内连接由后台任务回收"
    );
    wait_until("窗口结束时连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    wait_until("物理关闭", || closed.load(Ordering::SeqCst) == 1).await;
    assert_eq!(ds.idle_count(), 0, "已关闭的池不得收回连接");
    assert_eq!(ds.active_count(), 0, "窗口结束后不得残留 active");
    assert_conserved(&ds, "close×归还窗口");
}

/// `semaphore.close()` 的副作用：已关闭池上不得挂起、重复 close 幂等、init 不得复活
#[tokio::test]
async fn close_semaphore_side_effects() {
    let mut c = cfg("mock://a");
    c.max_active = 2;
    c.initial_size = 1;
    let ds = DruidDataSource::new(MockDriver::new(), c);
    ds.init().await.unwrap();
    ds.close().await.unwrap();

    // 1) 关闭后新借用必须**立即**报错（semaphore 已关闭 ⇒ 不排队 ⇒ 不挂起）
    for i in 0..3 {
        let r = tokio::time::timeout(Duration::from_millis(300), ds.get_connection())
            .await
            .unwrap_or_else(|_| panic!("第 {i} 次：close() 后 get_connection 挂起"));
        let e = r.err().unwrap();
        assert!(matches!(e, DruidError::Pool(_)), "第 {i} 次错误类型: {e:?}");
    }
    // 2) semaphore.close() 与 DruidDataSource::close() 都可重复调用
    ds.close().await.unwrap();
    ds.close().await.unwrap();

    // 3) init() 不得让已关闭的数据源复活
    let e = ds.init().await.unwrap_err();
    assert!(
        matches!(e, DruidError::Pool(_)),
        "close 后 init 应报 Pool: {e:?}"
    );

    // 4) 之后仍不得交付连接
    let r = tokio::time::timeout(Duration::from_millis(300), ds.get_connection())
        .await
        .expect("不得挂起");
    assert!(r.is_err());
    assert_conserved(&ds, "close 后语义");
}

/// config.filters 非空必须在 init() **硬失败**，且不得置位 inited
///
/// 攻的是「校验顺序」：若 `inited.swap` 早于校验，配置错误的数据源会进入
/// 「已初始化」状态，get_connection 照常放行 —— 安全配置的静默失效换个马甲复现。
#[tokio::test]
async fn config_filters_hard_error_blocks_init() {
    // 1) filters 未接线：硬报错
    let mut c = cfg("mock://a");
    c.filters = vec!["wall".into()];
    let ds = DruidDataSource::new(MockDriver::new(), c);
    let e = ds.init().await.unwrap_err();
    assert!(
        matches!(e, DruidError::Config(_)),
        "应报 Config 错误: {e:?}"
    );
    let r = tokio::time::timeout(Duration::from_millis(300), ds.get_connection())
        .await
        .expect("不得挂起");
    let e = r.err().unwrap();
    assert!(
        e.to_string().contains("not initialized"),
        "init 失败后不得放行（inited 不得被置位），实际: {e}"
    );

    // 2) 参数校验失败（initial_size > max_active）同样不得置位 inited
    let mut c2 = cfg("mock://b");
    c2.initial_size = 2;
    c2.max_active = 1;
    let ds2 = DruidDataSource::new(MockDriver::new(), c2);
    assert!(matches!(
        ds2.init().await.unwrap_err(),
        DruidError::Config(_)
    ));
    assert!(
        ds2.get_connection().await.is_err(),
        "配置校验失败的数据源不得交付连接"
    );

    // 3) 对照：with_filters 注入（config.filters 仍为空）不得被这个硬报错误伤
    let ds3 = DruidDataSource::with_filters(
        MockDriver::new(),
        cfg("mock://c"),
        vec![Box::new(DenyFilter) as Box<dyn Filter>],
    );
    ds3.init().await.unwrap();
    assert_eq!(ds3.filter_chain().len(), 1);
    let g = ds3.get_connection().await.unwrap();
    assert!(
        g.execute("DROP TABLE t").await.is_err(),
        "注入的 Filter 必须生效"
    );
    drop(g);
    assert_conserved(&ds3, "with_filters 不受 config.filters 硬报错影响");
}

/// `pool_size()` 单锁快照与 active/idle 分量一致（只在静止点比较两个分量）
#[tokio::test]
async fn pool_size_single_snapshot_consistent() {
    let mut c = cfg("mock://a");
    c.max_active = 3;
    c.initial_size = 2;
    let ds = DruidDataSource::new(MockDriver::new(), c);
    ds.init().await.unwrap();
    assert_eq!(ds.pool_size(), 2, "静止态 pool_size 应等于 idle+active");

    let g1 = ds.get_connection().await.unwrap();
    let g2 = ds.get_connection().await.unwrap();
    assert_eq!(ds.pool_size(), 2);
    assert_eq!((ds.idle_count(), ds.active_count()), (0, 2));
    assert!(ds.pool_size() <= ds.max_active(), "不得超过 max_active");

    drop(g1);
    assert_eq!(ds.pool_size(), 2, "归还后物理连接总数不变");
    assert_eq!((ds.idle_count(), ds.active_count()), (1, 1));
    drop(g2);
    assert_eq!((ds.idle_count(), ds.active_count()), (2, 0));
    assert_eq!(ds.pool_size(), 2);
    assert_conserved(&ds, "pool_size 快照");
}

/// close() 与「正在交付中的借用者」并发：任何交错下都不得有连接被交付后失联
///
/// 最后一处 `is_closed()`（datasource.rs:345）到 `Ok(guard)` 之间没有 await，
/// 理论上仍存在极窄窗口（跨线程）。本用例反复在该窗口附近制造交错，
/// 验证即便命中「关闭后仍交付」，连接也只是被借用后销毁，不会从计数中消失。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_races_delivery_no_lost_connection() {
    let mut delivered_total = 0u64;
    for round in 0..20u64 {
        let mut driver = MockDriver::new();
        driver.connect_latency = Duration::from_millis(2);
        let closed = driver.closed.clone();
        let mut c = cfg("mock://a");
        c.initial_size = 2;
        c.max_active = 2;
        let ds = Arc::new(DruidDataSource::new(driver, c));
        ds.init().await.unwrap();

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let ds2 = ds.clone();
            tasks.push(tokio::spawn(async move { ds2.get_connection().await }));
        }
        tokio::time::sleep(Duration::from_millis(1)).await; // 借用者正处在 connect/交付之间
        ds.close().await.unwrap();

        for t in tasks {
            if let Ok(Ok(g)) = t.await {
                delivered_total += 1; // 关闭瞬间已交付的租约：允许存在，但必须被回收
                drop(g);
            }
        }

        wait_until("全部连接被销毁", || {
            ds.metrics().create_count() == ds.metrics().destroy_count()
        })
        .await;
        wait_until("物理关闭收敛", || {
            closed.load(Ordering::SeqCst) == ds.metrics().destroy_count()
        })
        .await;
        assert_eq!(ds.pool_size(), 0, "第 {round} 轮：关闭后不得残留连接");
        assert_conserved(&ds, &format!("close×交付竞态 第 {round} 轮"));
    }
    // 交付本来就不保证不发生（窗口存在），这里只保证「交付了也回收得掉」
    println!("close×交付竞态：20 轮中共交付 {delivered_total} 次租约，全部回收");
}

// ══════════════ 6. 反例检查（修复是否过度/失效） ══════════════

/// with_filters(vec![]) 必须与 new() 等价
#[tokio::test]
async fn with_filters_empty_vec_equivalent_to_new() {
    let ds = DruidDataSource::with_filters(MockDriver::new(), cfg("mock://a"), vec![]);
    assert!(ds.filter_chain().is_empty());
    assert_eq!(ds.filter_chain().len(), 0);
    ds.init().await.unwrap();

    let g = ds.get_connection().await.unwrap();
    assert_eq!(g.execute("SELECT 1").await.unwrap(), 1);
    assert_eq!(g.query("SELECT 1").await.unwrap().len(), 1);
    drop(g);
    assert_conserved(&ds, "空 filter 链");
}

/// max_lifetime 必须按 `created_at`（物理连接创建时刻）判定，而不是"最近使用时刻"
///
/// 反例形态：借还刷新 last_used_at 让热点连接永生（Java Druid 的经典 bug 形态）。
#[tokio::test]
async fn max_lifetime_uses_created_at_not_last_used() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 1;
    c.max_active = 1;
    c.max_lifetime_ms = 200;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    // 寿命内反复借还（每次都在刷新 last_used_at）
    for i in 0..2 {
        let g = borrow_within(&ds, 1000, "max_lifetime 生命周期内").await;
        drop(g);
        assert_eq!(ds.metrics().create_count(), 1, "第 {i} 轮应复用连接");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 越过物理寿命：下一次借用必须销毁旧连接并新建
    tokio::time::sleep(Duration::from_millis(220)).await;
    let g = borrow_within(&ds, 1000, "max_lifetime 过期后").await;
    assert_eq!(ds.metrics().create_count(), 2, "超过 max_lifetime 必须新建");
    assert_eq!(ds.metrics().destroy_count(), 1, "过期连接必须销毁");
    wait_until("过期连接物理关闭", || {
        closed.load(Ordering::SeqCst) == 1
    })
    .await;
    drop(g);
    assert_conserved(&ds, "max_lifetime 按 created_at");
}

/// 反例：test_on_borrow=false 时不得触发 validate（也不得因此销毁/重建连接）
#[tokio::test]
async fn test_on_borrow_false_skips_validation() {
    let driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst); // 校验必然失败
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a"); // cfg 已置 test_on_borrow=false
    c.max_active = 1;
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    for _ in 0..3 {
        let g = borrow_within(&ds, 1000, "test_on_borrow=false").await;
        assert!(g.execute("SELECT 1").await.is_ok());
        drop(g);
    }
    assert_eq!(ds.metrics().create_count(), 1, "应复用同一条连接");
    assert_eq!(ds.metrics().destroy_count(), 0);
    assert_eq!(closed.load(Ordering::SeqCst), 0);
    assert_conserved(&ds, "test_on_borrow=false");

    // 对照：开启校验后同一驱动必然失败并销毁连接
    let driver = MockDriver::new();
    driver.validate_ok.store(false, Ordering::SeqCst);
    let mut c2 = cfg("mock://b");
    c2.test_on_borrow = true;
    let ds2 = DruidDataSource::new(driver, c2);
    ds2.init().await.unwrap();
    assert!(ds2.get_connection().await.is_err());
    assert_eq!(ds2.metrics().destroy_count(), 1);
    assert_eq!(ds2.idle_count(), 0);
    assert_conserved(&ds2, "test_on_borrow=true 对照");
}

/// close() 幂等：串行两次 + 并发两次都不得 panic/重复销毁/残留空闲连接
#[tokio::test]
async fn close_is_idempotent() {
    let driver = MockDriver::new();
    let closed = driver.closed.clone();
    let mut c = cfg("mock://a");
    c.initial_size = 2;
    let ds = Arc::new(DruidDataSource::new(driver, c));
    ds.init().await.unwrap();

    let a = ds.clone();
    let b = ds.clone();
    let (ra, rb) = tokio::join!(
        tokio::spawn(async move { a.close().await }),
        tokio::spawn(async move { b.close().await })
    );
    ra.unwrap().unwrap();
    rb.unwrap().unwrap();
    ds.close().await.unwrap();

    assert_eq!(ds.idle_count(), 0);
    assert_eq!(closed.load(Ordering::SeqCst), 2, "每条连接只能物理关闭一次");
    assert_eq!(ds.metrics().destroy_count(), 2, "不得重复计入销毁");
    assert_conserved(&ds, "幂等 close");
}

// ══════════════ 7. KeepAlive 状态语义重写（最终轮） ══════════════

/// 摘出窗口的完整状态语义：借用方拿到**另一条**物理连接、窗口内守恒、结束后归位
///
/// 攻点：重写后 keepalive 与借用路径「同一套状态语义」是否真的等价 ——
/// 取许可 → 同锁内计入 active + 摘出 idle → 锁外校验 → return_to_idle / 销毁。
#[tokio::test]
async fn keepalive_window_borrower_gets_other_physical_connection() {
    let driver = OverlapDriver::new(300, 60);
    let overlaps = driver.overlaps.clone();
    let used_after_close = driver.used_after_close.clone();
    let validating = driver.validating.clone();
    let last_validated_id = driver.last_validated_id.clone();
    let mut c = cfg("mock://a");
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 40;
    c.initial_size = 1;
    c.max_active = 2; // 留出第二个许可，让借用可以在校验窗口内并发发生
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();
    assert_eq!(ds.pool_size(), 1);

    wait_until("KeepAlive 校验开始", || {
        validating.load(Ordering::SeqCst) > 0
    })
    .await;
    let validated_id = last_validated_id.load(Ordering::SeqCst);

    // 校验在途（300ms）：借用方必须另建连接，且不得与被校验的那条交叠
    let g = borrow_within(&ds, 1000, "KeepAlive 校验窗口内借用").await;
    assert_ne!(
        g.connection_id(),
        validated_id,
        "借到了正在被 KeepAlive 校验的连接"
    );
    assert!(g.execute("SELECT business").await.is_ok());
    assert_eq!(
        overlaps.load(Ordering::SeqCst),
        0,
        "同一条连接上校验与业务交叠"
    );
    assert_eq!(
        used_after_close.load(Ordering::SeqCst),
        0,
        "业务语句跑在了已关闭的连接上"
    );

    // 窗口内：被校验的连接计入 active，不在 idle；两条物理连接都被看见
    assert_eq!(ds.idle_count(), 0, "被校验的连接不得留在 idle");
    assert_eq!(ds.active_count(), 2, "被校验 + 业务借用各 1");
    assert_conserved(&ds, "KeepAlive 摘出窗口内");

    drop(g);
    // 校验完的连接回到 idle（用等待型断言：后台循环仍在跑，不做瞬时状态断言）
    wait_until("校验收敛、连接全部回池", || ds.idle_count() == 2).await;
    assert_eq!(ds.pool_size(), 2, "两条物理连接");
    assert_eq!(ds.metrics().create_count(), 2);
    assert_eq!(ds.metrics().destroy_count(), 0, "校验成功不得销毁连接");
    // 停掉后台循环后再验守恒（避免与后台结算窗口交错采样）
    ds.close().await.unwrap();
    assert_conserved(&ds, "KeepAlive 摘出窗口后");
}

/// close() 落在 KeepAlive 校验窗口内：连接必须被销毁**恰好一次**，不得重复结算
///
/// 两条可能的收尾路径（abort → `Validating::drop`；校验返回 → `return_to_idle`
/// 发现池已关闭）都会走销毁，本用例守住「只销毁一次 + 只通告一次 + 计数归位」。
#[tokio::test]
async fn close_during_keepalive_window_settles_exactly_once() {
    let log = Arc::new(EventLog::default());
    let driver = OverlapDriver::new(300, 0);
    let validating = driver.validating.clone();
    let closed_all = driver.closed_all.clone();
    let mut c = cfg("mock://a");
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 30;
    c.initial_size = 1;
    c.max_active = 2;
    let ds = DruidDataSource::with_filters(
        driver,
        c,
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    );
    ds.init().await.unwrap();

    wait_until("KeepAlive 校验开始", || {
        validating.load(Ordering::SeqCst) > 0
    })
    .await;
    assert_eq!(ds.active_count(), 1, "校验中的连接应计入 active");
    assert_conserved(&ds, "close 前（窗口内）");

    ds.close().await.unwrap();
    wait_until("校验中的连接被销毁", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    wait_until("物理关闭恰好一次", || {
        closed_all.load(Ordering::SeqCst) == 1
    })
    .await;
    // 再给一段窗口，确认没有第二次结算姗姗来迟
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ds.metrics().destroy_count(), 1, "不得重复计入销毁");
    assert_eq!(closed_all.load(Ordering::SeqCst), 1, "不得重复物理关闭");
    assert_eq!(
        log.events().iter().filter(|e| *e == "conn_closed").count(),
        1,
        "不得重复通告 filter chain: {:?}",
        log.events()
    );
    assert_eq!(ds.active_count(), 0, "abort/销毁后 active 必须归位");
    assert_eq!(ds.idle_count(), 0);
    assert_conserved(&ds, "close×KeepAlive 校验窗口");
}

/// 数据源析构（非 close）落在校验窗口内：`Validating` RAII 兜底必须完成结算
///
/// 这是 abort 路径的核心断言：active 归位、连接被物理关闭、filter chain 收到通告，
/// 且**不重复**（Take 与 Drop 两条路径互斥）。
#[tokio::test]
async fn datasource_drop_during_keepalive_window_settles_once() {
    let log = Arc::new(EventLog::default());
    let driver = OverlapDriver::new(300, 0);
    let validating = driver.validating.clone();
    let closed_all = driver.closed_all.clone();
    let mut c = cfg("mock://a");
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 30;
    c.initial_size = 1;
    c.max_active = 2;
    let ds = Arc::new(DruidDataSource::with_filters(
        driver,
        c,
        vec![Box::new(LogFilter(log.clone())) as Box<dyn Filter>],
    ));
    ds.init().await.unwrap();

    wait_until("KeepAlive 校验开始", || {
        validating.load(Ordering::SeqCst) > 0
    })
    .await;
    let m = ds.metrics_arc(); // 数据源析构后仍可观测
    drop(ds); // abort 后台循环：结算只能靠 Validating::drop

    wait_until("RAII 兜底计入销毁", || m.destroy_count() == 1).await;
    wait_until("RAII 兜底物理关闭", || {
        closed_all.load(Ordering::SeqCst) == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(m.destroy_count(), 1, "不得重复计入销毁");
    assert_eq!(closed_all.load(Ordering::SeqCst), 1, "不得重复物理关闭");
    assert_eq!(m.active(), 0, "abort 后 active 必须归位（不得永久虚高）");
    assert_eq!(m.idle(), 0);
    assert_eq!(m.create_count(), m.destroy_count(), "析构后不得残留连接");
    assert_eq!(
        log.events().iter().filter(|e| *e == "conn_closed").count(),
        1,
        "不得重复通告 filter chain: {:?}",
        log.events()
    );
}

/// `try_acquire_owned()` 拿不到许可就跳过：借用中的连接绝不被打扰，归还后立刻自愈
///
/// 回答「池满时 keepalive 完全不做校验会不会让坏连接长期滞留」：
/// 池满 ⇒ idle 必为空（每条 idle 连接的许可在归还时即已释放），
/// 所以跳过只发生在**无空闲连接可校验**时；连接一回到 idle，下一轮就会校验并驱逐。
#[tokio::test]
async fn keepalive_skips_when_pool_exhausted_then_evicts_dead_conn() {
    let driver = OverlapDriver::new(0, 0);
    driver.validate_ok.store(false, Ordering::SeqCst); // 上游已是死连接
    let closed_all = driver.closed_all.clone();
    let mut c = cfg("mock://a");
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 30;
    c.initial_size = 1;
    c.max_active = 1; // 池满 ⇒ 唯一许可被借用方持有
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    let g = borrow_within(&ds, 1000, "长期借用唯一许可").await;
    assert_eq!(
        ds.idle_count(),
        0,
        "池满时 idle 必为空 → 不存在「坏连接滞留 idle」"
    );
    tokio::time::sleep(Duration::from_millis(150)).await; // 数个 keepalive 轮次全部跳过
    assert_eq!(
        ds.metrics().destroy_count(),
        0,
        "借用中的连接不得被校验/销毁"
    );
    assert_eq!(closed_all.load(Ordering::SeqCst), 0);
    assert!(g.execute("SELECT 1").await.is_ok(), "业务连接完好");

    drop(g); // 归还 → 下一轮 keepalive 校验失败 → 驱逐
    wait_until("归还后坏连接被驱逐", || {
        ds.metrics().destroy_count() == 1
    })
    .await;
    wait_until("物理关闭", || closed_all.load(Ordering::SeqCst) == 1).await;
    assert_eq!(ds.idle_count(), 0, "坏连接不得退回 idle");
    assert_conserved(&ds, "KeepAlive 跳过分支后驱逐"); // 此刻 idle 空且无在途结算，是静止点

    // 驱逐后重建：新连接可用（持有许可期间 keepalive 不会碰它）
    let g2 = borrow_within(&ds, 1000, "驱逐后重建").await;
    assert_eq!(ds.metrics().create_count(), 2);
    assert!(g2.execute("SELECT 2").await.is_ok());
    drop(g2);
    ds.close().await.unwrap(); // 停掉后台循环（它会校验并驱逐这条新的坏连接）
    assert_conserved(&ds, "KeepAlive 驱逐后重建");
}

/// KeepAlive 不得给连接「续命」：校验后回池不得让 max_lifetime 失效
///
/// 重写引入了新局面：keepalive 会把校验完的连接 `return_to_idle`（刷新 last_used_at）。
/// 若寿命判定被误改成看 last_used_at，热点/被反复校验的连接就会永生。
/// 这里关掉驱逐循环，只靠借用路径的 `entry_alive` 判定：到期后借用必须销毁重建。
#[tokio::test]
async fn keepalive_does_not_extend_max_lifetime() {
    let driver = OverlapDriver::new(0, 0);
    let validating = driver.validating.clone();
    let closed_all = driver.closed_all.clone();
    let mut c = cfg("mock://a"); // cfg 已关闭驱逐循环，排除驱逐的干扰
    c.initial_size = 1;
    c.max_active = 1;
    c.max_lifetime_ms = 60;
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 25; // 期间会反复校验并回池
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    // 等 KeepAlive 至少跑过一轮（连接被摘出又放回，last_used_at 被刷新）
    wait_until("KeepAlive 校验一轮", || {
        validating.load(Ordering::SeqCst) > 0
    })
    .await;
    wait_until("校验完的连接回到 idle", || ds.idle_count() == 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await; // 远超 max_lifetime=60ms
    wait_until(
        "到期连接仍在 idle（keepalive 不销毁它）",
        || ds.idle_count() == 1,
    )
    .await;

    // 到期后借用：必须销毁旧连接并新建（寿命按 created_at，不因校验回池而延长）
    let g = borrow_within(&ds, 1000, "寿命到期后借用").await;
    assert_eq!(ds.metrics().create_count(), 2, "超过 max_lifetime 必须新建");
    assert_eq!(ds.metrics().destroy_count(), 1, "到期连接必须销毁");
    wait_until("到期连接物理关闭", || {
        closed_all.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(g.execute("SELECT 1").await.is_ok(), "新建连接可用");
    drop(g);
    // 此处可安全断言：驱逐循环未启用，且 keepalive 校验恒成功（不销毁），
    // 唯一的在途变更只可能是「摘出/回池」，二者都在同一把锁内改 active/idle，快照自洽
    assert_conserved(&ds, "KeepAlive 不延长 max_lifetime");
    ds.close().await.unwrap();
}

/// KeepAlive 窗口 × 归还校验窗口 × 驱逐 × 业务流量：四条路径共用同一套结算函数
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keepalive_and_return_window_do_not_interfere() {
    let driver = OverlapDriver::new(5, 2);
    let closed_all = driver.closed_all.clone();
    let overlaps = driver.overlaps.clone();
    let used_after_close = driver.used_after_close.clone();
    let mut c = cfg("mock://a");
    c.max_active = 3;
    c.initial_size = 2;
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 15;
    c.test_on_return = true; // 归还窗口也走 settle_active / return_to_idle
    c.time_between_eviction_runs_ms = 25; // 驱逐循环同时在场
    let ds = Arc::new(DruidDataSource::new(driver, c));
    ds.init().await.unwrap();

    let mut tasks = Vec::new();
    for i in 0..6 {
        let ds = ds.clone();
        tasks.push(tokio::spawn(async move {
            for j in 0..10 {
                let g = ds.get_connection().await.unwrap();
                assert_eq!(g.execute(&format!("SELECT {i}-{j}")).await.unwrap(), 1);
                drop(g);
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // 业务流量已停：等各条收尾路径收敛，再验「一次销毁 = 一次物理关闭」
    tokio::time::sleep(Duration::from_millis(150)).await;
    wait_until("销毁与物理关闭收敛", || {
        closed_all.load(Ordering::SeqCst) == ds.metrics().destroy_count()
    })
    .await;
    assert_eq!(
        overlaps.load(Ordering::SeqCst),
        0,
        "同一条连接上校验与业务交叠"
    );
    assert_eq!(
        used_after_close.load(Ordering::SeqCst),
        0,
        "业务语句跑在了已关闭的连接上"
    );
    assert!(
        ds.pool_size() <= ds.max_active(),
        "物理连接数超过 max_active: pool_size={}",
        ds.pool_size()
    );

    // 守恒只在后台循环停下后的静止点断言（在途结算的同步段跨线程不可原子观测）
    ds.close().await.unwrap();
    wait_until("关闭后全部销毁", || {
        ds.metrics().destroy_count() == ds.metrics().create_count()
    })
    .await;
    assert_eq!(ds.pool_size(), 0);
    assert_conserved(&ds, "压测后关闭");
}

/// 校验窗口导致 idle 暂时低于 min_idle 时，驱逐循环不得把池挖穿
#[tokio::test]
async fn keepalive_window_does_not_break_min_idle_under_eviction() {
    let driver = OverlapDriver::new(80, 0);
    let closed_all = driver.closed_all.clone();
    let overlaps = driver.overlaps.clone();
    let mut c = cfg("mock://a");
    c.max_active = 3;
    c.initial_size = 2;
    c.min_idle = 1;
    c.keep_alive = true;
    c.keep_alive_between_time_ms = 30;
    c.time_between_eviction_runs_ms = 30;
    c.max_evictable_idle_time_ms = 0; // 只要高于 min_idle 就驱逐，放大驱逐压力
    let ds = DruidDataSource::new(driver, c);
    ds.init().await.unwrap();

    // 跑若干轮驱逐 × KeepAlive，全程只观测不干预
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            ds.pool_size() <= ds.max_active(),
            "物理连接数超过 max_active: {}",
            ds.pool_size()
        );
        assert_eq!(
            overlaps.load(Ordering::SeqCst),
            0,
            "同一条连接上校验与业务交叠"
        );
    }

    // min_idle 是驱逐的下界：空闲连接数最终必须回到 min_idle 之上
    wait_until("空闲连接回到 min_idle 之上", || {
        ds.idle_count() >= 1
    })
    .await;
    // 停掉两个后台循环后再验收尾（销毁先记数、物理关闭随后落地，在途时不做瞬时断言）
    ds.close().await.unwrap();
    wait_until("一次销毁对应一次物理关闭", || {
        closed_all.load(Ordering::SeqCst) == ds.metrics().destroy_count()
    })
    .await;
    assert_eq!(
        ds.metrics().destroy_count(),
        ds.metrics().create_count(),
        "关闭后不得残留未销毁连接"
    );
    assert_eq!(ds.pool_size(), 0);
    assert_conserved(&ds, "驱逐×KeepAlive×min_idle");
}

// ══════════════ 辅助：慢执行驱动 ══════════════

/// execute 挂起 latency 的连接（用于在 execute 在途时取消）
#[derive(Debug)]
struct SlowConn {
    id: u64,
    latency: Duration,
    closed: Arc<AtomicU64>,
}

#[async_trait]
impl Connection for SlowConn {
    async fn execute(&self, _sql: &str) -> Result<u64, DruidError> {
        tokio::time::sleep(self.latency).await;
        Ok(1)
    }
    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        tokio::time::sleep(self.latency).await;
        Ok(vec![vec!["row".into()]])
    }
    async fn close(&self) -> Result<(), DruidError> {
        self.closed.fetch_add(1, Ordering::SeqCst);
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
struct SlowDriver {
    latency: Duration,
    closed: Arc<AtomicU64>,
    ids: AtomicU64,
}

impl SlowDriver {
    fn new(latency: Duration) -> Self {
        SlowDriver {
            latency,
            closed: Arc::new(AtomicU64::new(0)),
            ids: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl Driver for SlowDriver {
    type Connection = SlowConn;
    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
        _timeout: Option<Duration>,
    ) -> Result<SlowConn, DruidError> {
        Ok(SlowConn {
            id: self.ids.fetch_add(1, Ordering::SeqCst) + 1,
            latency: self.latency,
            closed: self.closed.clone(),
        })
    }
    fn name(&self) -> &'static str {
        "SlowDriver"
    }
    async fn validate(&self, _conn: &SlowConn) -> Result<(), DruidError> {
        Ok(())
    }
}

/// execute 必 panic 的连接（用于验证 unwind 路径下的兜底闭合）
#[derive(Debug)]
struct PanicConn(u64);

#[async_trait]
impl Connection for PanicConn {
    async fn execute(&self, sql: &str) -> Result<u64, DruidError> {
        panic!("driver panic on {sql}");
    }
    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        Ok(vec![vec!["row".into()]])
    }
    async fn close(&self) -> Result<(), DruidError> {
        Ok(())
    }
    async fn ping(&self) -> Result<(), DruidError> {
        Ok(())
    }
    fn connection_id(&self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
struct PanicDriver;

#[async_trait]
impl Driver for PanicDriver {
    type Connection = PanicConn;
    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
        _timeout: Option<Duration>,
    ) -> Result<PanicConn, DruidError> {
        Ok(PanicConn(1))
    }
    fn name(&self) -> &'static str {
        "PanicDriver"
    }
    async fn validate(&self, _conn: &PanicConn) -> Result<(), DruidError> {
        Ok(())
    }
}

/// 记录「同一物理连接是否被并发使用 / 是否被用在已关闭的连接上」的连接。
/// 标记按**连接**粒度（每条连接自带 validating/executing），因此不同物理连接上
/// 同时发生的校验与业务语句不会被误判为交叠；`driver.validating` 才是全局信号
/// （供测试等待「校验已开始」）。
#[derive(Debug)]
struct OverlapConn {
    id: u64,
    /// 本连接正在被校验
    own_validating: Arc<AtomicU64>,
    /// 本连接正在被业务使用
    own_executing: Arc<AtomicU64>,
    /// 本连接已被物理关闭
    closed: Arc<AtomicBool>,
    overlaps: Arc<AtomicU64>,
    used_after_close: Arc<AtomicU64>,
    closed_all: Arc<AtomicU64>,
    exec_ms: u64,
}

#[async_trait]
impl Connection for OverlapConn {
    async fn execute(&self, _sql: &str) -> Result<u64, DruidError> {
        if self.closed.load(Ordering::SeqCst) {
            self.used_after_close.fetch_add(1, Ordering::SeqCst);
        }
        // 进入业务语句时本连接仍有校验在途 → 同一物理连接被并发使用
        if self.own_validating.load(Ordering::SeqCst) > 0 {
            self.overlaps.fetch_add(1, Ordering::SeqCst);
        }
        self.own_executing.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.exec_ms)).await;
        self.own_executing.fetch_sub(1, Ordering::SeqCst);
        Ok(1)
    }
    async fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        self.execute(sql).await?;
        Ok(vec![vec!["row".into()]])
    }
    async fn close(&self) -> Result<(), DruidError> {
        // 重复 close 只计一次（与真实驱动一致）
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.closed_all.fetch_add(1, Ordering::SeqCst);
        }
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
struct OverlapDriver {
    validate_ms: u64,
    exec_ms: u64,
    /// 校验结果开关（默认通过）
    validate_ok: AtomicBool,
    ids: AtomicU64,
    /// 全局「正在校验」计数（含任意连接）
    validating: Arc<AtomicU64>,
    overlaps: Arc<AtomicU64>,
    used_after_close: Arc<AtomicU64>,
    closed_all: Arc<AtomicU64>,
    /// 最近一条被校验的连接 id
    last_validated_id: Arc<AtomicU64>,
}

impl OverlapDriver {
    fn new(validate_ms: u64, exec_ms: u64) -> Self {
        OverlapDriver {
            validate_ms,
            exec_ms,
            validate_ok: AtomicBool::new(true),
            ids: AtomicU64::new(0),
            validating: Arc::new(AtomicU64::new(0)),
            overlaps: Arc::new(AtomicU64::new(0)),
            used_after_close: Arc::new(AtomicU64::new(0)),
            closed_all: Arc::new(AtomicU64::new(0)),
            last_validated_id: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait]
impl Driver for OverlapDriver {
    type Connection = OverlapConn;
    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
        _timeout: Option<Duration>,
    ) -> Result<OverlapConn, DruidError> {
        Ok(OverlapConn {
            id: self.ids.fetch_add(1, Ordering::SeqCst) + 1,
            own_validating: Arc::new(AtomicU64::new(0)),
            own_executing: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
            overlaps: self.overlaps.clone(),
            used_after_close: self.used_after_close.clone(),
            closed_all: self.closed_all.clone(),
            exec_ms: self.exec_ms,
        })
    }
    fn name(&self) -> &'static str {
        "OverlapDriver"
    }
    async fn validate(&self, conn: &OverlapConn) -> Result<(), DruidError> {
        self.validating.fetch_add(1, Ordering::SeqCst);
        self.last_validated_id.store(conn.id, Ordering::SeqCst);
        conn.own_validating.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(self.validate_ms)).await;
        // 校验期间有业务语句跑过**同一条**连接
        if conn.own_executing.load(Ordering::SeqCst) > 0 {
            self.overlaps.fetch_add(1, Ordering::SeqCst);
        }
        conn.own_validating.fetch_sub(1, Ordering::SeqCst);
        self.validating.fetch_sub(1, Ordering::SeqCst);
        if self.validate_ok.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(DruidError::Pool("invalid".into()))
        }
    }
}

// ══════════════ 攻不动清单（尝试过但未找到漏洞） ══════════════
//
// 第一轮（含 20 次连跑）：
//   · 取消点①②④⑤：计数、waiting、permit 全部正确归位，无连接泄漏
//   · 取消点③（复用连接在 validate 中取消）：被摘出的连接确实被物理关闭
//   · close×guard Drop（test_on_return 开/关）：连接不落入已关闭池，物理关闭恰好一次
//   · close×init 回填：任意交错下 create == destroy，无残留
//   · 并发压测：未观察到同一 conn_id 被两个 guard 同时持有
//   · max_active=1 重复借还：permit 释放顺序正确，物理连接始终 1 条
//   · close() 幂等（串行/并发）：无重复销毁、无 panic
//   · with_filters(vec![]) ≡ new()；test_on_borrow=false 不触发校验
//   · 逃生通道 guard.connection() 确实绕过全部 filter 钩子
//
// 第二轮（针对新增不变量）：
//   · 交付前 is_closed() 复查：close×connect / close×borrow-validate 两处窗口都已关闭
//     （close() 返回后不再交付；在途连接被 Drop 物理销毁，无残留）
//   · 归还校验窗口语义：窗口内连接计入 active、守恒成立；第二个借用者在
//     max_active=1 下正确**等待**而非报错，且不新建第二条物理连接（permit 未提前释放）
//   · close() 落在归还窗口内：窗口结束时连接被销毁而非入池（close 的 idle 排空够不到它，
//     由后台任务在 return_to_idle 里发现 closed 后物理关闭）
//   · semaphore.close()：关闭后新借用立即失败（不挂起）、重复 close 幂等、
//     close 后 init 不复活、唤醒的排队者拿到 DruidError::Pool
//   · StmtScope 兜底闭合：execute 成功/拦截/panic 展开/取消各路径恰好闭合一次，
//     钩子顺序 created→before→after|err→closed 保持；未 poll 就 forget 的 future
//     不触发任何钩子（async fn 体首次 poll 才执行，非缺陷）
//   · config.filters 非空在 init() 硬失败，且校验先于 inited.swap：
//     失败后 get_connection 报 not initialized，with_filters 注入不受影响
//   · pool_size() 单锁快照与 active/idle 分量一致；close×交付竞态 20 轮无连接失联
//
// 仍未修复的残余风险（非本轮引入，已固化在用例中）：
//   · test_on_return 校验无超时（validation_query_timeout_secs=0 默认）：
//     校验任务挂起时 permit 被其持有，max_active 容量永久缺失（hung_return_validation_swallows_permit）
//   · close() 唤醒排队者的错误文案是 "semaphore closed" 而非 "datasource is closed"（仅文案）
