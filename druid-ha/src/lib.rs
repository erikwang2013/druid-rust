//! 高可用数据源
//!
//! 多数据源负载均衡、健康检查和故障切换。
//! 对应 Java Druid 的 HighAvailableDataSource。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use druid_core::DruidError;
use druid_pool::driver::Driver;
use druid_pool::DruidDataSource;

/// 数据源节点状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeStatus {
    Active, // 正常
    Down,   // 故障
}

/// 单个节点的探测状态：连续失败/成功计数与退避截止时间
#[derive(Debug)]
struct ProbeState {
    consecutive_failures: u32,
    consecutive_successes: u32,
    /// Down 节点下次允许探测的时间；None 表示立即可探测
    retry_at: Option<Instant>,
}

impl ProbeState {
    fn new() -> Self {
        ProbeState {
            consecutive_failures: 0,
            consecutive_successes: 0,
            retry_at: None,
        }
    }
}

/// 数据源节点
struct HaNode<D: Driver> {
    datasource: Arc<DruidDataSource<D>>,
    status: Mutex<NodeStatus>,
    probe: Mutex<ProbeState>,
    weight: usize,
    name: String,
}

/// 高可用数据源
pub struct HighAvailableDataSource<D: Driver> {
    nodes: Vec<Arc<HaNode<D>>>,
    /// 轮询计数器
    round_robin: AtomicUsize,
    /// 健康检查间隔
    check_interval: Duration,
    /// 探测超时；None 时取 check_interval 的 1/4
    probe_timeout: Option<Duration>,
    /// 连续失败 N 次才标记 Down
    failure_threshold: u32,
    /// 恢复需连续成功 M 次
    success_threshold: u32,
    /// 故障节点指数退避的上限
    max_backoff: Duration,
    /// 健康检查循环的退出通知
    shutdown_signal: tokio::sync::Notify,
    /// 健康检查 SQL
    validation_sql: String,
}

impl<D: Driver> Default for HighAvailableDataSource<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Driver> HighAvailableDataSource<D> {
    pub fn new() -> Self {
        HighAvailableDataSource {
            nodes: Vec::new(),
            round_robin: AtomicUsize::new(0),
            check_interval: Duration::from_secs(30),
            probe_timeout: None,
            failure_threshold: 3,
            success_threshold: 2,
            max_backoff: Duration::from_secs(300),
            shutdown_signal: tokio::sync::Notify::new(),
            validation_sql: "SELECT 1".to_string(),
        }
    }

    /// 添加数据源节点
    pub fn add_node(&mut self, name: &str, ds: DruidDataSource<D>, weight: usize) {
        // weight 0 会导致 get_datasource 中 total_weight 为 0、取模除零 panic，统一按 1 处理
        let node = Arc::new(HaNode {
            datasource: Arc::new(ds),
            status: Mutex::new(NodeStatus::Active),
            probe: Mutex::new(ProbeState::new()),
            weight: weight.max(1),
            name: name.to_string(),
        });
        self.nodes.push(node);
    }

    /// 设置健康检查间隔
    pub fn set_check_interval(&mut self, interval: Duration) {
        self.check_interval = interval;
    }

    /// 设置探测超时；不设置时取 check_interval 的 1/4
    pub fn set_probe_timeout(&mut self, timeout: Duration) {
        self.probe_timeout = Some(timeout);
    }

    /// 连续失败达到 n 次才标记 Down（默认 3）
    pub fn set_failure_threshold(&mut self, n: u32) {
        self.failure_threshold = n.max(1);
    }

    /// 恢复需连续成功 n 次（默认 2）
    pub fn set_success_threshold(&mut self, n: u32) {
        self.success_threshold = n.max(1);
    }

    /// 设置故障节点指数退避的上限（默认 300s）
    pub fn set_max_backoff(&mut self, backoff: Duration) {
        self.max_backoff = backoff;
    }

    /// 设置验证 SQL
    pub fn set_validation_sql(&mut self, sql: &str) {
        self.validation_sql = sql.to_string();
    }

    /// 获取一个活跃数据源（加权轮询）
    pub async fn get_datasource(&self) -> Result<Arc<DruidDataSource<D>>, DruidError> {
        let active: Vec<&Arc<HaNode<D>>> = self
            .nodes
            .iter()
            .filter(|n| *n.status.lock().unwrap_or_else(|e| e.into_inner()) == NodeStatus::Active)
            .collect();

        if active.is_empty() {
            return Err(DruidError::Pool("no active datasource available".into()));
        }

        // 加权轮询
        let total_weight: usize = active.iter().map(|n| n.weight).sum();
        let idx = self.round_robin.fetch_add(1, Ordering::Relaxed) % total_weight;
        let mut cumulative = 0;
        for node in &active {
            cumulative += node.weight;
            if idx < cumulative {
                tracing::debug!("HA selected node: {}", node.name);
                return Ok(node.datasource.clone());
            }
        }

        Ok(active[0].datasource.clone())
    }

