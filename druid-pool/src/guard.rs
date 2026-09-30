//! 池连接 Guard：Drop 时自动归还或关闭，并提供带 Filter 钩子的执行入口

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use druid_core::DruidError;
use druid_filter::FilterChain;
use druid_stat::metrics::PoolMetrics;
use tokio::sync::OwnedSemaphorePermit;

use crate::background::{spawn_close, validate_with_timeout};
use crate::datasource::{PoolEntry, PoolInner};
use crate::driver::{Connection, Driver};

/// 池连接 Guard — Drop 时自动归还
///
/// 钩子顺序：`connection_borrow_before` →（可选借用校验）→ `connection_borrowed`
/// →（业务使用）→ `connection_return_before` →（可选归还校验）→ `connection_returned`
/// 或 `connection_closed`（连接被销毁时）。
///
/// Guard 在物理连接产生**之前**就已构造（见 `get_connection`），因此中途取消
/// 也会由 Drop 兜底：计数归位，在途连接物理关闭，不会泄漏。
pub struct PoolGuard<D: Driver> {
    /// 持有的物理连接；`None` 表示连接仍在创建中（`get_connection` 的 await 窗口）
    pub(crate) conn: Option<Arc<D::Connection>>,
    pub(crate) conn_id: u64,
    /// 物理连接创建时刻，随连接在池内流转，用于 max_lifetime 判定
    pub(crate) created_at: Instant,
    /// 并发许可；异步归还时移入后台任务，连接归位/关闭后才释放
    pub(crate) permit: Option<OwnedSemaphorePermit>,
    /// 是否已计入 active_count（摘取连接后置 true）
    pub(crate) counted: bool,
    /// 是否仍计入 waiting 指标（成功交付后置 false）
    pub(crate) waiting: bool,
    /// 是否可归还池中；仅在成功交付前保持 false，中途取消/校验失败时 Drop 直接关闭连接
    pub(crate) healthy: bool,
    /// 语句序号：与 conn_id 拼成全局唯一 statement id
    pub(crate) stmt_seq: AtomicU64,
    pub(crate) inner: Arc<Mutex<PoolInner<D::Connection>>>,
    pub(crate) filter_chain: Arc<FilterChain>,
    pub(crate) metrics: Arc<PoolMetrics>,
    pub(crate) driver: Arc<D>,
    pub(crate) test_on_return: bool,
    pub(crate) validate_timeout_secs: u64,
}

impl<D: Driver> PoolGuard<D> {
    /// 构造「在途」Guard：物理连接尚未产生，由 `get_connection` 逐步填充
    pub(crate) fn pending(
        inner: Arc<Mutex<PoolInner<D::Connection>>>,
        filter_chain: Arc<FilterChain>,
        metrics: Arc<PoolMetrics>,
        driver: Arc<D>,
        test_on_return: bool,
        validate_timeout_secs: u64,
    ) -> Self {
        PoolGuard {
            conn: None,
            conn_id: 0,
            created_at: Instant::now(),
            permit: None,
            counted: false,
            waiting: true,
            healthy: false,
            stmt_seq: AtomicU64::new(1),
            inner,
            filter_chain,
            metrics,
            driver,
            test_on_return,
            validate_timeout_secs,
        }
    }

    /// 逃生通道：直接访问裸连接 —— **绕过 Filter 链**（SQL 防火墙/统计不会生效），
    /// 仅在确实不需要 Filter 语义时使用。
    pub fn connection(&self) -> &Arc<D::Connection> {
        self.conn
            .as_ref()
            .expect("PoolGuard 尚未持有连接：该状态只存在于 get_connection 在途期间")
    }

    /// 连接 ID
    pub fn connection_id(&self) -> u64 {
        self.conn_id
    }

