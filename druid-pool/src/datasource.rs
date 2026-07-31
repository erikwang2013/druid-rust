use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use druid_core::{DruidConfig, DruidError};
use druid_filter::FilterChain;
use druid_stat::metrics::PoolMetrics;
use tokio::sync::Semaphore;

use crate::driver::{Connection, Driver};
use crate::pscache::PSCache;

struct PoolEntry<C: Connection> {
    conn: Arc<C>,
    last_used_at: Instant,
    id: u64,
}

struct PoolInner<C: Connection> {
    idle: VecDeque<PoolEntry<C>>,
    active_count: usize,
    closed: bool,
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
    pub fn new(driver: D, config: DruidConfig) -> Self {
        let max = config.max_active.max(1);
        let driver = Arc::new(driver);
        let fc = Arc::new(FilterChain::new(&config.url));
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
            filter_chain: fc,
            metrics: Arc::new(PoolMetrics::new()),
            pscache: Mutex::new(PSCache::new(ps_cache_size)),
            next_id: AtomicU64::new(1),
            inited: AtomicBool::new(false),
            evict_handle: Mutex::new(None),
            keepalive_handle: Mutex::new(None),
        }
    }

    pub async fn init(&self) -> Result<(), DruidError> {
        if self.inited.swap(true, Ordering::SeqCst) {
            return Err(DruidError::Pool("already initialized".into()));
        }
        self.filter_chain.data_source_inited();

        for _ in 0..self.config.initial_size {
            if let Ok(e) = self.create_entry().await {
                self.inner.lock().unwrap().idle.push_back(e);
            }
        }
        self.metrics.set_idle(self.idle_count());
        self.metrics.set_active(self.active_count());

        // eviction and keepalive threads remain unchanged...
        if self.config.time_between_eviction_runs_ms > 0 {
            let inner = self.inner.clone();
            let fchain = self.filter_chain.clone();
            let metrics = self.metrics.clone();
            let max_ms = self.config.max_evictable_idle_time_ms;
            let min_idle = self.config.min_idle;
            let interval = self.config.eviction_interval();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let now = Instant::now();
                    let mut g = inner.lock().unwrap();
                    while g.idle.len() > min_idle {
                        let should = g.idle.front().is_some_and(|e| {
                            now.duration_since(e.last_used_at).as_millis() > max_ms as u128
                        });
                        if should {
                            if let Some(e) = g.idle.pop_front() {
                                fchain.connection_closed(e.id);
                                metrics.set_idle(g.idle.len());
                                let c = e.conn.clone();
                                tokio::spawn(async move {
                                    let _ = c.close().await;
                                });
                            }
                        } else {
                            break;
                        }
                    }
                }
            });
            *self.evict_handle.lock().unwrap() = Some(handle);
        }

        // KeepAlive
        if self.config.keep_alive {
            let inner = self.inner.clone();
            let driver = self.driver.clone();
            let interval = self.config.keep_alive_interval();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let conns: Vec<Arc<D::Connection>> = {
                        let g = inner.lock().unwrap();
                        g.idle.iter().map(|e| e.conn.clone()).collect()
                    };
                    for conn in &conns {
                        let _ = driver.validate(conn).await;
                    }
                }
            });
            *self.keepalive_handle.lock().unwrap() = Some(handle);
        }

        tracing::info!(
            "DruidDataSource init: max={}, init={}",
            self.config.max_active,
            self.config.initial_size
        );
        Ok(())
    }

    pub async fn get_connection(&self) -> Result<PoolGuard<D::Connection>, DruidError> {
        let start = Instant::now();

        let permit = if let Some(max_wait) = self.config.max_wait() {
            match tokio::time::timeout(max_wait, self.semaphore.clone().acquire_owned()).await {
                Ok(Ok(p)) => p,
                Ok(Err(_)) => return Err(DruidError::Pool("semaphore closed".into())),
                Err(_) => return Err(DruidError::Pool("connection wait timeout".into())),
            }
        } else {
            self.semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| DruidError::Pool("semaphore closed".into()))?
        };

        let wait_ms = start.elapsed().as_millis() as u64;

        let (conn_id, conn, from_idle): (u64, Arc<D::Connection>, bool) = {
            let entry = {
                let mut g = self.inner.lock().unwrap();
                g.active_count += 1;
                let e = g.idle.pop_front();
                self.metrics.set_active(g.active_count);
                self.metrics.set_idle(g.idle.len());
                e
            };
            if let Some(e) = entry {
                (e.id, e.conn, true)
            } else {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                let c = self
                    .driver
                    .connect(
                        &self.config.url,
                        &self.config.username,
                        &self.config.password,
                    )
                    .await
                    .map(Arc::new)?;
                self.filter_chain.connection_created(id);
                (id, c, false)
            }
        };

        self.metrics.inc_borrow();
        self.metrics
            .add_wait_time_ns(start.elapsed().as_nanos() as u64);
        self.filter_chain.connection_borrowed(conn_id, wait_ms);

        if self.config.test_on_borrow {
            if let Err(e) = self.driver.validate(&conn).await {
                // rollback: undo state mutations that were done before validation
                let mut g = self.inner.lock().unwrap();
                g.active_count = g.active_count.saturating_sub(1);
                self.metrics.set_active(g.active_count);
                if from_idle {
                    g.idle.push_back(PoolEntry {
                        conn,
                        last_used_at: Instant::now(),
                        id: conn_id,
                    });
                    self.metrics.set_idle(g.idle.len());
                } else {
                    self.filter_chain.connection_closed(conn_id);
                }
                return Err(e);
            }
        }

        Ok(PoolGuard {
            conn,
            conn_id,
            permit: Some(permit),
            inner: self.inner.clone(),
            filter_chain: self.filter_chain.clone(),
            metrics: self.metrics.clone(),
        })
    }

    async fn create_entry(&self) -> Result<PoolEntry<D::Connection>, DruidError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let conn = self
            .driver
            .connect(
                &self.config.url,
                &self.config.username,
                &self.config.password,
            )
            .await
            .map(Arc::new)?;
        self.metrics.inc_create();
        self.filter_chain.connection_created(id);
        Ok(PoolEntry {
            conn,
            last_used_at: Instant::now(),
            id,
        })
    }

    // ── 状态查询 ──

    pub fn active_count(&self) -> usize {
        self.inner.lock().unwrap().active_count
    }
    pub fn idle_count(&self) -> usize {
        self.inner.lock().unwrap().idle.len()
    }
    pub fn max_active(&self) -> usize {
        self.config.max_active
    }
    pub fn filter_chain(&self) -> &FilterChain {
        &self.filter_chain
    }
    pub fn metrics(&self) -> &PoolMetrics {
        &self.metrics
    }
    pub fn pscache(&self) -> &Mutex<PSCache> {
        &self.pscache
    }

    pub async fn close(&self) -> Result<(), DruidError> {
        if let Some(h) = self.evict_handle.lock().unwrap().take() {
            h.abort();
        }
        if let Some(h) = self.keepalive_handle.lock().unwrap().take() {
            h.abort();
        }
        let conns: Vec<PoolEntry<D::Connection>> = {
            let mut g = self.inner.lock().unwrap();
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
}

/// 池连接 Guard — Drop 时自动归还
pub struct PoolGuard<C: Connection> {
    conn: Arc<C>,
    conn_id: u64,
    #[allow(dead_code)]
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    inner: Arc<Mutex<PoolInner<C>>>,
    filter_chain: Arc<FilterChain>,
    metrics: Arc<PoolMetrics>,
}

impl<C: Connection> PoolGuard<C> {
    pub fn connection(&self) -> &Arc<C> {
        &self.conn
    }
    pub fn connection_id(&self) -> u64 {
        self.conn_id
    }
}

impl<C: Connection> Drop for PoolGuard<C> {
    fn drop(&mut self) {
        let mut g = self.inner.lock().unwrap();
        g.active_count = g.active_count.saturating_sub(1);
        self.metrics.set_active(g.active_count);
        if !g.closed {
            self.filter_chain.connection_returned(self.conn_id);
            g.idle.push_back(PoolEntry {
                conn: self.conn.clone(),
                last_used_at: Instant::now(),
                id: self.conn_id,
            });
            self.metrics.set_idle(g.idle.len());
        } else {
            self.filter_chain.connection_closed(self.conn_id);
            self.metrics.set_idle(g.idle.len());
            let c = self.conn.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = c.close().await;
                });
            } else {
                tracing::warn!(
                    "PoolGuard dropped outside tokio runtime, connection {} may leak",
                    self.conn_id
                );
            }
        }
        // permit auto-released
    }
}
