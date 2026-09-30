use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use druid_core::{DruidConfig, DruidError};
use druid_filter::manager::FilterManager;
use druid_filter::{Filter, FilterChain};
use druid_stat::metrics::PoolMetrics;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::background::{self, spawn_close, validate_with_timeout};
use crate::driver::{Connection, Driver};
use crate::guard::PoolGuard;
use crate::pscache::PSCache;

/// 空闲池中的一条物理连接
pub(crate) struct PoolEntry<C: Connection> {
    pub(crate) conn: Arc<C>,
    /// 最近归还/创建时刻，仅用于空闲驱逐
    pub(crate) last_used_at: Instant,
    /// 物理连接创建时刻，用于 max_lifetime 判定（归还时不刷新）
    pub(crate) created_at: Instant,
    pub(crate) id: u64,
}

pub(crate) struct PoolInner<C: Connection> {
    pub(crate) idle: VecDeque<PoolEntry<C>>,
    pub(crate) active_count: usize,
    pub(crate) closed: bool,
}

impl<C: Connection> PoolInner<C> {
    fn new() -> Self {
        PoolInner {
            idle: VecDeque::new(),
            active_count: 0,
            closed: false,
        }
    }
}

pub struct DruidDataSource<D: Driver> {
    driver: Arc<D>,
    config: DruidConfig,
    semaphore: Arc<Semaphore>,
    inner: Arc<Mutex<PoolInner<D::Connection>>>,
    filter_chain: Arc<FilterChain>,
    metrics: Arc<PoolMetrics>,
    pscache: Mutex<PSCache>,
    next_id: AtomicU64,
    inited: AtomicBool,
    evict_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    keepalive_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<D: Driver> DruidDataSource<D> {
    /// 创建数据源（不挂载 Filter）
    pub fn new(driver: D, config: DruidConfig) -> Self {
        let chain = FilterChain::new(&config.url);
        Self::build(driver, config, chain)
    }

    /// 创建数据源并在构造期注入 Filter（SQL 防火墙 / 统计等接入点）
    ///
    /// 链在包装为 `Arc<FilterChain>` 前完成配置，运行期只读，因此无注册竞态。
    /// 需要保留 Filter 句柄时传入 `Arc<StatFilter>` 等共享实例的克隆
    /// （`Arc<T: Filter>` 本身即 Filter）。
    pub fn with_filters(driver: D, config: DruidConfig, mut filters: Vec<Box<dyn Filter>>) -> Self {
        if let Err(e) = FilterManager::init_filters(&mut filters) {
            tracing::error!("Filter 初始化失败，仍按配置挂载: {}", e);
        }
        let chain = FilterManager::create_chain(&config.url, filters);
        Self::build(driver, config, chain)
    }

    /// 创建数据源并接管一个已构建好的 FilterChain
    pub fn with_chain(driver: D, config: DruidConfig, chain: FilterChain) -> Self {
        Self::build(driver, config, chain)
    }

    fn build(driver: D, config: DruidConfig, chain: FilterChain) -> Self {
        let max = config.max_active.max(1);
        let driver = Arc::new(driver);
        let ps_cache_size = if config.pool_prepared_statements {
            config.max_pool_prepared_statement_per_connection_size
        } else {
            0
        };
        DruidDataSource {
            driver,
            config,
            semaphore: Arc::new(Semaphore::new(max)),
            inner: Arc::new(Mutex::new(PoolInner::new())),
            filter_chain: Arc::new(chain),
            metrics: Arc::new(PoolMetrics::new()),
            pscache: Mutex::new(PSCache::new(ps_cache_size)),
            next_id: AtomicU64::new(1),
            inited: AtomicBool::new(false),
            evict_handle: Mutex::new(None),
            keepalive_handle: Mutex::new(None),
        }
    }

    pub async fn init(&self) -> Result<(), DruidError> {
        if self.is_closed() {
            return Err(DruidError::Pool("datasource is closed".into()));
        }
        // 参数自洽性校验：initial_size > max_active 这类配置会静默开出借不出去的连接
        self.config.validate()?;
        // `filters` 无法按名实例化（druid-pool 不依赖具体 Filter 实现），曾经只是空操作：
        // 用户配了 filters = ["wall"] 却没有任何墙，是**安全配置的静默失效**。
        // 这里硬报错而不是打日志——日志在生产里常常没人看。
        if !self.config.filters.is_empty() {
            return Err(DruidError::Config(
                "DruidConfig::filters 未接线（无法按名实例化，druid-pool 不依赖具体 Filter 实现）；\
                 请改用 DruidDataSource::with_filters(...) 注入"
                    .into(),
            ));
        }
        if self.inited.swap(true, Ordering::SeqCst) {
            return Err(DruidError::Pool("already initialized".into()));
        }
        self.warn_ineffective_configs();
        self.filter_chain.data_source_inited();

        // initial_size <= max_active 已由 validate() 前置保证，无需再钳制
        let initial = self.config.initial_size;
        for _ in 0..initial {
            if self.is_closed() {
                break; // close 与 init 竞态：停止回填
            }
            let Ok(entry) = self.create_entry().await else {
                continue;
            };
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if g.closed {
                drop(g);
                self.metrics.inc_destroy();
                self.filter_chain.connection_closed(entry.id);
                spawn_close(entry.conn);
                break;
            }
            g.idle.push_back(entry);
        }
        self.metrics.set_idle(self.idle_count());
        self.metrics.set_active(self.active_count());

        if self.is_closed() {
            return Ok(()); // 已关闭：不再启动无人可停的后台循环
        }

        // 后台维护循环（空闲驱逐 / KeepAlive）
        if self.config.time_between_eviction_runs_ms > 0 {
            let handle = background::spawn_eviction_loop::<D>(
                self.inner.clone(),
                self.filter_chain.clone(),
                self.metrics.clone(),
                self.config.clone(),
            );
            *self.evict_handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }
        if self.config.keep_alive {
            let handle = background::spawn_keepalive_loop::<D>(
                self.inner.clone(),
                self.driver.clone(),
                self.metrics.clone(),
                self.filter_chain.clone(),
                self.semaphore.clone(),
                self.config.clone(),
            );
            *self
                .keepalive_handle
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }

        tracing::info!(
            "DruidDataSource init: max={}, init={}",
            self.config.max_active,
            initial
        );
        Ok(())
    }

    /// 提示配置了但当前版本未生效的配置项（一次性汇总，避免刷屏）
    fn warn_ineffective_configs(&self) {
        let d = DruidConfig::default();
        let c = &self.config;
        let mut items: Vec<&str> = Vec::new();
        if c.min_evictable_idle_time_ms != d.min_evictable_idle_time_ms {
            items.push("min_evictable_idle_time_ms");
        }
        if c.test_while_idle {
            items.push("test_while_idle");
        }
        if c.validation_query.is_some() {
            items.push("validation_query");
        }
        if c.socket_timeout_secs != d.socket_timeout_secs {
            items.push("socket_timeout_secs");
        }
        if !c.connection_properties.is_empty() {
            items.push("connection_properties");
        }
        if c.driver_class_name.is_some() {
            items.push("driver_class_name");
        }
        if !items.is_empty() {
            tracing::warn!("DruidDataSource 配置项未生效: {}", items.join(", "));
        }
        if c.pool_prepared_statements {
            tracing::warn!("pool_prepared_statements 仅创建了 PSCache 容器，尚未接入借用路径");
        }
    }

    fn is_closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    /// 获取并发许可：受 max_wait 限制
    async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit, DruidError> {
        let acquire = self.semaphore.clone().acquire_owned();
        let result = match self.config.max_wait() {
            Some(max_wait) => tokio::time::timeout(max_wait, acquire)
                .await
                .map_err(|_| DruidError::Pool("connection wait timeout".into()))?,
            None => acquire.await,
        };
        result.map_err(|_| DruidError::Pool("semaphore closed".into()))
    }

    /// max_lifetime 判定：按物理连接创建时刻，而非最近使用时刻
    fn entry_alive(&self, e: &PoolEntry<D::Connection>) -> bool {
        self.config.max_lifetime_ms == 0
            || e.created_at.elapsed().as_millis() as u64 <= self.config.max_lifetime_ms
    }

    pub async fn get_connection(&self) -> Result<PoolGuard<D>, DruidError> {
        if self.is_closed() {
            return Err(DruidError::Pool("datasource is closed".into()));
        }
        if !self.inited.load(Ordering::SeqCst) {
            return Err(DruidError::Pool("datasource not initialized".into()));
        }

        let start = Instant::now();
        self.metrics.inc_waiting();

        // 提前构造 Guard：connect/validate 等 await 都发生在 Guard 存在之后，
        // 这样 future 被 drop（如 timeout 取消）时由 Drop 兜底——计数归位、
        // 在途物理连接关闭，不会泄漏，active/waiting 也不会虚高固化。
        let mut guard = PoolGuard::pending(
            self.inner.clone(),
            self.filter_chain.clone(),
            self.metrics.clone(),
            self.driver.clone(),
            self.config.test_on_return,
            self.config.validation_query_timeout_secs,
        );

        // 1) 等待并发许可（max_active 闸门）
        guard.permit = Some(self.acquire_permit().await?);
        // 等待期间数据源可能已被 close()：此时不得再创建/交付连接
        if self.is_closed() {
            return Err(DruidError::Pool("datasource is closed".into()));
        }
        let wait_ms = start.elapsed().as_millis() as u64;

        // 2) 摘取空闲连接并计入 active
        let idle_entry = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.active_count += 1;
            self.metrics.set_active(g.active_count);
            let e = g.idle.pop_front();
            self.metrics.set_idle(g.idle.len());
            e
        };
        guard.counted = true;

        // 3) 复用空闲连接，或（无空闲/已过期时）新建
        match idle_entry.as_ref().filter(|e| self.entry_alive(e)) {
            Some(e) => {
                self.metrics.inc_cache_hit();
                guard.conn_id = e.id;
                guard.created_at = e.created_at;
                guard.conn = Some(e.conn.clone());
            }
            None => {
                if let Some(expired) = idle_entry {
                    // 超过 max_lifetime 的空闲连接直接销毁
                    self.metrics.inc_destroy();
                    self.filter_chain.connection_closed(expired.id);
                    spawn_close(expired.conn);
                }
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                guard.conn_id = id;
                // connect 失败/被取消时 guard.conn 仍为空，Drop 只做计数归位
                let c = self
                    .driver
                    .connect(
                        &self.config.url,
                        &self.config.username,
                        &self.config.password,
                        Some(self.config.connect_timeout()),
                    )
                    .await?;
                // 连接已产生，之后任何取消都由 Drop 负责关闭
                guard.conn = Some(Arc::new(c));
                guard.created_at = Instant::now();
                self.metrics.inc_create();
                self.filter_chain.connection_created(id);
                // connect 在途时可能已被 close()：不得交付 close() 看不见的连接（Drop 销毁它）
                if self.is_closed() {
                    return Err(DruidError::Pool("datasource is closed".into()));
                }
            }
        }

        // 4) 借出前钩子
        self.filter_chain.connection_borrow_before(guard.conn_id);

        // 5) 借用校验（受 validation_query_timeout_secs 约束，同时收窄取消窗口）
        if self.config.test_on_borrow {
            let conn = guard.conn.as_ref().expect("连接已在上一步创建或复用");
            if let Err(e) = validate_with_timeout(
                &*self.driver,
                conn,
                self.config.validation_query_timeout_secs,
            )
            .await
            {
                tracing::warn!(
                    "test_on_borrow validation failed for connection {}: {}",
                    guard.conn_id,
                    e
                );
                self.filter_chain.connection_error(guard.conn_id, &e);
                // healthy=false → Drop 物理关闭该连接
                return Err(e);
            }
        }

        // 6) 交付前最后一次复查：校验在途时可能已被 close()
        //    （check-then-await 的窗口不限于 connect，凡 is_closed() 之后还有 await 都要复查）
        if self.is_closed() {
            return Err(DruidError::Pool("datasource is closed".into()));
        }

        // 7) 成功交付
        self.metrics.inc_borrow();
        self.metrics
            .add_wait_time_ns(start.elapsed().as_nanos() as u64);
        self.filter_chain
            .connection_borrowed(guard.conn_id, wait_ms);
        // 等待在此结束：立刻归位 waiting，并置位避免 Drop 重复扣减
        self.metrics.dec_waiting();
        guard.waiting = false;
        guard.healthy = true;
        Ok(guard)
    }

