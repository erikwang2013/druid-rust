# Druid-Rust v1.0.6 代码审查报告

**日期:** 2026-07-31
**分支:** main
**审查范围:** 全部 10 个 crate，30 个 .rs 源文件

---

## 1. 验证结果

| 检查项 | 状态 | 详情 |
|--------|------|------|
| `cargo build` | ✅ 通过 | 零警告 |
| `cargo test` | ✅ 49/49 通过 | 全部通过 |
| `cargo clippy --all-targets -- -D warnings` | ✅ 通过 | 零警告 |

### 测试覆盖统计

| Crate | 测试数 | 覆盖范围 |
|-------|--------|----------|
| druid-core | 0 | 类型定义 + DbType 枚举，间接覆盖 |
| druid-filter | 5 | FilterChain 调用、空链、Filter 名称 |
| druid-util | 15 | 加密/解密、SQL 类型检测、命名转换、字符串处理、时间工具 |
| druid-sql | 8 | 词法分析、SQL 解析、格式化往返、Schema 访问 |
| druid-stat | 3 | SQL 统计、慢 SQL 检测、数据源统计 |
| druid-wall | 7 | 防火墙检查、缓存、禁用模式、关键词拒绝 |
| druid-pool | 7 (2 单元 + 5 集成) | 连接池初始化、连接借用/归还、并发限制、PSCache 淘汰 |
| druid-proxy | 2 | 代理执行、连接关闭 |
| druid-ha | 2 | 高可用轮询、节点标记 |
| druid-console | 0 | HTTP 端点（无测试） |
| **总计** | **49** | |

---

## 2. v1.0.6 已修复问题回顾

上一版报告（v1.0.5）中标记的问题在 v1.0.6 中的修复状态：

| 编号 | 问题 | 状态 |
|------|------|------|
| 2.1 | XSS 漏洞 — druid-console HTML 输出未转义 | ✅ 已修复 (html_escape 函数) |
| 2.2 | 竞态条件 — get_connection 双重加锁 | ✅ 已修复 (合并到同一 lock 作用域) |
| 2.3 | truncate_sql 多字节字符 panic | ✅ 已修复 (使用 char_indices) |
| 2.4 | PSCache 定义但未集成 | ⚠️ 部分修复 (创建实例但未在 statement 执行中使用) |
| 2.5 | PoolMetrics 定义但未使用 | ⚠️ 部分修复 (已引用但指标同步不完整) |
| 2.6 | SQL 解析器缺失 NOT LIKE/NOT BETWEEN/NOT IN | ✅ 已修复 |
| 2.7 | druid-ha 创建多余 Tokio Runtime | ✅ 已修复 (使用 tokio::spawn) |
| 2.8 | SQL Formatter 有未格式化变体 | ✅ 已修复 (所有变体已覆盖) |
| 2.9 | PoolGuard Drop 中 tokio::spawn 风险 | ✅ 已修复 (Handle::try_current + spawn_blocking) |
| 2.10 | camel_to_snake 处理连续大写字母错误 | ✅ 已修复 ("URL" → "url") |
| 2.11 | PSCache::put 重复存储问题 | ✅ 已修复 (contains_key 预检查) |
| 2.12 | quick_check 关键词匹配边界问题 | ✅ 已修复 (ascii_alphanumeric 边界检查) |
| 2.14 | parse_sql 潜在无限循环 | ✅ 已修复 (MAX_ITERATIONS = 10_000) |
| 2.15 | 驱逐时间默认值为 0 | ✅ 已修复 (30min / 7h 默认值) |

---

## 3. 现存问题

### 🟠 高危 (High)

#### 3.1 PoolMetrics 指标与 PoolInner 状态不同步

**文件:** `druid-pool/src/datasource.rs:86-87, 169-207` + `druid-stat/src/metrics.rs`

**问题:** `PoolMetrics`（AtomicU64 无锁）和 `PoolInner`（Mutex 保护）维护了两套独立的活跃/空闲连接计数，二者在 init() 之后不再同步。

- `PoolMetrics::set_active()` 和 `set_idle()` 仅在 `init()` 末尾调用（line 86-87）
- `get_connection()` 从 idle 队列取走连接、active_count += 1，但没有调用 `metrics.set_active()` / `metrics.set_idle()`
- `PoolGuard::drop()` 归还连接时同样没有更新 metrics

