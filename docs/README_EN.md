# Druid-Rust

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-61%20passed-green)]()

[中文文档](../README.md) | English

**Druid-Rust** is a Rust port of [Alibaba Druid](https://github.com/alibaba/druid) — a high-performance, monitorable database connection pool with integrated SQL parsing, security firewall, and statistics.

## Table of Contents

- [Architecture](#architecture)
- [Project Structure](#project-structure)
- [Features](#features)
- [Quick Start](#quick-start)
- [Usage Guide](#usage-guide)
- [Configuration Reference](#configuration-reference)
- [Java to Rust Migration](#java-to-rust-migration)
- [Development & Testing](#development--testing)
- [Review Report](#review-report)

## Architecture

### Layered Design

```
┌─────────────────────────────────────────────────┐
│                  druid-console                   │  ← Web monitoring
│               (axum HTTP + dashboard)            │
├─────────────────────────────────────────────────┤
│      druid-ha (HA)       │   druid-proxy (proxy) │  ← Advanced layer
│   Weighted round-robin    │   Filter interception │
├─────────────────────────────────────────────────┤
│                  druid-pool                      │  ← Connection pool core
│    Semaphore / Eviction / KeepAlive / PSCache    │
├──────────────────┬──────────────────────────────┤
│   druid-wall     │       druid-stat             │  ← Filter plugins
│   SQL Firewall    │       Statistics             │
├──────────────────┴──────────────────────────────┤
│                druid-filter                      │  ← Pluggable architecture
│           Filter-Chain (20+ hooks)               │
├─────────────────────────────────────────────────┤
│                 druid-sql                        │  ← SQL parsing engine
│       Lexer / Parser / AST / Visitor / Format    │
├─────────────────────────────────────────────────┤
│           druid-core  +  druid-util              │  ← Foundation
│        DruidError / DbType / DruidConfig          │
└─────────────────────────────────────────────────┘
```

### Design Principles

**1. Async-First**
- Built on `tokio` runtime, fully asynchronous I/O
- `Semaphore`-based non-blocking concurrency control with timeout
- Background eviction and KeepAlive driven by `tokio::spawn`

**2. Composition over Inheritance**
- Java class hierarchy → Rust `trait` + `struct` composition
- `Filter` trait with 20+ lifecycle hooks, default no-op implementations
- `dyn Filter` trait objects for pluggable filter chains

**3. Type Safety**
- `DbType` enum covers 30 database dialects exhaustively
- `SQLStatement`/`SQLExpr` as algebraic data types
- `DruidError` via `thiserror`, preserving error type information

**4. Zero-Cost Abstractions**
- AOT-compiled native binary, no JIT warmup
- `PoolGuard` RAII — zero-overhead automatic connection return
- Monomorphization eliminates dynamic dispatch overhead

### Data Flow

```
User code
  │
  ▼
DruidDataSource.get_connection()     ← Semaphore permit
  │
  ├─► FilterChain.connection_borrowed()   ← WallFilter/StatFilter
  │
  ▼
PoolGuard (connection in use)        ← Pooled connection
  │
  ▼
PoolGuard::drop()                    ← RAII auto-return
  │
  ├─► FilterChain.connection_returned()
  │
  ▼
Idle queue ← Wait for next borrow or eviction

Background (tokio::spawn):
  • Evictor:  Periodic idle connection cleanup
  • KeepAlive: Periodic idle connection validation
```

## Project Structure

```
druid-rust/
├── README.md              # Chinese documentation
├── Cargo.toml             # Workspace root (10 crates)
├── docs/
│   ├── PLAN.md            # Migration plan
│   ├── README_EN.md       # English docs (this file)
│   └── REVIEW_REPORT.md   # Code review report
│
├── druid-core/            # Foundation: DruidError, DbType, DruidConfig
├── druid-util/            # Utilities: SQL, string, crypto, time tools
├── druid-sql/             # SQL parser: Lexer, Parser, AST, Visitor, Formatter
├── druid-filter/          # Filter chain: Filter trait, FilterChain, FilterAdapter
├── druid-wall/            # SQL firewall: WallChecker, WallProvider, WallFilter
├── druid-stat/            # Statistics: StatFilter, slow SQL, PoolMetrics
├── druid-pool/            # Connection pool: DruidDataSource, PoolGuard, PSCache
│   ├── benches/           # Performance benchmarks
│   └── examples/          # Usage examples
├── druid-proxy/           # Proxy layer: ProxyConnection, ProxyStatement
├── druid-ha/              # High availability: load balancing, failover
└── druid-console/         # Web console: axum server, dashboard, JSON APIs
```

### Crate Dependency Graph

```
druid-console → druid-stat → druid-filter → druid-core
druid-ha      → druid-pool → druid-filter    druid-util
druid-proxy   → druid-pool    druid-util
                druid-wall → druid-sql
                             druid-filter
```

## Features

### Connection Pool (druid-pool)

| Feature | Implementation |
|---------|---------------|
| Concurrency | `tokio::sync::Semaphore` precise max_active limiting |
| Warmup | Pre-create `initial_size` connections on `init()` |
| Auto-return | `PoolGuard` RAII — returned on drop |
| Borrow timeout | `max_wait_ms` with automatic error |
| Eviction | Background task for idle connection cleanup |
| KeepAlive | Background validation of idle connections |
| Validation | `test_on_borrow` / `test_on_return` |
| PSCache | SQL → PreparedStatement LRU cache |
| Filter hooks | Complete FilterChain lifecycle integration |

### SQL Parser (druid-sql)

| Feature | Coverage |
|---------|----------|
| Lexer | 80+ token types (keywords, identifiers, literals, operators, comments) |
| Recursive descent | SELECT/JOIN/WHERE/GROUP/ORDER/LIMIT/INSERT/UPDATE/DELETE/CREATE/DROP |
| Expressions | Arithmetic/comparison/logical/CASE WHEN/subqueries/IN/NOT IN/BETWEEN/NOT BETWEEN/LIKE/NOT LIKE/EXISTS/aggregates |
| Dialects | 30 dialects (MySQL, PostgreSQL, Oracle, SQLServer, DB2, H2, ClickHouse, ...) |
| Schema visitor | Table and column reference extraction |
| Formatter | AST ↔ SQL bidirectional conversion |

### SQL Firewall (druid-wall)

| Check | Detail |
|-------|--------|
| Operation deny | Configurable: SELECT/INSERT/UPDATE/DELETE/DROP/TRUNCATE |
| Function deny | SLEEP/BENCHMARK/LOAD_FILE and custom |
| WHERE enforcement | UPDATE/DELETE without WHERE blocked |
| File output | SELECT INTO OUTFILE blocked |
| Multi-statement | Semicolon-delimited statements blocked |
| Keyword deny | Custom keyword blacklist |
| Length limit | `max_sql_length` check |
| Cache | LRU 512-entry with hit-rate stats |

### Statistics (druid-stat)

- Per-SQL: count, total time, max time, error count
- Slow SQL detection with `tracing::warn!` alerts
- Pool metrics: active/idle/borrow/create/destroy/wait
- `PoolMetrics` lock-free AtomicU64 (Prometheus-ready)
- Real-time web dashboard

### High Availability (druid-ha)

- Weighted round-robin load balancing
- Async health checks
- Automatic failover: Down → Testing → Active

## Quick Start

```toml
[dependencies]
druid-core = "1.0"
druid-pool = "1.0"
tokio = { version = "1", features = ["full"] }
```

```rust
use druid_core::DruidConfig;
use druid_pool::DruidDataSource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = DruidConfig::new("mysql://localhost:3306/mydb", "root", "pass");
    config.initial_size = 5;
    config.max_active = 20;

    let ds = DruidDataSource::new(your_driver, config);
    ds.init().await?;

    let guard = ds.get_connection().await?;
    // ... use connection ...
    drop(guard); // auto-return

    ds.close().await?;
    Ok(())
}
```

## Usage Guide

### SQL Firewall

```rust
use druid_wall::{WallConfig, WallFilter};

let wall = WallFilter::new(WallConfig {
    update_delete_require_where: true,
    deny_functions: vec!["SLEEP".into(), "BENCHMARK".into()],
    ..Default::default()
});
```

### SQL Monitoring

```rust
use druid_stat::StatFilter;
use std::sync::Arc;

let stat = Arc::new(StatFilter::new("mydb", 1000));
let sql_stats = stat.get_sql_stats();      // Sorted by total time
let slow_sql = stat.get_slow_sql();         // Above threshold
let ds_stat = stat.get_datasource_stat();   // Pool overview
```

### Web Console

```rust
let stat = Arc::new(StatFilter::new("app-db", 500));
tokio::spawn(async { druid_console::start_server(stat, "127.0.0.1:9090").await.unwrap(); });
// Open http://127.0.0.1:9090/druid/index.html
```

### High Availability

```rust
use druid_ha::HighAvailableDataSource;

let mut ha = HighAvailableDataSource::new();
ha.add_node("master", master_ds, 2);    // Weight 2
ha.add_node("slave", slave_ds, 1);      // Weight 1
let ds = ha.get_datasource().await?;     // Weighted round-robin
ha.mark_down("master");                  // Failover to slave
ha.spawn_health_check_loop();            // Background health checks
```

### Custom Filter

```rust
use druid_filter::{Filter, FilterContext};
use druid_core::DruidError;

struct MyFilter;
impl Filter for MyFilter {
    fn name(&self) -> &'static str { "my" }
    fn statement_execute_before(&self, ctx: &FilterContext) -> Result<(), DruidError> { Ok(()) }
    fn statement_execute_after(&self, ctx: &FilterContext, elapsed_ms: u64, rows: u64) {
        tracing::info!("SQL: {}ms, {} rows", elapsed_ms, rows);
    }
}
```

## Configuration Reference

### DruidConfig

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `url` | String | — | Database URL |
| `username` | String | — | Username |
| `password` | String | — | Password |
| `initial_size` | usize | 0 | Initial connections |
| `min_idle` | usize | 0 | Min idle |
| `max_active` | usize | 8 | Max active |
| `max_wait_ms` | u64 | 0 | Max wait (0=∞) |
| `time_between_eviction_runs_ms` | u64 | 60000 | Eviction interval |
| `min_evictable_idle_time_ms` | u64 | 1800000 | Min idle before eviction (30min) |
| `max_evictable_idle_time_ms` | u64 | 25200000 | Max idle before eviction (7h) |
| `test_on_borrow` | bool | true | Validate on borrow |
| `test_on_return` | bool | false | Validate on return |
| `keep_alive` | bool | false | Enable KeepAlive |
| `keep_alive_between_time_ms` | u64 | 120000 | KeepAlive interval |
| `pool_prepared_statements` | bool | false | Enable PSCache |
| `connect_timeout_secs` | u64 | 30 | Connect timeout |
| `socket_timeout_secs` | u64 | 30 | Socket timeout |

### WallConfig

| Parameter | Type | Default |
|-----------|------|---------|
| `enabled` | bool | true |
| `deny_operations` | Vec\<DenyOperation\> | [Truncate,DropTable,AlterTable] |
| `deny_functions` | Vec\<String\> | [SLEEP,BENCHMARK,LOAD_FILE] |
| `deny_keywords` | Vec\<String\> | [] |
| `deny_schemas` | Vec\<String\> | [] |
| `max_sql_length` | usize | 8192 |
| `allow_multi_statements` | bool | false |
| `update_delete_require_where` | bool | true |
| `select_into_outfile_allow` | bool | false |
| `table_whitelist_mode` | bool | false |

## Java to Rust Migration

Based on [coding-to-rust/java-to-rust](https://github.com):

| Java | Rust | Notes |
|------|------|-------|
| `class` + inheritance | `struct` + `trait` | Composition |
| JVM | Native binary (LLVM) | AOT, no warmup |
| Checked Exception | `Result<T, E>` + `thiserror` | Errors as values |
| `synchronized` | `Mutex<T>` / `RwLock<T>` | Data-inside-lock |
| `Optional<T>` | `Option<T>` | Exhaustive matching |
| Spring Boot DI | Constructor injection | No DI container |
| JPA/Hibernate | sqlx | Compile-time SQL |
| Annotation | `#[derive(...)]` | Compile-time codegen |
| Filter Chain | `Vec<Box<dyn Filter>>` | Trait objects |
| Visitor | trait + enum match | Exhaustive |
| `Thread` | `tokio::spawn` | M:N scheduling |

Key differences from Java:
- No DI container — constructor injection suffices
- No ORM — sqlx checks SQL at compile time
- No JDBC — custom `Driver`/`Connection` async traits
- `MutexGuard` must not cross `.await` boundaries

## Development & Testing

### Code Quality

```bash
cargo check --workspace          # Quick compile check
cargo clippy --all-targets       # Lint check (current: 0 warnings)
cargo fmt --all                  # Format
cargo test --workspace           # 61 passed; 0 failed
```

### Benchmarks

```bash
cargo bench --bench pool_bench
```

### Examples

```bash
cargo run --example basic
```

### Test Distribution

| Crate | Tests |
|-------|-------|
| druid-core | 7 |
| druid-util | 15 |
| druid-sql | 8 |
| druid-filter | 5 |
| druid-wall | 7 |
| druid-pool | 7 |
| druid-stat | 3 |
| druid-console | 5 |
| druid-proxy | 2 |
| druid-ha | 2 |
| **Total** | **61** |

## Review Report

Latest code review: [REVIEW_REPORT.md](REVIEW_REPORT.md)

- `cargo check`: ✅ Zero warnings
- `cargo clippy --all-targets`: ✅ Zero warnings
- `cargo test`: ✅ 61/61 passed
- `cargo fmt --check`: ✅ Consistent formatting

## License

Apache 2.0 — same as [Alibaba Druid](https://github.com/alibaba/druid).

---

## Support

Thank you for using Druid-Rust! If this project helps you, feel free to buy the developer a coffee ☕

<p align="center">
  <table align="center">
    <tr>
      <td align="center" width="200">
        <img src="alipay.png" width="130" height="130" alt="Alipay"><br>
        <b>Alipay</b>
      </td>
      <td align="center" width="200">
        <img src="weixinpay.png" width="130" height="130" alt="WeChat Pay"><br>
        <b>WeChat Pay</b>
      </td>
    </tr>
  </table>
</p>

---

[中文文档](../README.md)