    async fn create_entry(&self) -> Result<PoolEntry<D::Connection>, DruidError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let conn = self
            .driver
            .connect(
                &self.config.url,
                &self.config.username,
                &self.config.password,
                Some(self.config.connect_timeout()),
            )
            .await
            .map(Arc::new)?;
        self.metrics.inc_create();
        self.filter_chain.connection_created(id);
        Ok(PoolEntry {
            conn,
            last_used_at: Instant::now(),
            created_at: Instant::now(),
            id,
        })
    }

    // ── 状态查询 ──

    pub fn active_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_count
    }
    pub fn idle_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .idle
            .len()
    }
    pub fn max_active(&self) -> usize {
        self.config.max_active
    }
    /// 池内物理连接总数（active + idle），在同一把锁下取样。
    ///
    /// 并发下分别调用 `active_count()` / `idle_count()` 会读到两个不同瞬间，
    /// 可能把一次正常归还读成「总数超过 max_active」的假象。
    pub fn pool_size(&self) -> usize {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.active_count + g.idle.len()
    }
    /// Filter 链（构造期注入，运行期只读）
    pub fn filter_chain(&self) -> &FilterChain {
        &self.filter_chain
    }
    pub fn metrics(&self) -> &PoolMetrics {
        &self.metrics
    }
    /// 池内权威指标的共享句柄。
    ///
    /// `metrics()` 只给引用，而 `StatFilter::bind_metrics` 需要所有权（绑定后
    /// 控制台的 active/idle 直接读池内计数，不再由 StatFilter 自行累加，避免漂移）：
    /// ```ignore
    /// stat.bind_metrics(ds.metrics_arc());
    /// ```
    pub fn metrics_arc(&self) -> Arc<PoolMetrics> {
        self.metrics.clone()
    }
    /// PreparedStatement 缓存容器；⚠️ 当前仅暴露未接入借用路径，开关无实际效果
    pub fn pscache(&self) -> &Mutex<PSCache> {
        &self.pscache
    }

    pub async fn close(&self) -> Result<(), DruidError> {
        self.stop_background_loops();
        // 唤醒所有在 semaphore 上排队的借用者（否则持有的 permit 不归还时永久挂起）；
        // 它们醒来后因池已关闭而失败
        self.semaphore.close();
        let conns: Vec<PoolEntry<D::Connection>> = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.closed = true;
            g.idle.drain(..).collect()
        };
        for e in conns {
            self.metrics.inc_destroy();
            self.filter_chain.connection_closed(e.id);
            let _ = e.conn.close().await;
        }
        tracing::info!("DruidDataSource closed");
        Ok(())
    }

    fn stop_background_loops(&self) {
        for slot in [&self.evict_handle, &self.keepalive_handle] {
            if let Some(h) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                h.abort();
            }
        }
    }
}

impl<D: Driver> Drop for DruidDataSource<D> {
    fn drop(&mut self) {
        // 未显式 close 就丢弃数据源：停掉后台循环并排空空闲连接
        self.stop_background_loops();
        let drained: Vec<PoolEntry<D::Connection>> = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.closed = true;
            g.idle.drain(..).collect()
        };
        if drained.is_empty() {
            return;
        }
        let metrics = self.metrics.clone();
        let fchain = self.filter_chain.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    for e in drained {
                        metrics.inc_destroy();
                        fchain.connection_closed(e.id);
                        let _ = e.conn.close().await;
                    }
                });
            }
            Err(_) => tracing::warn!(
                "DruidDataSource 在 tokio 运行时外析构，{} 条空闲连接未显式关闭",
                drained.len()
            ),
        }
    }
}
