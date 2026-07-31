# Druid-Rust v1.0.5 代码审查报告

**日期:** 2026-07-31
**分支:** main
**审查范围:** 全部 10 个 crate，30 个 .rs 源文件

---

## 1. 验证结果

| 检查项 | 状态 | 详情 |
|--------|------|------|
| `cargo check` | ✅ 通过 | 零警告 |
| `cargo test` | ✅ 49/49 通过 | 所有测试通过 |
| `cargo clippy --all-targets` | ✅ 通过 | 零警告 |
| `cargo build` | ✅ 通过 | 零警告 |

### 测试覆盖统计

| Crate | 测试数 | 覆盖范围 |
|-------|--------|----------|
| druid-core | 0 | 类型定义，通过其他 crate 间接覆盖 |
| druid-filter | 5 | FilterChain, FilterAdapter, FilterManager |
| druid-util | 15 | 加密、SQL 工具、字符串处理、时间工具 |
| druid-sql | 8 | 词法分析、SQL 解析、格式化、Schema 访问 |
| druid-stat | 3 | SQL 统计收集、慢 SQL 检测、数据源统计 |
| druid-wall | 7 | SQL 防火墙检查、缓存、禁用模式 |
| druid-pool | 7 (2 单元 + 5 集成) | 连接池初始化、借用归还、并发限制 |
| druid-proxy | 2 | 代理连接执行、关闭 |
| druid-ha | 2 | 高可用轮询、故障标记 |
| druid-console | 0 | HTTP 监控端点 |
| **总计** | **49** | |

---

## 2. 新发现问题

### 🔴 严重 (Critical)

#### 2.1 XSS 漏洞 — druid-console HTML 输出未转义

**文件:** `druid-console/src/lib.rs:51-61`
**风险:** SQL 文本通过 `format!` 直接嵌入 HTML。如果 SQL 中包含 `<script>` 等标签，将导致 XSS 攻击。

```rust
// 当前代码（未转义）
rows.push_str(&format!(
    "<tr><td>{}</td><td>{}</td>...",
    s.sql,  // ← 直接嵌入 HTML
));
```

**建议:** 对 SQL 文本做 HTML 实体转义（`<` → `&lt;`, `>` → `&gt;`, `&` → `&amp;`, `"` → `&quot;`, `'` → `&#x27;`），或使用 `askama` 模板引擎（已在 workspace 依赖中声明）。

#### 2.2 竞态条件 — get_connection 双重加锁

**文件:** `druid-pool/src/datasource.rs:156-162`
**风险:** `self.inner.lock().unwrap()` 在同一个 `if let` 的两个分支中被多次调用，释放锁的间隙其他线程可能修改 pool 状态。

```rust
let idle_entry = self.inner.lock().unwrap().idle.pop_front();  // 第一次加锁（释放）
let (conn_id, conn) = if let Some(e) = idle_entry {
    self.inner.lock().unwrap().active_count += 1;  // 第二次加锁（存在间隙）
    (e.id, e.conn)
} else {
    self.inner.lock().unwrap().active_count += 1;  // 同上
    ...
};
```

**建议:** 将 pop 和 active_count inc 合并到同一次加锁中完成。

### 🟠 高危 (High)

#### 2.3 truncate_sql 多字节字符 panic

**文件:** `druid-util/src/string.rs:61-67`
**风险:** `&sql[..max_len]` 按字节索引切片，在中文等多字节 UTF-8 字符边界处会 panic。

```rust
format!("{}...", &sql[..max_len])  // max_len 可能在多字节字符中间
```

**建议:** 使用 `sql.char_indices()` 找到安全的字符边界，或 `sql.chars().take(max_len).collect::<String>()`。

#### 2.4 PSCache 定义但未集成

**文件:** `druid-pool/src/pscache.rs` + `druid-core/src/config.rs:58-61`
**风险:** `pool_prepared_statements` 和 `max_pool_prepared_statement_per_connection_size` 配置字段已定义，`PSCache` 模块已实现，但 `DruidDataSource` 从未使用 PSCache。用户开启该配置后无实际效果。

#### 2.5 PoolMetrics 定义但未使用

**文件:** `druid-stat/src/metrics.rs`
**风险:** `PoolMetrics` 用 `AtomicU64` 实现了高性能无锁指标采集，但未被 `DruidDataSource` 使用。当前数据源使用 `Mutex<PoolInner>` 方式内部计数。

**建议:** 在 `DruidDataSource` 中集成 `PoolMetrics`，或移除该文件避免死代码。

#### 2.6 SQL 解析器缺失 NOT LIKE / NOT BETWEEN / NOT IN

**文件:** `druid-sql/src/parser/mod.rs:376-378, 402-412, 415-432`
**风险:** `NOT LIKE` 完全未实现（代码注释 "// 可能的 NOT LIKE" 标注了缺口），`NOT BETWEEN` 和 `NOT IN` 的 `not` 字段始终为 `false`。

```rust
// NOT BETWEEN 始终 hardcode not: false（line 411）
left = SQLExpr::Between { ..., not: false };
// NOT IN 同理（line 431）
left = SQLExpr::InList { ..., not: false };
```

**建议:** 在 comparison 解析中检查 `Token::Not` 前缀，正确设置 `not: true`。

#### 2.7 druid-ha 创建多余 Tokio Runtime