    /// 执行 SQL（返回受影响行数）
    ///
    /// 顺序：`statement_created` → `statement_execute_before`（拦截则 `statement_error`）
    /// → 驱动执行 → 成功 `statement_execute_after` / 失败 `statement_error` → `statement_closed`。
    pub async fn execute(&self, sql: &str) -> Result<u64, DruidError> {
        let stmt_id = self.next_stmt_id();
        self.filter_chain.statement_created(sql, stmt_id);
        // 兜底闭合：正常返回、出错、或在 await 中被取消，statement_closed 都恰好触发一次
        let _scope = StmtScope {
            chain: &self.filter_chain,
            sql,
            stmt_id,
        };

        let result = match self.filter_chain.statement_execute_before(sql, stmt_id) {
            Ok(()) => {
                let start = Instant::now();
                match self.connection().execute(sql).await {
                    Ok(rows) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        self.filter_chain
                            .statement_execute_after(sql, stmt_id, elapsed, rows);
                        Ok(rows)
                    }
                    Err(e) => {
                        self.filter_chain.statement_error(sql, stmt_id, &e);
                        Err(e)
                    }
                }
            }
            Err(e) => {
                // 被 Filter 拦截（如 SQL 防火墙）：同样走错误钩子
                self.filter_chain.statement_error(sql, stmt_id, &e);
                Err(e)
            }
        };

        result
    }

    /// 查询 SQL（返回行数据）
    ///
    /// 与 [`PoolGuard::execute`] 相同，成功时额外触发 `resultset_open` / `resultset_closed`。
    pub async fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        let stmt_id = self.next_stmt_id();
        self.filter_chain.statement_created(sql, stmt_id);
        // 与 execute 同样的兜底闭合（含 resultset 钩子在前的顺序保证）。
        // ⚠️ StmtScope 是**唯一**的闭合路径：成功 / 拦截 / 取消三条路径都恰好闭合一次，
        // 不要再在函数末尾补显式 statement_closed（那会让成功路径闭合两次）。
        let _scope = StmtScope {
            chain: &self.filter_chain,
            sql,
            stmt_id,
        };

        match self.filter_chain.statement_execute_before(sql, stmt_id) {
            Ok(()) => {
                let start = Instant::now();
                match self.connection().query(sql).await {
                    Ok(rows) => {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let rows_read = rows.len() as u64;
                        self.filter_chain.resultset_open(sql);
                        self.filter_chain.resultset_closed(sql, rows_read);
                        self.filter_chain
                            .statement_execute_after(sql, stmt_id, elapsed, rows_read);
                        Ok(rows)
                    }
                    Err(e) => {
                        self.filter_chain.statement_error(sql, stmt_id, &e);
                        Err(e)
                    }
                }
            }
            Err(e) => {
                self.filter_chain.statement_error(sql, stmt_id, &e);
                Err(e)
            }
        }
    }

    /// 生成全局唯一 statement id：高位 conn_id、低位连接内自增序号
    fn next_stmt_id(&self) -> u64 {
        (self.conn_id << 32) | self.stmt_seq.fetch_add(1, Ordering::SeqCst)
    }
}

/// statement 生命周期兜底：`statement_created` 之后创建，Drop 时触发 `statement_closed`。
///
/// 放在 Drop 里而不是函数末尾显式调用，是为了让 execute/query 的 future 在中途被
/// 取消（timeout / select! 落选 / abort）时同样闭合，依赖 created/closed 配对的
/// 统计类 Filter 不会因此漂移。成功路径下 Drop 发生在返回之前，钩子顺序不变。
struct StmtScope<'a> {
    chain: &'a FilterChain,
    sql: &'a str,
    stmt_id: u64,
}

impl Drop for StmtScope<'_> {
    fn drop(&mut self) {
        self.chain.statement_closed(self.sql, self.stmt_id);
    }
}