**影响:** 通过 `metrics.active()` / `metrics.idle()` 查询到的活跃/空闲连接数始终停留在初始化时的快照，无法反映运行时状态。监控面板（druid-console）依赖 StatFilter 而非 PoolMetrics，但如果外部调用 `datasource.metrics()` 将获得过期数据。

**建议:** 在 `get_connection()` 和 `PoolGuard::drop()` 中同步更新 PoolMetrics 的 active/idle 计数，或废弃 PoolMetrics 中的 set_active/set_idle 方法。

#### 3.2 PSCache 未集成到 Statement 执行流程

**文件:** `druid-pool/src/datasource.rs:47,67,248` + `druid-pool/src/pscache.rs`

**问题:** `DruidDataSource` 创建了 `PSCache` 实例并暴露了 `pscache()` 访问器，但在 `PoolGuard` 中没有 prepared statement 缓存逻辑。用户开启 `pool_prepared_statements: true` 后，PSCache 实例存在但永远不会有条目被插入。

**影响:** 配置项 `pool_prepared_statements` 和 `max_pool_prepared_statement_per_connection_size` 形同虚设，用户期望的 PS 缓存功能不生效。

**建议:** 在连接使用时提供 PreparedStatement 执行方法，在 execute 前后调用 `pscache.get()` / `pscache.put()`，或者在未实现前将配置标记为 `#[deprecated]`。

### 🟡 中危 (Medium)

#### 3.3 substitute_params 参数注入风险

**文件:** `druid-util/src/string.rs:43-52`

**问题:** `substitute_params` 使用 `result.find('?')` 在循环中进行字符串替换，存在两个缺陷：

1. **二次替换:** 如果参数值中包含 `?`，下一次循环的 `find('?')` 可能匹配到已替换的参数值内部
2. **字面量污染:** 不区分 SQL 字符串字面量内的 `?` 和真实的占位符

```rust
// 示例：第二个参数值中的 ? 会被错误匹配
substitute_params("SELECT ? FROM t WHERE x = ?", &["col", "a?b"])
// 实际结果不确定：可能替换第一个 ? 后匹配到 "a?b" 中的 ?
```

**建议:** 使用一次性 token 替换而非循环 find；或解析 SQL AST 后再替换。

#### 3.4 format_select indent 参数未使用

**文件:** `druid-sql/src/format.rs:16`

**问题:** `format_select(stmt: &SelectStatement, _indent: usize)` 接受 `_indent` 参数但完全不使用。子查询嵌套时不会产生缩进格式化输出。

**建议:** 实现缩进逻辑，或移除此参数。

#### 3.5 druid-console 和 druid-core 测试覆盖为 0

**文件:** `druid-console/src/lib.rs` + `druid-core/src/types.rs`

**问题:** 两个面向用户的模块完全无测试：
- `druid-console`: HTTP 端点（stat_json、sql_json、slow_sql_json、index_page）无集成/单元测试
- `druid-core`: `DbType::parse()` 支持 30+ 种数据库别名映射，无测试验证

**建议:** 为 druid-console 添加 HTTP 端点测试（使用 axum::test），为 DbType::parse 添加别名映射测试。

#### 3.6 PoolGuard Drop 中连接关闭可能静默失败

**文件:** `druid-pool/src/datasource.rs:306-312`

**问题:** 当 PoolGuard drop 时 pool 已关闭，代码尝试在 runtime 上下文中关闭物理连接。但如果 `Handle::try_current()` 返回 Err（不在 tokio runtime 上下文中），连接直接丢弃而不调用 close。

```rust
if let Ok(handle) = tokio::runtime::Handle::try_current() {
    // 连接被关闭
} // else: 连接静默泄漏
```

**建议:** 在 else 分支中至少记录 warning 日志，或使用 `std::thread::spawn` + 新建 runtime 作为回退。

### 🟢 建议 (Low)

#### 3.7 手写 base64 实现

**文件:** `druid-util/src/crypto.rs:24-72`

保持在 v1.0.5 报告中的建议：使用 `base64` crate 替代手写实现。不存在已知 bug，但手写的编解码器维护成本高，且健壮性通常不如广泛使用的 crate。

#### 3.8 未使用的 workspace 依赖

**文件:** `Cargo.toml`