    /// 标记节点故障（重置探测计数）
    pub fn mark_down(&self, name: &str) {
        for node in &self.nodes {
            if node.name == name {
                self.set_status(node, NodeStatus::Down);
                self.reset_probe_state(node);
                return;
            }
        }
    }

    /// 标记节点恢复（重置探测计数）
    pub fn mark_up(&self, name: &str) {
        for node in &self.nodes {
            if node.name == name {
                self.set_status(node, NodeStatus::Active);
                self.reset_probe_state(node);
                return;
            }
        }
    }

    /// 节点总数
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 活跃节点数
    pub fn active_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| *n.status.lock().unwrap_or_else(|e| e.into_inner()) == NodeStatus::Active)
            .count()
    }

    /// 获取所有节点名称
    pub fn node_names(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.name.clone()).collect()
    }

    /// 设置节点状态并记录日志（不重置探测计数，供巡检内部使用）
    fn set_status(&self, node: &HaNode<D>, status: NodeStatus) {
        *node.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        match status {
            NodeStatus::Active => tracing::info!("HA node {} marked ACTIVE", node.name),
            NodeStatus::Down => tracing::warn!("HA node {} marked DOWN", node.name),
        }
    }

    /// 重置节点的探测计数与退避
    fn reset_probe_state(&self, node: &HaNode<D>) {
        *node.probe.lock().unwrap_or_else(|e| e.into_inner()) = ProbeState::new();
    }

    /// 实际生效的探测超时
    fn effective_probe_timeout(&self) -> Duration {
        self.probe_timeout.unwrap_or(self.check_interval / 4)
    }

    /// Down 节点的退避时长：check_interval * 2^(超出失败阈值的次数)，上限 max_backoff
    fn backoff(&self, consecutive_failures: u32) -> Duration {
        let exp = consecutive_failures
            .saturating_sub(self.failure_threshold)
            .min(16); // 防止移位溢出
        self.check_interval
            .saturating_mul(1u32 << exp)
            .min(self.max_backoff)
    }

    /// 执行一轮健康检查
    ///
    /// 取消安全：探测结束前不写入任何状态，因此调用方用 `timeout`/`select!`
    /// 打断本函数不会让节点卡在中间状态。
    pub async fn run_health_check(&self) {
        let timeout = self.effective_probe_timeout();
        for node in &self.nodes {
            // 快照状态与退避截止时间（不嵌套持锁，避免与 mark_down/mark_up 死锁）
            let status = node
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let retry_at = node
                .probe
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retry_at;

            // Down 节点按指数退避重试，避免每轮无条件重试打爆故障后端
            if status == NodeStatus::Down && retry_at.is_some_and(|t| Instant::now() < t) {
                tracing::debug!("HA node {} 处于退避窗口，本轮跳过探测", node.name);
                continue;
            }

            // 探测借用业务连接池，超时防止单个节点挂住整个巡检任务
            let ok = match tokio::time::timeout(timeout, node.datasource.get_connection()).await {
                Ok(Ok(guard)) => {
                    drop(guard);
                    true
                }
                Ok(Err(e)) => {
                    tracing::debug!("HA node {} 探测失败: {}", node.name, e);
                    false
                }
                Err(_) => {
                    tracing::warn!("HA node {} 探测超时（{:?}）", node.name, timeout);
                    false
                }
            };

            let mut ps = node.probe.lock().unwrap_or_else(|e| e.into_inner());
            if ok {
                ps.consecutive_failures = 0;
                ps.retry_at = None;
                if status == NodeStatus::Down {
                    // 恢复需连续成功 M 次，避免抖动节点立刻全量放回
                    ps.consecutive_successes += 1;
                    if ps.consecutive_successes >= self.success_threshold {
                        ps.consecutive_successes = 0;
                        drop(ps);
                        self.set_status(node, NodeStatus::Active);
                    }
                } else {
                    ps.consecutive_successes = 0;
                }
            } else {
                ps.consecutive_successes = 0;
                ps.consecutive_failures += 1;
                ps.retry_at = Some(Instant::now() + self.backoff(ps.consecutive_failures));
                // 连续失败达到阈值才摘除，网络抖动不再立刻踢节点
                let trip = status == NodeStatus::Active
                    && ps.consecutive_failures >= self.failure_threshold;
                drop(ps);
                if trip {
                    self.set_status(node, NodeStatus::Down);
                }
            }
        }
    }

    /// 启动健康检查循环（需在 tokio 上下文中调用）
    ///
    /// 返回 `JoinHandle`；调用 [`Self::shutdown`] 通知循环退出，循环彻底结束前
    /// 仍持有本对象的 Arc，需 await 该 handle 才能完成回收。
    pub fn spawn_health_check_loop(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        let interval = this.check_interval;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    // notify_one 会保留许可：巡检进行中发出的退出信号也不会丢失
                    _ = this.shutdown_signal.notified() => break,
                }
                this.run_health_check().await;
            }
        })
    }

    /// 通知健康检查循环退出（需 await spawn 返回的 JoinHandle 等待任务结束）
    pub fn shutdown(&self) {
        self.shutdown_signal.notify_one();
    }
}

#[cfg(test)]
mod tests;