impl<D: Driver> Drop for PoolGuard<D> {
    fn drop(&mut self) {
        let conn = self.conn.take();

        if self.waiting {
            self.metrics.dec_waiting();
        }

        // 连接尚在创建中（connect 被取消或失败）：无物理连接，仅计数归位即可
        let Some(conn) = conn else {
            settle_active(&self.inner, &self.metrics, self.counted);
            return;
        };

        let pool_closed = self.inner.lock().unwrap_or_else(|e| e.into_inner()).closed;

        // 在途取消 / 借出前校验失败 / 池已关闭：物理关闭，绝不入池
        if !self.healthy || pool_closed {
            settle_active(&self.inner, &self.metrics, self.counted);
            self.metrics.inc_destroy();
            self.filter_chain.connection_closed(self.conn_id);
            spawn_close(conn);
            return;
        }

        self.filter_chain.connection_return_before(self.conn_id);

        if self.test_on_return {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let permit = self.permit.take(); // 连接归位/关闭后才释放并发许可
                let (driver, inner, fc, m) = (
                    self.driver.clone(),
                    self.inner.clone(),
                    self.filter_chain.clone(),
                    self.metrics.clone(),
                );
                let (conn_id, created_at, timeout, counted) = (
                    self.conn_id,
                    self.created_at,
                    self.validate_timeout_secs,
                    self.counted,
                );
                handle.spawn(async move {
                    // 校验窗口内连接**仍计入 active**：连接只是"在途"，尚未真正归位，
                    // 这样 create - destroy == idle + active 在归还全程成立（close() 也看得见它）
                    if validate_with_timeout(&*driver, &conn, timeout)
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            "test_on_return validation failed for connection {}",
                            conn_id
                        );
                        settle_active(&inner, &m, counted);
                        fc.connection_closed(conn_id);
                        m.inc_destroy();
                        let _ = conn.close().await;
                    } else if let Err(c) =
                        return_to_idle(&inner, &m, conn, conn_id, created_at, counted)
                    {
                        // 校验期间池被关闭：连接不能进池，直接物理关闭
                        fc.connection_closed(conn_id);
                        m.inc_destroy();
                        let _ = c.close().await;
                    } else {
                        fc.connection_returned(conn_id);
                    }
                    drop(permit);
                });
                return;
            }
            tracing::warn!("无 tokio 运行时，跳过归还校验，连接同步归还");
        }

        match return_to_idle(
            &self.inner,
            &self.metrics,
            conn,
            self.conn_id,
            self.created_at,
            self.counted,
        ) {
            Ok(()) => self.filter_chain.connection_returned(self.conn_id),
            Err(c) => {
                // 归还瞬间池已关闭
                self.filter_chain.connection_closed(self.conn_id);
                self.metrics.inc_destroy();
                spawn_close(c);
            }
        }
    }
}

/// active 计数归位：连接真正回池或物理关闭后调用
/// （异步归还校验、KeepAlive 校验等「连接已离开 idle 但未交付」的窗口内保持 active）
pub(crate) fn settle_active<C: Connection>(
    inner: &Mutex<PoolInner<C>>,
    metrics: &PoolMetrics,
    counted: bool,
) {
    let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
    if counted {
        g.active_count = g.active_count.saturating_sub(1);
    }
    metrics.set_active(g.active_count);
}

/// 尝试把连接放回空闲队列，并在同一个锁内归位 active 计数；
/// 池已关闭时把连接原样返回，由调用方关闭。
///
/// 归还路径共用：`PoolGuard::drop`、归还校验后台任务、KeepAlive 校验循环。
pub(crate) fn return_to_idle<C: Connection>(
    inner: &Mutex<PoolInner<C>>,
    metrics: &PoolMetrics,
    conn: Arc<C>,
    id: u64,
    created_at: Instant,
    counted: bool,
) -> Result<(), Arc<C>> {
    let mut g = inner.lock().unwrap_or_else(|e| e.into_inner());
    if counted {
        g.active_count = g.active_count.saturating_sub(1);
    }
    if g.closed {
        metrics.set_active(g.active_count);
        return Err(conn);
    }
    g.idle.push_back(PoolEntry {
        conn,
        last_used_at: Instant::now(),
        created_at,
        id,
    });
    // 先 idle 后 active：观测者读到新 idle 时，active 必已归位
    metrics.set_idle(g.idle.len());
    metrics.set_active(g.active_count);
    Ok(())
}
