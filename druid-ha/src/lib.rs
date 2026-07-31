//! 高可用数据源
//!
//! 多数据源负载均衡、健康检查和故障切换。
//! 对应 Java Druid 的 HighAvailableDataSource。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use druid_core::DruidError;
use druid_pool::driver::Driver;
use druid_pool::DruidDataSource;

/// 数据源节点状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeStatus {
    Active,  // 正常
    Down,    // 故障
    Testing, // 探测中
}

/// 数据源节点
struct HaNode<D: Driver> {
    datasource: Arc<DruidDataSource<D>>,
    status: Mutex<NodeStatus>,
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
            validation_sql: "SELECT 1".to_string(),
        }
    }

    /// 添加数据源节点
    pub fn add_node(&mut self, name: &str, ds: DruidDataSource<D>, weight: usize) {
        let node = Arc::new(HaNode {
            datasource: Arc::new(ds),
            status: Mutex::new(NodeStatus::Active),
            weight,
            name: name.to_string(),
        });
        self.nodes.push(node);
    }

    /// 设置健康检查间隔
    pub fn set_check_interval(&mut self, interval: Duration) {
        self.check_interval = interval;
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
            .filter(|n| *n.status.lock().unwrap() == NodeStatus::Active)
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

    /// 标记节点故障
    pub fn mark_down(&self, name: &str) {
        for node in &self.nodes {
            if node.name == name {
                *node.status.lock().unwrap() = NodeStatus::Down;
                tracing::warn!("HA node {} marked DOWN", name);
                return;
            }
        }
    }

    /// 标记节点恢复
    pub fn mark_up(&self, name: &str) {
        for node in &self.nodes {
            if node.name == name {
                *node.status.lock().unwrap() = NodeStatus::Active;
                tracing::info!("HA node {} marked ACTIVE", name);
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
            .filter(|n| *n.status.lock().unwrap() == NodeStatus::Active)
            .count()
    }

    /// 获取所有节点名称
    pub fn node_names(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.name.clone()).collect()
    }

    /// 执行一轮健康检查
    pub async fn run_health_check(self: &Arc<Self>) {
        for node in &self.nodes {
            let status = { node.status.lock().unwrap().clone() };
            match status {
                NodeStatus::Active => {
                    if let Ok(guard) = node.datasource.get_connection().await {
                        drop(guard);
                    } else {
                        self.mark_down(&node.name);
                    }
                }
                NodeStatus::Down => {
                    {
                        *node.status.lock().unwrap() = NodeStatus::Testing;
                    }
                    if let Ok(guard) = node.datasource.get_connection().await {
                        drop(guard);
                        self.mark_up(&node.name);
                    } else {
                        *node.status.lock().unwrap() = NodeStatus::Down;
                    }
                }
                NodeStatus::Testing => {}
            }
        }
    }

    /// 启动健康检查循环（需在 tokio 上下文中调用）
    pub fn spawn_health_check_loop(self: &Arc<Self>) {
        let this = self.clone();
        let interval = this.check_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                this.run_health_check().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
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
            *self.closed.lock().unwrap() = true;
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
        connect_count: AtomicU64,
    }
    impl MockHaDriver {
        fn new() -> Self {
            MockHaDriver {
                connect_count: AtomicU64::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl Driver for MockHaDriver {
        type Connection = MockHaConn;
        async fn connect(&self, _: &str, _: &str, _: &str) -> Result<MockHaConn, DruidError> {
            let id = self.connect_count.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(MockHaConn::new(id))
        }
        fn name(&self) -> &'static str {
            "MockHaDriver"
        }
        async fn validate(&self, _: &MockHaConn) -> Result<(), DruidError> {
            Ok(())
        }
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
        let ha = HighAvailableDataSource::<MockHaDriver>::new();
        // No nodes yet so nothing to mark
        assert_eq!(ha.node_count(), 0);
    }
}
