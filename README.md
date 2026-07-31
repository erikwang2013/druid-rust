# Druid Rust

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-49%20passed-green)]()

[English Documentation](docs/README_EN.md) | 中文文档

**Druid-Rust** 是 [Alibaba Druid](https://github.com/alibaba/druid) 的 Rust 移植版——将 JDBC 连接池、SQL 解析分析、安全防护和监控统计深度整合为一体，是 Rust 生态中功能最全面的数据库连接池之一。

## 目录

- [架构设计](#架构设计)
- [项目结构](#项目结构)
- [核心功能](#核心功能)
- [快速开始](#快速开始)
- [使用教程](#使用教程)
- [配置参考](#配置参考)
- [从 Java Druid 迁移](#从-java-druid-迁移)
- [开发与测试](#开发与测试)

## 架构设计

### 分层架构

```
┌─────────────────────────────────────────────────┐
│                  druid-console                   │  ← Web 监控控制台
│              (axum HTTP + 监控页面)               │
├─────────────────────────────────────────────────┤
│     druid-ha (高可用)    │   druid-proxy (代理)   │  ← 高级特性层
│   加权轮询 / 故障切换     │  Filter 自动拦截       │
├─────────────────────────────────────────────────┤
│                  druid-pool                      │  ← 连接池核心
│   Semaphore 并发 / 驱逐 / KeepAlive / PSCache    │
├──────────────────┬──────────────────────────────┤
│   druid-wall     │       druid-stat             │  ← Filter 插件层
│   SQL 防火墙      │       监控统计               │
├──────────────────┴──────────────────────────────┤
│                druid-filter                      │  ← 可插拔架构
│          Filter-Chain 责任链 (20+ 钩子)          │
├─────────────────────────────────────────────────┤
│                 druid-sql                        │  ← SQL 解析引擎
│      Lexer / Parser / AST / Visitor / Format     │
├─────────────────────────────────────────────────┤
│          druid-core  +  druid-util               │  ← 基础设施
│      DruidError / DbType / DruidConfig           │
└─────────────────────────────────────────────────┘
```

### 设计原则

**1. 异步优先 (Async-First)**
- 基于 `tokio` 运行时，全链路异步 I/O
- `Semaphore` 实现非阻塞并发控制，支持获取超时
- 后台驱逐和 KeepAlive 通过 `tokio::spawn` 驱动

**2. 组合优于继承 (Composition over Inheritance)**
- Java Druid 的类继承体系改为 Rust `trait` + `struct` 组合
- `Filter` trait 提供 20+ 生命周期钩子，默认空实现
- `dyn Filter` trait 对象构建可插拔责任链

**3. 类型安全 (Type-Safe)**
- `DbType` 枚举穷举 30 种数据库方言
- `SQLStatement`/`SQLExpr` 代数数据类型，编译器检查完整性
- `DruidError` 枚举（`thiserror`），保留错误类型信息

**4. 零成本抽象 (Zero-Cost Abstraction)**
- AOT 编译为原生二进制，无 JIT 预热
- `PoolGuard` RAII 模式，零运行时开销的自动归还
- `#[inline]` + 单态化消除动态分发开销

### 数据流

```
用户代码
  │
  ▼
DruidDataSource.get_connection()     ← Semaphore 许可控制
  │
  ├─► FilterChain.connection_borrowed()   ← WallFilter/StatFilter 回调
  │
  ▼
PoolGuard (使用连接)                  ← 池化连接
  │
  ▼
PoolGuard::drop()                     ← RAII 自动归还
  │
  ├─► FilterChain.connection_returned()
  │
  ▼
空闲队列 ← 等待下次借用或被驱逐

后台任务 (tokio::spawn):
  • Evictor:  定时清理超时空闲连接
  • KeepAlive: 定时验证空闲连接有效性
```

## 项目结构

```
druid-rust/
├── README.md                          # 项目说明（本文件）
├── Cargo.toml                         # workspace 根，管理 10 个 crate
├── docs/
│   ├── PLAN.md                        # 重构规划文档
│   └── README_EN.md                   # 英文文档
│
├── druid-core/                        # ── 层 0: 基础设施 ──
│   └── src/{error.rs, types.rs, config.rs}
│        • DruidError (thiserror 枚举)
│        • DbType (30 种数据库方言)
│        • DruidConfig (完整连接池配置)
│
├── druid-util/                        # ── 层 0: 工具库 ──
│   └── src/{sql.rs, string.rs, crypto.rs, time.rs}
│        • SQL 类型检测、JDBC URL 推断
│        • 驼峰/下划线转换、参数替换
│        • 密码加解密、时间格式化
│
├── druid-sql/                         # ── 层 1: SQL 解析 ──
│   └── src/
│        ├── token.rs                  # 80+ Token 定义 + 关键字映射
│        ├── ast/expr.rs               # 30 种表达式 + 6 种语句 AST
│        ├── parser/lexer.rs           # 词法分析器
│        ├── parser/mod.rs             # 递归下降 Parser
│        ├── parser/dialects/mysql.rs  # 方言扩展
│        ├── visitor/schema.rs         # SchemaStatVisitor
│        └── format.rs                 # AST → SQL 字符串
│
├── druid-filter/                      # ── 层 2: Filter 架构 ──
│   └── src/{lib.rs, adapter.rs, chain.rs, manager.rs}
│        • Filter trait (20+ 生命周期钩子)
│        • FilterAdapter 默认空实现
│        • FilterChain 责任链
│        • FilterManager
│
├── druid-wall/                        # ── 层 3: 防火墙 ──
│   └── src/{config.rs, checker.rs, provider.rs, lib.rs}
│        • WallConfig 13 项安全配置
│        • WallChecker AST 级安全检查
│        • WallProvider LRU 检查缓存
│        • WallFilter (实现 Filter trait)
│
├── druid-stat/                        # ── 层 3: 监控 ──
│   └── src/{lib.rs, metrics.rs}
│        • StatFilter SQL 执行统计
│        • 慢 SQL 检测 + tracing 告警
│        • DataSourceStat 连接池指标
│        • PoolMetrics lock-free 指标
│
├── druid-pool/                        # ── 层 4: 连接池 ──
│   ├── src/{driver.rs, datasource.rs, pscache.rs}
│   │    • Driver + Connection 异步 trait
│   │    • DruidDataSource 核心实现
│   │    • Semaphore 并发控制
│   │    • 驱逐/KeepAlive 后台任务
│   │    • PoolGuard RAII 自动归还
│   │    • PSCache LRU 缓存
│   ├── benches/pool_bench.rs          # 性能基准
│   └── examples/basic.rs              # 使用示例
│
├── druid-proxy/                       # ── 层 5: 代理 ──
│   └── src/lib.rs
│        • ProxyConnection 包装层
│        • ProxyStatement Filter 拦截
│
├── druid-ha/                          # ── 层 5: 高可用 ──
│   └── src/lib.rs
│        • 加权轮询负载均衡
│        • 健康检查 + 故障切换
│        • mark_down/mark_up 手动切换
│
└── druid-console/                     # ── 层 6: 控制台 ──
    └── src/lib.rs
         • /druid/stat.json
         • /druid/sql.json
         • /druid/slow-sql.json
         • /druid/index.html (监控页面)
```

### Crate 依赖关系

```
druid-console ──────► druid-stat ──────► druid-filter ──────► druid-core
druid-ha ───────────► druid-pool ──────► druid-filter         druid-util
druid-proxy ────────► druid-pool        druid-util
                      druid-wall ──────► druid-sql
                                         druid-filter
```

## 核心功能

### 1. 连接池 (druid-pool)

| 功能 | 说明 |
|------|------|
| 并发控制 | `tokio::sync::Semaphore` 精确限制 max_active |
| 连接预热 | `init()` 时预创建 initial_size 个连接 |
| 自动归还 | `PoolGuard` RAII，drop 时自动归还空闲队列 |
| 获取超时 | `max_wait_ms` 超时自动返回错误 |
| 空闲驱逐 | 后台 `tokio::spawn` 定时清理超时空闲连接 |
| KeepAlive | 后台定时验证连接有效性（锁提前释放） |
| 借还验证 | `test_on_borrow` / `test_on_return` 可选开启 |
| PSCache | SQL → PreparedStatement LRU 缓存 |
| Filter 集成 | 完整的 FilterChain 生命周期钩子 |

### 2. SQL 解析器 (druid-sql)

| 功能 | 说明 |
|------|------|
| 词法分析 | 80+ Token 类型，关键字/标识符/字面量/操作符/注释 |
| 递归下降解析 | SELECT/JOIN/WHERE/GROUP/ORDER/LIMIT/INSERT/UPDATE/DELETE/CREATE/DROP |
| 表达式 | 算术/比较/逻辑/函数调用/CASE WHEN/子查询/IN/BETWEEN/LIKE/EXISTS/聚合 |
| 30 种方言 | MySQL/PostgreSQL/Oracle/SQLServer/DB2/H2/ClickHouse/Doris/StarRocks/... |
| Schema 提取 | `SchemaVisitor` 提取引用的表名和列名 |
| 格式化 | AST → SQL 字符串双向转换 |

### 3. SQL 防火墙 (druid-wall)

| 检查项 | 机制 |
|--------|------|
| 操作禁止 | 可配置禁止 SELECT/INSERT/UPDATE/DELETE/DROP/TRUNCATE 等 |
| 危险函数 | 拦截 SLEEP/BENCHMARK/LOAD_FILE 等注入函数 |
| WHERE 强制 | UPDATE/DELETE 无 WHERE 子句拦截 |
| 文件写入 | SELECT INTO OUTFILE 拦截 |
| 多语句 | 分号分隔的多语句拦截 |
| 关键字 | 自定义禁止关键字列表 |
| SQL 长度 | 最大长度限制 |
| 缓存 | LRU 512 条检查结果缓存，命中率统计 |

### 4. 监控统计 (druid-stat)

- 每条 SQL 的执行次数、总耗时、最大耗时、错误数
- 慢 SQL 自动检测 + `tracing::warn!` 告警
- 连接池指标：活跃/空闲/借用/创建/关闭/等待时间
- `PoolMetrics` lock-free AtomicU64，可对接 Prometheus
- Web 控制台实时查看

### 5. 高可用 (druid-ha)

- 加权轮询多数据源负载均衡
- 异步健康检查（手动触发或独立线程循环）
- 自动故障切换（Down → Testing → Active）

## 快速开始

### 添加依赖

```toml
[dependencies]
druid-core = "0.1"
druid-pool = "0.1"
druid-wall = "0.1"
druid-stat = "0.1"
tokio = { version = "1", features = ["full"] }
```

### 基础用法

```rust
use druid_core::DruidConfig;
use druid_pool::DruidDataSource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = DruidConfig::new(
        "mysql://localhost:3306/mydb",
        "root",
        "password",
    );
    config.initial_size = 5;
    config.max_active = 20;
    config.min_idle = 3;

    let ds = DruidDataSource::new(your_mysql_driver, config);
    ds.init().await?;

    let guard = ds.get_connection().await?;
    // ... 执行 SQL ...
    drop(guard); // 自动归还

    ds.close().await?;
    Ok(())
}
```

## 使用教程

### 集成 SQL 防火墙

```rust
use druid_wall::{WallConfig, WallFilter};

let wall = WallFilter::new(WallConfig {
    update_delete_require_where: true,
    deny_functions: vec!["SLEEP".into(), "BENCHMARK".into()],
    ..Default::default()
});
// wall 实现 Filter trait，可插入 FilterChain
```

### 添加 SQL 监控

```rust
use druid_stat::StatFilter;
use std::sync::Arc;

let stat = Arc::new(StatFilter::new("mydb", 1000)); // 1000ms 慢 SQL 阈值

// 获取统计
let sql_stats = stat.get_sql_stats();      // 按耗时排序的 SQL 列表
let slow_sql = stat.get_slow_sql();         // 超过阈值的慢 SQL
let ds_stat = stat.get_datasource_stat();   // 连接池概览
println!("执行次数: {}, 慢SQL: {}", ds_stat.execute_count, slow_sql.len());
```

### 启动 Web 监控控制台

```rust
use druid_console;
use std::sync::Arc;

let stat = Arc::new(StatFilter::new("app-db", 500));
tokio::spawn(async {
    druid_console::start_server(stat, "127.0.0.1:9090").await.unwrap();
});
// 浏览器访问 http://127.0.0.1:9090/druid/index.html
```

### 高可用多数据源

```rust
use druid_ha::HighAvailableDataSource;
use std::time::Duration;

let mut ha = HighAvailableDataSource::new();
ha.add_node("master", master_ds, 2);     // 权重 2
ha.add_node("slave-1", slave1_ds, 1);    // 权重 1
ha.add_node("slave-2", slave2_ds, 1);    // 权重 1
ha.set_check_interval(Duration::from_secs(30));

let ds = ha.get_datasource().await?;      // 加权轮询
ha.mark_down("master");                   // master 故障，切到 slave
ha.mark_up("master");                     // 恢复
ha.spawn_health_check_loop();             // 启动健康检查循环
```

### 自定义 Filter

```rust
use druid_filter::{Filter, FilterContext};
use druid_core::DruidError;

struct MyLogFilter;

impl Filter for MyLogFilter {
    fn name(&self) -> &'static str { "my-log" }

    fn connection_borrowed(&self, ctx: &FilterContext, wait_ms: u64) {
        tracing::info!("conn borrowed: wait={}ms", wait_ms);
    }

    fn statement_execute_before(&self, ctx: &FilterContext) -> Result<(), DruidError> {
        if let Some(sql) = &ctx.sql { tracing::debug!("executing: {}", sql); }
        Ok(())
    }

    fn statement_execute_after(&self, ctx: &FilterContext, elapsed_ms: u64, rows: u64) {
        tracing::info!("executed in {}ms, {} rows", elapsed_ms, rows);
    }
}
```

## 配置参考

### DruidConfig 全部参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `url` | String | — | 数据库连接 URL |
| `username` | String | — | 用户名 |
| `password` | String | — | 密码 |
| `initial_size` | usize | 0 | 初始化连接数 |
| `min_idle` | usize | 0 | 最小空闲连接数 |
| `max_active` | usize | 8 | 最大活跃连接数 |
| `max_wait_ms` | u64 | 0 | 获取连接最大等待(0=无限) |
| `time_between_eviction_runs_ms` | u64 | 60000 | 驱逐检查间隔 |
| `min_evictable_idle_time_ms` | u64 | 0 | 连接最小空闲存活时间 |
| `max_evictable_idle_time_ms` | u64 | 0 | 连接最大空闲存活时间 |
| `test_on_borrow` | bool | true | 获取时验证 |
| `test_on_return` | bool | false | 归还时验证 |
| `test_while_idle` | bool | false | 空闲时验证 |
| `validation_query` | Option\<String\> | None | 验证 SQL |
| `pool_prepared_statements` | bool | false | 启用 PSCache |
| `max_pool_prepared_statement` | usize | 10 | PSCache 大小 |
| `keep_alive` | bool | false | 启用 KeepAlive |
| `keep_alive_between_time_ms` | u64 | 120000 | KeepAlive 间隔 |
| `filters` | Vec\<String\> | [] | Filter 列表 |
| `connection_properties` | Vec\<String\> | [] | 连接属性 |
| `connect_timeout_secs` | u64 | 30 | 连接超时 |
| `socket_timeout_secs` | u64 | 30 | Socket 超时 |

### WallConfig 安全参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | bool | true | 启用防火墙 |
| `deny_operations` | Vec\<DenyOperation\> | [Truncate,DropTable,AlterTable] | 禁止操作 |
| `deny_functions` | Vec\<String\> | [SLEEP,BENCHMARK,LOAD_FILE] | 禁止函数 |
| `deny_keywords` | Vec\<String\> | [] | 禁止关键字 |
| `deny_tables` | Vec\<String\> | [] | 禁止表 |
| `deny_schemas` | Vec\<String\> | [] | 禁止 Schema |
| `max_sql_length` | usize | 8192 | 最大 SQL 长度 |
| `allow_multi_statements` | bool | false | 允许多语句 |
| `update_delete_require_where` | bool | true | UPDATE/DELETE 强制 WHERE |
| `select_into_outfile_allow` | bool | false | 允许 SELECT INTO OUTFILE |
| `table_whitelist_mode` | bool | false | 表白名单模式 |

## 从 Java Druid 迁移

基于 [coding-to-rust/java-to-rust](https://github.com) 规则：

| Java | Rust | 说明 |
|------|------|------|
| `class` + 继承 | `struct` + `trait` | 组合优于继承 |
| JVM | Native binary (LLVM) | AOT 编译，零预热 |
| Checked Exception | `Result<T, E>` + `thiserror` | 错误即值 |
| `synchronized` | `Mutex<T>` / `RwLock<T>` | 数据放入锁内 |
| `Optional<T>` | `Option<T>` | 穷尽模式匹配 |
| Spring Boot DI | 构造函数注入 | 无 DI 容器 |
| JPA/Hibernate | sqlx (async-native) | 显式 SQL |
| `volatile` | `AtomicBool` / `Ordering` | 显式内存排序 |
| Annotation | `#[derive(...)]` | 编译期代码生成 |
| ServiceLoader SPI | `linkme` / 手动注册 | 链接期发现 |
| Filter 责任链 | `Vec<Box<dyn Filter>>` | trait 对象 |
| Visitor 模式 | trait + enum match | 穷尽匹配 |
| `Thread` / ExecutorService | `tokio::spawn` | M:N 调度 |

关键差异：
- **不需要 DI 容器**：Rust 中构造函数注入即够用，无需 Spring
- **不需要 ORM**：sqlx 编译期检查 SQL，无需 Hibernate 的运行时延迟加载
- **没有 JDBC 标准**：自定义 `Driver`/`Connection` async trait 替代 JDBC 接口
- **MutexGuard 不能跨 `.await`**：后台任务需提前 clone 所需数据

## 开发与测试

### 运行测试

```bash
cargo test
# 49 passed; 0 failed
```

### 运行基准

```bash
cargo bench --bench pool_bench
```

### 运行示例

```bash
cargo run --example basic
```

### 测试分布

| Crate | 测试数 |
|-------|--------|
| druid-core | 0 |
| druid-util | 15 |
| druid-sql | 8 |
| druid-filter | 5 |
| druid-wall | 7 |
| druid-pool | 7 |
| druid-stat | 3 |
| druid-console | 0 |
| druid-proxy | 2 |
| druid-ha | 2 |
| **总计** | **49** |

### 构建

```bash
cargo build              # debug
cargo build --release    # release (LTO, codegen-units=1)
cargo clippy -- -D warnings
cargo fmt --check
```

## License

Apache 2.0 — 与 [Alibaba Druid](https://github.com/alibaba/druid) 保持一致。

---

[English Documentation](docs/README_EN.md)
