use serde::{Deserialize, Serialize};
use std::time::Duration;

/// DruidDataSource 连接池配置
#[derive(Clone, Serialize, Deserialize)]
pub struct DruidConfig {
    /// 数据库连接 URL
    pub url: String,
    /// 用户名
    pub username: String,
    /// 密码（序列化时不输出；反序列化时缺省为空串）
    #[serde(skip_serializing, default)]
    pub password: String,
    /// 数据库驱动类名
    pub driver_class_name: Option<String>,

    // 连接池参数
    /// 初始化连接数，默认 0
    #[serde(default)]
    pub initial_size: usize,
    /// 最小空闲连接数，默认 0
    #[serde(default)]
    pub min_idle: usize,
    /// 最大活跃连接数，默认 8
    #[serde(default = "DruidConfig::default_max_active")]
    pub max_active: usize,
    /// 获取连接最大等待时间(毫秒)，默认不限制
    #[serde(default)]
    pub max_wait_ms: u64,
    /// 空闲连接检测间隔(毫秒)，默认 60s
    #[serde(default = "DruidConfig::default_time_between_eviction_runs_ms")]
    pub time_between_eviction_runs_ms: u64,
    /// 连接最小生存时间(毫秒)，默认 30min
    #[serde(default = "DruidConfig::default_min_evictable")]
    pub min_evictable_idle_time_ms: u64,
    /// 连接空闲最大生存时间(毫秒)，默认 7h
    #[serde(default = "DruidConfig::default_max_evictable")]
    pub max_evictable_idle_time_ms: u64,
    /// 连接绝对最大生命周期(毫秒)，默认 0 表示不限制
    #[serde(default)]
    pub max_lifetime_ms: u64,

    // 验证参数
    /// 获取连接时是否验证
    #[serde(default = "DruidConfig::default_true")]
    pub test_on_borrow: bool,
    /// 归还连接时是否验证
    #[serde(default)]
    pub test_on_return: bool,
    /// 空闲检测时是否验证
    #[serde(default)]
    pub test_while_idle: bool,
    /// 验证查询 SQL
    pub validation_query: Option<String>,
    /// 验证查询超时(秒)
    #[serde(default)]
    pub validation_query_timeout_secs: u64,

    // PSCache（开启后通过 pscache 模块缓存 PreparedStatement 引用）
    /// 是否开启 PreparedStatement 缓存
    #[serde(default)]
    pub pool_prepared_statements: bool,
    /// PSCache 最大缓存数
    #[serde(default = "DruidConfig::default_max_pool_prepared_statement")]
    pub max_pool_prepared_statement_per_connection_size: usize,

    // KeepAlive
    /// 是否开启 KeepAlive
    #[serde(default)]
    pub keep_alive: bool,
    /// KeepAlive 间隔(毫秒)
    #[serde(default = "DruidConfig::default_keep_alive_between_time_ms")]
    pub keep_alive_between_time_ms: u64,

    // Filter 配置
    /// Filter 类名列表
    #[serde(default)]
    pub filters: Vec<String>,
    /// 连接属性
    #[serde(default)]
    pub connection_properties: Vec<String>,

    // 超时
    /// 连接超时(秒)
    #[serde(default = "DruidConfig::default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// Socket 超时(秒)
    #[serde(default = "DruidConfig::default_socket_timeout")]
    pub socket_timeout_secs: u64,
}

impl DruidConfig {
    fn default_max_active() -> usize {
        8
    }
    fn default_time_between_eviction_runs_ms() -> u64 {
        60_000
    }
    fn default_true() -> bool {
        true
    }
    fn default_max_pool_prepared_statement() -> usize {
        10
    }
    fn default_keep_alive_between_time_ms() -> u64 {
        120_000
    }
    fn default_connect_timeout() -> u64 {
        30
    }
    fn default_socket_timeout() -> u64 {
        30
    }
    fn default_min_evictable() -> u64 {
        1_800_000 // 30 minutes
    }
    fn default_max_evictable() -> u64 {
        25_200_000 // 7 hours
    }

    pub fn new(url: &str, username: &str, password: &str) -> Self {
        DruidConfig {
            url: url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            driver_class_name: None,
            initial_size: 0,
            min_idle: 0,
            max_active: Self::default_max_active(),
            max_wait_ms: 0,
            time_between_eviction_runs_ms: Self::default_time_between_eviction_runs_ms(),
            min_evictable_idle_time_ms: Self::default_min_evictable(),
            max_evictable_idle_time_ms: Self::default_max_evictable(),
            max_lifetime_ms: 0,
            test_on_borrow: true,
            test_on_return: false,
            test_while_idle: false,
            validation_query: None,
            validation_query_timeout_secs: 0,
            pool_prepared_statements: false,
            max_pool_prepared_statement_per_connection_size:
                Self::default_max_pool_prepared_statement(),
            keep_alive: false,
            keep_alive_between_time_ms: Self::default_keep_alive_between_time_ms(),
            filters: vec![],
            connection_properties: vec![],
            connect_timeout_secs: Self::default_connect_timeout(),
            socket_timeout_secs: Self::default_socket_timeout(),
        }
    }

    /// 获取连接最大等待时间
    pub fn max_wait(&self) -> Option<Duration> {
        if self.max_wait_ms > 0 {
            Some(Duration::from_millis(self.max_wait_ms))
        } else {
            None
        }
    }

    /// 空闲连接驱逐间隔
    pub fn eviction_interval(&self) -> Duration {
        Duration::from_millis(self.time_between_eviction_runs_ms)
    }

