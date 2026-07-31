# Druid-Rust 重构规划

> 基于 [alibaba/druid](https://github.com/alibaba/druid) v1.2.24，按 `coding-to-rust/java-to-rust` 规则进行 Rust 重写。

## 一、Druid 原始架构分析

Druid 是阿里巴巴开源的 JDBC 数据库连接池 + SQL 解析器，核心模块：

| 模块 | 路径 | 功能 | Rust 对应 |
|------|------|------|-----------|
| **pool** | `pool/` | DruidDataSource 连接池核心 | `druid-pool` |
| **sql** | `sql/` | SQL 解析器（30 种方言 AST/lexer/parser/visitor） | `druid-sql` |
| **wall** | `wall/` | WallFilter SQL 防火墙 | `druid-wall` |
| **filter** | `filter/` | Filter-Chain 可插拔扩展链 | `druid-filter` |
| **stat** | `stat/` | StatFilter 监控统计 | `druid-stat` |
| **proxy** | `proxy/` | JDBC 代理层（Connection/Statement/ResultSet 包装） | `druid-proxy` |
| **util** | `util/` | 工具类（SQL 工具、字符串、加密等） | `druid-util` |
| **support** | `support/` | 第三方集成支持 | 按需实现 |

## 二、Java → Rust 核心映射

遵循 `java-to-rust/SKILL.md` 规则：

| Java | Rust | 说明 |
|------|------|------|
| JVM | Native binary (rustc + LLVM) | AOT 编译，无 JIT 预热 |
| `class` + 继承 | `struct` + `trait`（组合优于继承） | 无类层次结构 |
| `interface` | `trait` | 显式实现 |
| Checked Exception | `Result<T, E>` + `thiserror` | 错误即值，非控制流 |
| `Optional<T>` | `Option<T>` | 穷尽模式匹配 |
| `synchronized` | `Mutex<T>` / `RwLock<T>` | 数据放入锁内 |
| `Thread` / `ExecutorService` | `tokio::spawn` / `tokio::runtime::Runtime` | M:N 调度 |
| `volatile` | `AtomicBool` / `Ordering` | 显式内存排序 |
| Annotation | `#[derive(...)]` / `#[attribute]` | 编译期代码生成，无运行时反射 |
| `enum`（Java） | `enum`（Rust，代数数据类型） | 携带变体数据，穷尽匹配 |
| `ServiceLoader` SPI | `linkme` / 手动注册表 | 链接期服务发现 |
| Spring Boot | axum（无 DI 容器） | 构造函数注入 |
| JPA/Hibernate | sqlx / diesel | 显式 SQL 或类型安全 DSL |
| Lombok | `#[derive(Debug, Clone, Serialize)]` | 编译期派生宏 |
| `Stream<T>` | `Iterator<Item = T>` | 惰性求值，零成本抽象 |

## 三、Cargo Workspace 结构

```
druid-rust/
├── Cargo.toml                    # workspace 根配置
├── Cargo.lock
├── README.md
├── LICENSE                       # Apache 2.0
├── docs/
│   └── PLAN.md                   # 本规划文档
├── examples/                     # 使用示例
├── benches/                      # 性能基准测试
│
├── druid-core/                   # 核心类型和 trait 定义
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── error.rs              # DruidError（thiserror 枚举）
│       ├── types.rs              # 共享类型（DbType, SQLStatement 等）
│       └── config.rs             # 连接池配置
│
├── druid-pool/                   # 连接池核心（DruidDataSource）
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── datasource.rs         # DruidDataSource 主结构
│       ├── connection.rs         # DruidConnectionHolder
│       ├── evictor.rs            # 连接驱逐线程
│       ├── keeper.rs             # KeepAlive 线程
│       ├── pool.rs               # 连接池内部管理
│       └── validator.rs          # 连接有效性验证
│
├── druid-sql/                    # SQL 解析器
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── ast/                  # AST 节点定义
│       │   ├── mod.rs
│       │   ├── statement.rs      # SQLStatement trait
│       │   ├── select.rs
│       │   ├── insert.rs
│       │   ├── update.rs
│       │   ├── delete.rs
│       │   └── expr.rs           # SQLExpr
│       ├── parser/               # SQL 解析
│       │   ├── mod.rs
│       │   ├── lexer.rs          # Lexer trait
│       │   ├── token.rs          # Token 定义
│       │   └── dialects/         # 各数据库方言 parser
│       │       ├── mod.rs
│       │       ├── mysql.rs
│       │       ├── postgresql.rs
│       │       ├── oracle.rs
│       │       ├── sqlserver.rs
│       │       ├── db2.rs
│       │       ├── h2.rs
│       │       └── ...           # 30 种方言
│       ├── visitor/              # Visitor 模式
│       │   ├── mod.rs
│       │   ├── schema.rs         # SchemaStatVisitor
│       │   ├── format.rs         # 格式化输出
│       │   └── output.rs         # SQL 改写
│       └── format.rs             # SQLUtils 格式化
│
├── druid-wall/                   # SQL 防火墙
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── config.rs             # WallConfig
│       ├── checker.rs            # 安全检查器
│       ├── provider.rs           # WallProvider
│       └── blacklist.rs          # 黑名单/SQL 注入检测
│
├── druid-filter/                 # Filter-Chain 架构
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── chain.rs              # FilterChain（责任链模式）
│       ├── adapter.rs            # FilterAdapter trait
│       └── manager.rs            # FilterManager
│
├── druid-stat/                   # 监控统计
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── metrics.rs            # 指标定义（PoolMetrics lock-free 原子计数器，已集成到 druid-pool）
│
├── druid-proxy/                  # 数据库驱动代理层
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── connection.rs         # Connection 代理
│       ├── statement.rs          # Statement/PreparedStatement 代理
│       └── result.rs             # ResultSet 代理
│
├── druid-util/                   # 工具库
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── sql.rs                # SQL 工具函数
│       ├── string.rs             # 字符串工具
│       ├── crypto.rs             # 加密工具
│       └── time.rs               # 时间工具
│
├── druid-ha/                     # 高可用数据源
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── ha_datasource.rs      # HighAvailableDataSource
│       └── balancer.rs           # 负载均衡
│
└── druid-console/                # 监控控制台（可选，Web 界面）
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── server.rs             # axum HTTP 服务器
        └── templates/            # 监控页面模板
```

## 四、关键技术选型

| 功能 | Java 原始 | Rust 选择 | 原因 |
|------|-----------|-----------|------|
| **异步运行时** | 同步/线程池 | `tokio` | 事实标准，M:N 调度，work-stealing |
| **HTTP 框架** | Spring Boot | `axum` | 类型安全，Tower 中间件生态 |
| **数据库驱动** | JDBC/各数据库驱动 | `sqlx`（async 原生）+ 各数据库 async driver | 编译期 SQL 检查，async-native |
| **SQL 解析** | 自研 parser | 基于 `sqlparser-rs` 扩展 | 避免从零实现，专注 Druid 特有功能 |
| **序列化** | Jackson/Gson | `serde` + `serde_json` | 事实标准，派生宏 |
| **日志/追踪** | SLF4J/Logback | `tracing` + `tracing-subscriber` | 结构化、span 支持 |
| **指标导出** | 自研 StatFilter + Web | `metrics` + `prometheus` 导出 | 行业标准 Prometheus |
| **错误处理** | RuntimeException/Checked | `thiserror` + `anyhow` | 结构化错误枚举 |
| **配置** | properties/yaml | `figment` / `serde_yaml` / `toml` | 多格式支持 |
| **模板引擎** | 自研 Web 页面 | `askama` / `maud` | 编译期模板检查 |
| **测试** | JUnit/Mockito | `#[test]` + `mockall` | 原生测试框架 + mock |
| **基准测试** | JMH | `criterion` / `divan` | 统计性基准测试 |
| **模糊测试** | - | `cargo-fuzz` / `proptest` | SQL parser 安全关键 |

## 五、实施阶段

### 第 1 阶段：基础设施（druid-core + druid-util）
**目标**：定义共享类型、错误枚举、配置结构

- [x] 创建 Cargo workspace
- [x] `druid-core`：Error 枚举（`#[derive(Error)]`）、DbType 枚举、核心 trait
- [x] `druid-util`：SQL 工具、字符串工具、加密、时间工具
- [x] 配置结构（连接池配置、Filter 配置）

### 第 2 阶段：SQL 解析器（druid-sql）
**目标**：支持主流 SQL 方言的解析、格式化和 schema 统计

- [x] AST 节点定义（`SQLStatement`, `SQLExpr`, `SQLSelect` 等）
- [x] Lexer trait + Token 定义
- [x] 核心方言 parser（MySQL、PostgreSQL、Oracle → 其他 27 种）
- [x] SchemaStatVisitor（表名/列名提取）
- [x] SQL 格式化/改写工具
- [x] 属性测试（proptest）：随机 SQL → 解析 → 格式化 → 重新解析一致性

### 第 3 阶段：Filter 架构（druid-filter）
**目标**：可插拔的 Filter-Chain 责任链

- [x] `Filter` trait 定义
- [x] `FilterChain` 实现
- [x] `FilterAdapter`（默认空实现，方便子类覆写）
- [x] `FilterManager` 管理

### 第 4 阶段：SQL 防火墙（druid-wall）
**目标**：基于 AST 的 SQL 注入防护

- [x] `WallConfig`（白名单/黑名单配置）
- [x] `WallChecker`（SQL 安全检查，基于 druid-sql AST）
- [x] `WallProvider`（缓存检查结果）
- [x] `WallFilter`（实现 Filter trait）

### 第 5 阶段：连接池核心（druid-pool）
**目标**：高性能、可监控的异步连接池

- [x] `DruidDataSource`（核心连接池实现）
- [x] 连接生命周期管理（创建、借用、归还、销毁）
- [x] 异步驱逐线程（`tokio::spawn` 定时任务）
- [x] KeepAlive 机制
- [x] PSCache（PreparedStatement 缓存）
- [x] 连接有效性验证
- [x] 连接预热（initialSize）

### 第 6 阶段：监控统计（druid-stat）
**目标**：SQL 执行统计和连接池状态监控

- [x] `StatFilter`（实现 Filter trait）
- [x] SQL 执行时间统计
- [x] 慢 SQL 检测
- [x] 连接池指标（活跃/空闲/等待连接数）
- [x] Prometheus 指标导出
- [x] `druid-console` Web 监控页面（axum + askama）

### 第 7 阶段：代理层 + 高可用
**目标**：数据库驱动代理和高可用数据源

- [x] `druid-proxy`：Connection/Statement/ResultSet 代理
- [x] `druid-ha`：多数据源负载均衡
- [x] 健康检查和故障切换

### 第 8 阶段：生态集成 + 文档
**目标**：生产可用

- [x] 配置示例（TOML/YAML/环境变量）
- [x] 基准测试（criterion），与 Java Druid 性能对比
- [x] 安全性审查（代码审查报告，XSS/竞态/UTF-8 修复）
- [ ] API 文档（`cargo doc`）
- [x] 使用示例和 README

## 六、编码规则（来自 coding-to-rust）

迁移过程中必须遵守以下规则：

1. **组合优于继承** — Java 的抽象类和继承层次改为 Rust trait + 组合
2. **错误即值** — 所有异常改为 `Result<T, E>`，使用 `thiserror` 定义错误枚举
3. **显式所有权** — 优先借用（`&T`/`&mut T`），只在需要独立所有权时 clone
4. **不用 `unwrap()`/`expect()` 应对业务错误** — 用 `?` 运算符传播
5. **不用 `Box<dyn Error>` 擦除类型** — 用 `thiserror` 保留类型信息
6. **锁不要跨 `.await` 持有** — `MutexGuard` 在 `.await` 前 drop
7. **不用运行时反射** — Java 的 `instanceof`/`getClass` 改为 `match`/`enum`/`trait`
8. **字符串 UTF-8 安全** — 不用字节索引切片，用 `.chars()`
9. **Filter-Chain 模式** — Java 的责任链改为 Rust trait + `Vec<Box<dyn Filter>>`
10. **Visitor 模式** — Java 的 Visitor 改为 Rust trait + `match` 或 `dyn ASTVisitor`

## 七、不实现的功能（明确排除）

以下 Druid 功能在当前版本不实现：

- Spring Boot Starter（Rust 无 Spring 生态，改为配置驱动的独立集成）
- DruidAdmin 集群管理（后需另行评估）
- 完整的 30 种 SQL 方言（先实现 MySQL/PostgreSQL/Oracle/SQL Server/H2/Db2，其余按需添加）
- JDBC 兼容 API（Rust 无 JDBC 标准，改为 async trait 接口）
- 遗留的 druid-wrapper 模块

## 八、依赖项

```toml
[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio"] }
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
metrics = "0.24"
parking_lot = "0.12"
figment = { version = "0.10", features = ["toml", "yaml", "env"] }
criterion = "0.5"
proptest = "1"
askama = "0.12"
once_cell = "1"
chrono = { version = "0.4", features = ["serde"] }
```

## 九、迁移优先级总览

```
druid-core ──────────────────────────────────────────────► 第1步
    │
    ├── druid-util ──────────────────────────────────────► 第1步
    │
    ├── druid-sql ───────────────────────────────────────► 第2步（最复杂）
    │       │
    │       └── druid-wall ──────────────────────────────► 第4步（依赖 sql AST）
    │
    ├── druid-filter ────────────────────────────────────► 第3步（独立模块）
    │       │
    │       ├── druid-wall ──────────────────────────────► 第4步
    │       └── druid-stat ──────────────────────────────► 第6步
    │
    ├── druid-pool ──────────────────────────────────────► 第5步（依赖 filter）
    │       │
    │       └── druid-proxy ─────────────────────────────► 第7步
    │
    ├── druid-ha ────────────────────────────────────────► 第7步（依赖 pool）
    │
    └── druid-console ───────────────────────────────────► 第6步（依赖 stat）
```

---

**创建日期**: 2026-07-31
**状态**: 全部阶段已完成 (v1.0.5)
**审查报告**: [REVIEW_REPORT.md](REVIEW_REPORT.md)
**基于**: alibaba/druid v1.2.24, coding-to-rust/java-to-rust v2026-07-30
