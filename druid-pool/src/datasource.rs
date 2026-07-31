use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use druid_core::{DruidConfig, DruidError};
use druid_filter::FilterChain;
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
    fn new() -> Self { PoolInner { idle: VecDeque::new(), active_count: 0, closed: false } }
}

pub struct DruidDataSource<D: Driver> {
    driver: Arc<D>,
    config: DruidConfig,
    semaphore: Arc<Semaphore>,
    inner: Arc<Mutex<PoolInner<D::Connection>>>,
    filter_chain: Arc<FilterChain>,
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
        DruidDataSource {
            driver, config,
            semaphore: Arc::new(Semaphore::new(max)),
            inner: Arc::new(Mutex::new(PoolInner::new())),
            filter_chain: fc,
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

        // 驱逐线程
        if self.config.time_between_eviction_runs_ms > 0 {
            let inner = self.inner.clone();
            let fchain = self.filter_chain.clone();
            let max_ms = self.config.max_evictable_idle_time_ms;
            let min_idle = self.config.min_idle;
            let interval = self.config.eviction_interval();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let now = Instant::now();
                    let mut g = inner.lock().unwrap();
                    while g.idle.len() > min_idle {
                        let should = g.idle.front().map_or(false, |e| {
                            now.duration_since(e.last_used_at).as_millis() > max_ms as u128
                        });
                        if should {
                            if let Some(e) = g.idle.pop_front() {
                                fchain.connection_closed(e.id);
                                let c = e.conn.clone();
                                tokio::spawn(async move { let _ = c.close().await; });
                            }
                        } else { break; }
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

        tracing::info!("DruidDataSource init: max={}, init={}", self.config.max_active, self.config.initial_size);
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
            self.semaphore.clone().acquire_owned().await
                .map_err(|_| DruidError::Pool("semaphore closed".into()))?
        };

        let wait_ms = start.elapsed().as_millis() as u64;

        let (conn_id, conn): (u64, Arc<D::Connection>) = {
            let mut g = self.inner.lock().unwrap();
            if let Some(e) = g.idle.pop_front() {
                let id = e.id;
                let c = e.conn;
                g.active_count += 1;
                (id, c)
            } else {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                g.active_count += 1;
                drop(g);
                let c = self.driver.connect(
                    &self.config.url, &self.config.username, &self.config.password,
                ).await.map(Arc::new)?;
                self.filter_chain.connection_created(id);
                (id, c)
            }
        };

        self.filter_chain.connection_borrowed(conn_id, wait_ms);

        if self.config.test_on_borrow {
            self.driver.validate(&conn).await?;
        }

        Ok(PoolGuard {
            conn, conn_id,
            permit: Some(permit),
            inner: self.inner.clone(),
            filter_chain: self.filter_chain.clone(),
        })
    }

    async fn create_entry(&self) -> Result<PoolEntry<D::Connection>, DruidError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let conn = self.driver.connect(
            &self.config.url, &self.config.username, &self.config.password,
        ).await.map(Arc::new)?;
        self.filter_chain.connection_created(id);
        Ok(PoolEntry { conn, last_used_at: Instant::now(), id })
    }

    // ── 状态查询 ──

    pub fn active_count(&self) -> usize { self.inner.lock().unwrap().active_count }
    pub fn idle_count(&self) -> usize { self.inner.lock().unwrap().idle.len() }
    pub fn max_active(&self) -> usize { self.config.max_active }
    pub fn filter_chain(&self) -> &FilterChain { &self.filter_chain }

    pub async fn close(&self) -> Result<(), DruidError> {
        if let Some(h) = self.evict_handle.lock().unwrap().take() { h.abort(); }
        if let Some(h) = self.keepalive_handle.lock().unwrap().take() { h.abort(); }
        let mut g = self.inner.lock().unwrap();
        g.closed = true;
        while let Some(e) = g.idle.pop_front() {
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
}

impl<C: Connection> PoolGuard<C> {
    pub fn connection(&self) -> &Arc<C> { &self.conn }
    pub fn connection_id(&self) -> u64 { self.conn_id }
}

impl<C: Connection> Drop for PoolGuard<C> {
    fn drop(&mut self) {
        let mut g = self.inner.lock().unwrap();
        g.active_count = g.active_count.saturating_sub(1);
        if !g.closed {
            self.filter_chain.connection_returned(self.conn_id);
            g.idle.push_back(PoolEntry {
                conn: self.conn.clone(),
                last_used_at: Instant::now(),
                id: self.conn_id,
            });
        } else {
            self.filter_chain.connection_closed(self.conn_id);
            let c = self.conn.clone();
            tokio::spawn(async move { let _ = c.close().await; });
        }
        // permit auto-released
    }
}
