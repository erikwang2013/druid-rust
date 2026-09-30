//! 后台维护循环：空闲连接驱逐、KeepAlive 校验
//!
//! 两个循环都只持有 Arc 克隆，由 [`DruidDataSource`](crate::DruidDataSource)
//! 保存 JoinHandle，close()/Drop 时 abort。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use druid_core::{DruidConfig, DruidError};
use druid_filter::FilterChain;
use druid_stat::metrics::PoolMetrics;
use tokio::sync::Semaphore;

use crate::datasource::{PoolEntry, PoolInner};
use crate::driver::{Connection, Driver};
use crate::guard::{return_to_idle, settle_active};

/// 带超时的连接校验：`validation_query_timeout_secs > 0` 时超时视为校验失败
pub(crate) async fn validate_with_timeout<D: Driver>(
    driver: &D,
    conn: &D::Connection,
    timeout_secs: u64,
) -> Result<(), DruidError> {
    if timeout_secs == 0 {
        return driver.validate(conn).await;
    }
    match tokio::time::timeout(Duration::from_secs(timeout_secs), driver.validate(conn)).await {
        Ok(r) => r,
        Err(_) => Err(DruidError::Pool("validation query timeout".into())),
    }
}

/// 后台关闭一条物理连接（Drop 中不能 await）
pub(crate) fn spawn_close<C: Connection>(conn: Arc<C>) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                let _ = conn.close().await;
            });
        }
        Err(_) => tracing::warn!("无 tokio 运行时，连接未能显式关闭（依赖驱动自身的 Drop）"),
    }
}

/// 启动空闲连接驱逐循环：按空闲时长与绝对寿命回收空闲连接
pub(crate) fn spawn_eviction_loop<D: Driver>(
    inner: Arc<Mutex<PoolInner<D::Connection>>>,
    fchain: Arc<FilterChain>,
    metrics: Arc<PoolMetrics>,
    config: DruidConfig,
) -> tokio::task::JoinHandle<()> {
    let max_idle_ms = config.max_evictable_idle_time_ms;
    let max_lifetime_ms = config.max_lifetime_ms;
    let min_idle = config.min_idle;
    let interval = config.eviction_interval();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            let now = Instant::now();
            let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
            let current_idle = g.idle.len();
            let mut evicted = 0usize;
            let mut to_evict: Vec<PoolEntry<D::Connection>> = Vec::new();
            g.idle.retain(|e| {
                let idle_ms = now.duration_since(e.last_used_at).as_millis() as u64;
                let alive_ms = now.duration_since(e.created_at).as_millis() as u64;
                let above_min_idle = (current_idle - evicted) > min_idle;
                let over_max_idle = above_min_idle && idle_ms > max_idle_ms;
                // 绝对寿命按 created_at 判定：热点连接不会被归还刷新而永生
                let over_lifetime =
                    max_lifetime_ms > 0 && above_min_idle && alive_ms > max_lifetime_ms;
                if over_max_idle || over_lifetime {
                    evicted += 1;
                    to_evict.push(PoolEntry {
                        conn: e.conn.clone(),
                        last_used_at: e.last_used_at,
                        created_at: e.created_at,
                        id: e.id,
                    });
                    false
                } else {
                    true
                }
            });
            metrics.set_idle(g.idle.len());
            drop(g); // Filter 回调不持池锁
            for e in &to_evict {
                metrics.inc_destroy();
                fchain.connection_closed(e.id);
            }
            for e in to_evict {
                spawn_close(e.conn);
            }
        }
    })
}