    /// KeepAlive 间隔
    pub fn keep_alive_interval(&self) -> Duration {
        Duration::from_millis(self.keep_alive_between_time_ms)
    }

    /// 连接超时
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs)
    }
}

impl Default for DruidConfig {
    fn default() -> Self {
        DruidConfig::new("", "", "")
    }
}

impl std::fmt::Debug for DruidConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DruidConfig")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"***")
            .field("driver_class_name", &self.driver_class_name)
            .field("initial_size", &self.initial_size)
            .field("min_idle", &self.min_idle)
            .field("max_active", &self.max_active)
            .field("max_wait_ms", &self.max_wait_ms)
            .field(
                "time_between_eviction_runs_ms",
                &self.time_between_eviction_runs_ms,
            )
            .field(
                "min_evictable_idle_time_ms",
                &self.min_evictable_idle_time_ms,
            )
            .field(
                "max_evictable_idle_time_ms",
                &self.max_evictable_idle_time_ms,
            )
            .field("test_on_borrow", &self.test_on_borrow)
            .field("test_on_return", &self.test_on_return)
            .field("test_while_idle", &self.test_while_idle)
            .field("validation_query", &self.validation_query)
            .field(
                "validation_query_timeout_secs",
                &self.validation_query_timeout_secs,
            )
            .field("pool_prepared_statements", &self.pool_prepared_statements)
            .field(
                "max_pool_prepared_statement_per_connection_size",
                &self.max_pool_prepared_statement_per_connection_size,
            )
            .field("keep_alive", &self.keep_alive)
            .field(
                "keep_alive_between_time_ms",
                &self.keep_alive_between_time_ms,
            )
            .field("filters", &self.filters)
            .field("connection_properties", &self.connection_properties)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("socket_timeout_secs", &self.socket_timeout_secs)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_sets_credentials() {
        let cfg = DruidConfig::new("jdbc:mysql://localhost/db", "root", "s3cret");
        assert_eq!(cfg.url, "jdbc:mysql://localhost/db");
        assert_eq!(cfg.username, "root");
        assert_eq!(cfg.password, "s3cret");
    }

    #[test]
    fn test_new_defaults() {
        let cfg = DruidConfig::new("", "", "");
        assert_eq!(cfg.initial_size, 0);
        assert_eq!(cfg.min_idle, 0);
        assert_eq!(cfg.max_active, 8);
        assert_eq!(cfg.max_wait_ms, 0);
        assert_eq!(cfg.time_between_eviction_runs_ms, 60_000);
        assert_eq!(cfg.min_evictable_idle_time_ms, 1_800_000);
        assert_eq!(cfg.max_evictable_idle_time_ms, 25_200_000);
        assert_eq!(cfg.max_lifetime_ms, 0);
        assert!(cfg.test_on_borrow);
        assert!(!cfg.test_on_return);
        assert!(!cfg.test_while_idle);
        assert_eq!(cfg.validation_query_timeout_secs, 0);
        assert!(!cfg.pool_prepared_statements);
        assert_eq!(cfg.max_pool_prepared_statement_per_connection_size, 10);
        assert!(!cfg.keep_alive);
        assert_eq!(cfg.keep_alive_between_time_ms, 120_000);
        assert!(cfg.filters.is_empty());
        assert!(cfg.connection_properties.is_empty());
        assert_eq!(cfg.connect_timeout_secs, 30);
        assert_eq!(cfg.socket_timeout_secs, 30);
    }

    #[test]
    fn test_default_trait_matches_new() {
        let d = DruidConfig::default();
        let n = DruidConfig::new("", "", "");
        assert_eq!(d.url, n.url);
        assert_eq!(d.max_active, n.max_active);
        assert_eq!(d.test_on_borrow, n.test_on_borrow);
        assert_eq!(d.time_between_eviction_runs_ms, n.time_between_eviction_runs_ms);
    }

    #[test]
    fn test_max_wait_boundaries() {
        let cfg = DruidConfig::new("", "", "");
        assert_eq!(cfg.max_wait(), None); // 0 表示不限制
        let mut cfg = cfg.clone();
        cfg.max_wait_ms = 500;
        assert_eq!(cfg.max_wait(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_duration_helpers() {
        let mut cfg = DruidConfig::new("", "", "");
        cfg.time_between_eviction_runs_ms = 1234;
        cfg.keep_alive_between_time_ms = 5678;
        cfg.connect_timeout_secs = 42;
        assert_eq!(cfg.eviction_interval(), Duration::from_millis(1234));
        assert_eq!(cfg.keep_alive_interval(), Duration::from_millis(5678));
        assert_eq!(cfg.connect_timeout(), Duration::from_secs(42));
    }

    #[test]
    fn test_debug_masks_password() {
        let cfg = DruidConfig::new("jdbc:mysql://h/db", "root", "topsecret");
        let dbg = format!("{:?}", cfg);
        assert!(!dbg.contains("topsecret"));
        assert!(dbg.contains("***"));
        assert!(dbg.contains("root"));
    }

    // serde 序列化/反序列化测试见 druid-console 中的跨 crate 测试
    // （serde_json 非本 crate 依赖，且不允许修改 Cargo.toml）

    #[test]
    fn test_config_is_clonable() {
        let cfg = DruidConfig::new("url", "u", "p");
        let cloned = cfg.clone();
        assert_eq!(cloned.url, cfg.url);
        assert_eq!(cloned.password, cfg.password);
    }
}