| 依赖 | 状态 |
|------|------|
| `parking_lot` | 已声明，druid-pool 中引用了 `parking_lot.workspace = true` 但未实际使用 |
| `askama` | 已声明未使用（druid-console 已手写 HTML + html_escape） |
| `anyhow` | 已声明未使用 |
| `figment` | 已声明未使用 |
| `metrics` | 已声明未使用 |
| `proptest` | 已声明未使用 |
| `once_cell` | 已声明未使用（Rust 1.80+ std 包含 LazyLock） |

**建议:** 移除未使用的依赖以减少编译时间和二进制体积。保留 `parking_lot` 如果计划后续用它替代 `std::sync::Mutex`。

#### 3.9 MySQL 方言深度实现缺失

**文件:** `druid-sql/src/parser/dialects/mysql.rs`

当前使用核心 Parser 处理所有方言，MySQL 方言文件仅作占位。生产环境中 MySQL 特有语法（反引号标识符、`LIMIT offset, count`、`ON DUPLICATE KEY UPDATE` 等）无法解析。

---

## 4. 架构评价

### 模块职责

| Crate | 定位 | 评价 |
|-------|------|------|
| druid-core | 类型、配置、错误定义 | ✅ 职责清晰 |
| druid-util | 工具函数 | ✅ 内聚性好 |
| druid-sql | SQL 解析/格式化 | ✅ 解析器与格式化分离合理 |
| druid-filter | Filter-Chain 骨架 | ✅ 接口设计优雅 |
| druid-wall | SQL 防火墙 | ✅ 检查逻辑清晰 |
| druid-stat | SQL 监控统计 | ✅ StatFilter 职责单一 |
| druid-pool | 连接池核心 | ⚠️ 指标同步需要完善 |
| druid-proxy | 代理层 | ✅ 薄封装，职责恰当 |
| druid-ha | 高可用 | ✅ 加权轮询实现正确 |
| druid-console | HTTP 监控面板 | ⚠️ 缺测试，但功能可用 |

### 设计亮点

1. **Filter-Chain 架构:** `Filter` trait 用默认空实现 + 单一职责 Filter（StatFilter、WallFilter）组合，与 Java Druid 设计理念一致，扩展性好
2. **SQL AST 完整性:** 18 种 SQLExpr 变体覆盖主流 SQL 语法，支持子查询、JOIN、窗口函数、CASE WHEN
3. **连接池并发模型:** Semaphore + Mutex 组合，限流和状态管理职责分离
4. **PoolGuard RAII:** Drop 自动归还连接，避免连接泄漏
5. **配置默认值:** 驱逐时间、最大连接数等默认值与 Java Druid 对齐

---

## 5. 优先修复路线

### 立即修复 (v1.0.7)
1. **PoolMetrics 同步** — 在 get_connection / PoolGuard::drop 中更新 active/idle 计数
2. **PSCache 接入或标记 deprecated** — 避免功能假象

### 下一版本 (v1.1.0)
3. 补充 druid-console 和 druid-core 测试
4. 修复 substitute_params 二次替换问题
5. 实现 MySQL 方言解析器（反引号、LIMIT offset,count 语法）
6. 清理未使用的 workspace 依赖

### 后续版本
7. 添加真实数据库 Driver 实现（sqlx 集成）
8. format_select 缩进支持
9. 替换手写 base64

---

## 6. 总结

| 维度 | 评级 | 说明 |
|------|------|------|
| 编译 | ✅ 优秀 | 0 warnings |
| Clippy | ✅ 优秀 | 0 warnings |
| 测试 | ⚠️ 中等 | 49 tests 全通过，核心路径覆盖好，但 console/core 无测试 |
| 安全性 | ✅ 良好 | XSS 已修复，Wall 关键词边界已修复 |
| 并发安全 | ⚠️ 需改进 | 主流程无竞态，但 PoolMetrics 指标不同步 |
| 功能完整度 | ⚠️ 部分缺失 | PSCache 未接入、MySQL 方言未实现 |
| 代码质量 | ✅ 良好 | v1.0.6 修复了上一轮的 12/14 个问题，架构清晰 |

项目 v1.0.6 较前一版有显著提升：14 个问题中修复了 12 个（86%），剩余的 PSCache 集成和 PoolMetrics 同步是结构性工作。编译和 lint 管线完全干净，49 个测试全部通过。核心架构（Filter-Chain、Wall、Stat、HA）设计合理，与 Java Druid 设计理念保持一致。