/// 启动 KeepAlive 循环：校验空闲连接，无效则驱逐
///
/// 与借用路径**同一套状态语义**：先取并发许可，再在同一把锁内「计入 active + 摘出 idle」，
/// 校验结束后归还（`return_to_idle`）或物理销毁。因此：
///   - 校验在途的连接借用方拿不到 —— 借用方会另建新连接，或等在这一个许可上；
///   - 驱逐与 close() 都看得见它（计入 active），`create - destroy == idle + active` 全程成立；
///   - 池内物理连接数不会超过 max_active（连接只在持有许可时离开 idle）。
///
/// ⚠️ 两个**有意为之**的语义，勿当 bug 修：
///   - 校验窗口内该连接不再计入 idle，`min_idle` 可能暂时不满足，校验完成即恢复
///     （「临时少留一条」而非泄漏）；
///   - 校验期间占用一个并发许可，驱动校验挂起时会临时占用一个 max_active 名额
///     （与 test_on_return 的残余风险同类）。
pub(crate) fn spawn_keepalive_loop<D: Driver>(
    inner: Arc<Mutex<PoolInner<D::Connection>>>,
    driver: Arc<D>,
    metrics: Arc<PoolMetrics>,
    fchain: Arc<FilterChain>,
    semaphore: Arc<Semaphore>,
    config: DruidConfig,
) -> tokio::task::JoinHandle<()> {
    let interval = config.keep_alive_interval();
    let timeout_secs = config.validation_query_timeout_secs;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            // 本轮校验条数取「开始时」的 idle 长度：校验期间新归还的连接留到下一轮，
            // 否则刚要归还的连接会被同一轮立即再摘出来空转一遍
            let mut budget = {
                let g = inner.lock().unwrap_or_else(|e| e.into_inner());
                g.idle.len()
            };
            while budget > 0 {
                budget -= 1;
                // 借用路径同款闸门：拿不到许可说明连接都在使用中（此时 idle 必为空），本轮结束
                let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                    break;
                };
                // 与 get_connection 一致：同一把锁内先计入 active，再把连接摘出 idle
                let entry = {
                    let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
                    let e = if g.closed { None } else { g.idle.pop_front() };
                    if e.is_some() {
                        g.active_count += 1;
                        metrics.set_active(g.active_count);
                        metrics.set_idle(g.idle.len());
                    }
                    e
                };
                let Some(entry) = entry else {
                    break; // 池已关闭，或本轮已被借空
                };

                // 校验在锁外进行（可能长耗时）；Validating 兜底 close()/Drop 中断的路径
                let mut v = Validating {
                    inner: inner.clone(),
                    metrics: metrics.clone(),
                    fchain: fchain.clone(),
                    conn: Some(entry.conn),
                    id: entry.id,
                    settled: false,
                };
                let ok = {
                    let conn = v.conn.as_ref().expect("刚构造的 Validating 必持有连接");
                    validate_with_timeout(&*driver, conn, timeout_secs)
                        .await
                        .is_ok()
                };
                if ok {
                    if let Err(c) =
                        return_to_idle(&inner, &metrics, v.take(), entry.id, entry.created_at, true)
                    {
                        // 校验期间池被关闭：连接不入池，直接物理销毁
                        fchain.connection_closed(entry.id);
                        metrics.inc_destroy();
                        let _ = c.close().await;
                    }
                } else {
                    tracing::warn!(
                        "KeepAlive validation failed for conn {}, evicting",
                        entry.id
                    );
                    settle_active(&inner, &metrics, true);
                    fchain.connection_closed(entry.id);
                    metrics.inc_destroy();
                    let _ = v.take().close().await;
                }
                drop(permit);
            }
        }
    })
}

/// 正在校验的空闲连接：计数已计入 active、连接已摘出 idle（借用方拿不到）。
///
/// 正常路径由 `take()` 显式结算；若后台循环在校验在途被 close()/Drop abort，
/// 则由 Drop 兜底 —— 计数归位 + 通告 + 销毁计数 + 物理关闭，顺序与
/// `PoolGuard::drop` 的取消路径一致，不会留下「active 永久虚高 + 连接未关闭」的残局。
struct Validating<C: Connection> {
    inner: Arc<Mutex<PoolInner<C>>>,
    metrics: Arc<PoolMetrics>,
    fchain: Arc<FilterChain>,
    conn: Option<Arc<C>>,
    id: u64,
    settled: bool,
}

impl<C: Connection> Validating<C> {
    /// 取走连接并标记已结算 —— 调用方负责 active 归位与物理关闭。
    /// 取走与结算之间不得有 await（否则中断点会落在两者中间）
    fn take(&mut self) -> Arc<C> {
        self.settled = true;
        self.conn.take().expect("校验中的连接只会被取走一次")
    }
}

impl<C: Connection> Drop for Validating<C> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let Some(conn) = self.conn.take() else {
            return;
        };
        settle_active(&self.inner, &self.metrics, true);
        self.fchain.connection_closed(self.id);
        self.metrics.inc_destroy();
        spawn_close(conn);
    }
}