**文件:** `druid-ha/src/lib.rs:174-186`
**风险:** `spawn_health_check_loop` 使用 `std::thread::spawn` + 新建 `tokio::runtime::Runtime` 来运行异步健康检查，导致额外线程和 runtime 开销。

```rust
std::thread::spawn(move || {
    let rt = tokio::runtime::Runtime::new().unwrap();  // 不必要的 runtime
    rt.block_on(async move { ... });
});
```

**建议:** 使用 `tokio::spawn` 代替，或接受 `Handle` 参数复用现有 runtime。

### 🟡 中危 (Medium)

#### 2.8 SQL Formatter 有未格式化变体

**文件:** `druid-sql/src/format.rs:275`
**风险:** `format_expr` 的 catch-all 分支 `_ => format!("<{:?}>", expr)` 会使 `InSubQuery`、`Cast`、`WindowFunction` 等变体输出 debug 格式而非合法 SQL。

**建议:** 为所有 `SQLExpr` 变体添加格式化分支。

#### 2.9 PoolGuard Drop 中 tokio::spawn 风险

**文件:** `druid-pool/src/datasource.rs:278-281`
**风险:** Drop 中 `tokio::spawn` 在 runtime 已关闭时可能 panic 或静默失败，导致连接泄漏。

**建议:** 检测 runtime 上下文，或使用同步回退方案关闭连接。

#### 2.10 camel_to_snake 处理连续大写字母错误

**文件:** `druid-util/src/string.rs:4-16`
**风险:** `camel_to_snake("URL")` 输出 `"u_r_l"`，正确输出应为 `"url"`。连续大写字母通常视为一个缩写词。

#### 2.11 PSCache::put 重复存储问题

**文件:** `druid-pool/src/pscache.rs:34-50`
**风险:** `put` 先执行容量检查和淘汰，再调用 `or_insert`。如果 SQL 已存在，淘汰操作不必要。

**建议:** 先用 `contains_key` 检查，或使用 `Entry` API 避免不必要的淘汰。

#### 2.12 WallProvider::quick_check 关键词匹配边界问题

**文件:** `druid-wall/src/checker.rs:110-121`
**风险:** 关键词匹配基于空格分隔的简单字符串包含检查，可能漏掉紧跟标点符号或换行的情况。

### 🟢 建议 (Low)

#### 2.13 FilterChain 中 stmt_id 参数未使用

**文件:** `druid-filter/src/chain.rs:91-106`
`statement_execute_before` 和 `statement_execute_after` 接受 `stmt_id: u64` 参数但被 `let _ = stmt_id;` 忽略。

#### 2.14 parse_sql 潜在无限循环

**文件:** `druid-sql/src/parser/mod.rs:818-842`
如果 `parse_statement()` 返回 Ok 但未推进任何 token，while 循环将无限执行。应加最大迭代次数保护。

#### 2.15 驱逐时间默认值为 0

**文件:** `druid-core/src/config.rs:34-38`
`min_evictable_idle_time_ms` 和 `max_evictable_idle_time_ms` 默认为 0，与 Java Druid 默认值（min=30min, max=7h）不一致，可能导致连接被过早驱逐。

#### 2.16 手写 base64 实现

**文件:** `druid-util/src/crypto.rs:24-72`
建议使用 `base64` crate 减少维护负担和潜在 bug。

---

## 3. 依赖审计

workspace `Cargo.toml` 中声明的以下依赖未被任何 crate 使用：

| 依赖 | 状态 |
|------|------|
| `parking_lot` | 已声明未使用（可用它优化连接池锁） |
| `askama` | 已声明未使用（druid-console 手写 HTML） |
| `anyhow` | 已声明未使用 |
| `figment` | 已声明未使用 |
| `metrics` | 已声明未使用 |
| `proptest` | 已声明未使用 |
| `once_cell` | Rust 1.80+ 标准库已包含 `LazyLock` |

---

## 4. 优先修复路线

### 立即修复 (v1.0.6)
1. **XSS 漏洞** — druid-console HTML 转义
2. **竞态条件** — get_connection 合并加锁
3. **UTF-8 panic** — truncate_sql 字符边界

### 下一版本 (v1.1.0)
4. 集成 PSCache 到 DruidDataSource
5. 集成 PoolMetrics 到 DruidDataSource
6. 实现 NOT LIKE / NOT BETWEEN / NOT IN 解析
7. 完善 SQL Formatter 所有变体

### 后续版本
8. druid-ha 使用 tokio::spawn 替代 thread::spawn
9. 完善 MySQL 方言解析器
10. 添加真实数据库 Driver 实现

---

## 5. 总结

| 维度 | 评级 | 说明 |
|------|------|------|
| 编译 | ✅ 通过 | 0 warnings |
| Clippy | ✅ 通过 | 0 warnings |
| 测试 | ⚠️ 中等 | 49 tests，核心路径覆盖，缺边界和错误路径 |
| 安全性 | ⚠️ 1 个 XSS | druid-console 未转义 |
| 并发安全 | ⚠️ 1 个竞态 | get_connection 双重加锁 |
| 功能完整度 | ⚠️ 部分未集成 | PSCache、PoolMetrics、MySQL 方言 |
| 代码质量 | ✅ 良好 | 架构清晰，模块职责分明 |

项目整体质量良好，49 个测试全部通过，编译和 lint 管线干净。核心架构（Filter-Chain、Wall、Stat、HA）设计合理，与 Java Druid 设计理念保持一致。主要待完善的是功能集成度和边界情况处理。
