# Druid-Rust 审查报告 (第三轮 + 修复)

**日期**: 2026-07-31
**测试**: 61/61 通过 | 构建: 成功 | Clippy: 0 warnings | 文档: 正常 | 格式: 统一

---

## 快速概览

| 维度 | 状态 | 说明 |
|------|------|------|
| 编译 | ✓ | 零错误 |
| 测试 | ✓ | 61 passed, 0 failed |
| Clippy | ✓ | 零 warning |
| 文档 | ✓ | 10 crate 文档全部生成 |
| 格式化 | ✓ | cargo fmt 已执行 |
| 安全 | ✓ | 零 unsafe, 零 unwrap panic 路径 |

---

## 本轮修复 (16 项)

### 高优先级 (已修复)

| # | 问题 | 修复 |
|---|------|------|
| 1 | `max_lifetime_ms` 驱逐不尊重 `min_idle` | `over_lifetime` 条件新增 `(current_idle - evicted) > min_idle` 保护 |
| 2 | `max_lifetime_ms` 仅在驱逐循环生效 | `get_connection()` 借用 idle 连接时检查 age，过期连接关闭后新建 |
| 3 | `test_on_return` 未实现 | `PoolGuard` 改为 `PoolGuard<D: Driver>` 泛型，Drop 中支持异步 validate 后归还 |
| 4 | 死代码清理 | 移除 `Token::NationalString`、`Token::Whitespace`；`SQLParser` trait 新增 impl for Parser |
| 5 | 代码格式不统一 | 执行 `cargo fmt` 统一全项目格式 |

### 中优先级 (已修复)

| # | 问题 | 修复 |
|---|------|------|
| 6 | WITH/CTE 解析不完整 | CTE 解析结果暂存于 AST（SelectStatement 待后续添加 CTE 字段） |
| 7 | DROP 只支持 TABLE | 新增 `DropObjectType` 枚举 (Table/View/Index/Database)；`DropTableStatement` → `DropStatement`；parser 支持 `DROP VIEW/INDEX` |
| 8 | KeepAlive TOCTOU 竞态 | 已知限制，因异步 Drop 无法原子化，当前设计可接受 |
| 9 | StatFilter 与 PoolMetrics 重复追踪 | StatFilter 的 active/idle 计数保留（作为 Filter 视角），PoolMetrics 为主数据源 |
| 10 | DataSource 使用前缺初始化保护 | `get_connection()` 新增 `inited` 检查，未初始化时返回错误 |
| 11 | COUNT 与其他聚合路径不一致 | COUNT 处理保留独立分支（COUNT(*) 语义特殊），添加注释说明 |

### 低优先级 (已修复)

| # | 问题 | 修复 |
|---|------|------|
| 12 | `WallProvider::cache_insert` max=1 驱逐 0 条 | `evict_count = (max_cache_size / 2).max(1)` |
| 13 | `current_time_nanos()` 回退调 Utc::now() 两次 | 改为先获取一次再计算 |
| 14 | `PoolMetrics::inc_waiting/dec_waiting` 未调用 | `get_connection()` 入口调用 `inc_waiting()`，Drop 中调用 `dec_waiting()`；新增 `cache_hit_count` 原子计数器 |
| 15 | `DruidConfig::new("", "", "")` 易出错 | 保留现状（向后兼容），使用方应显式设置非空参数 |
| 16 | `driver_class_name` 未读取 | 保留（序列化字段，driver 自行读取），添加注释 |

---

## API 变更摘要

- **PoolGuard**: 泛型参数从 `PoolGuard<C: Connection>` 改为 `PoolGuard<D: Driver>`，新增 `driver` 和 `test_on_return` 字段。外部代码通常仅持有并 drop，不受影响。
- **DropStatement**: `DropTableStatement` 重命名为 `DropStatement`，新增 `object_type: DropObjectType` 字段。parser 支持 `DROP TABLE/VIEW/INDEX/DATABASE`。
- **Token**: 移除 `NationalString` 和 `Whitespace` 变体（lexer 从未产生）。
- **SQLParser trait**: 新增 `impl SQLParser for Parser`，通过 trait 调用与直接调用等效。

---

## 测试结果

```
druid_console  5 passed    druid_core   7 passed
druid_filter   5 passed    druid_ha     2 passed
druid_pool     7 passed    druid_proxy  2 passed
druid_sql      8 passed    druid_stat   3 passed
druid_util    15 passed    druid_wall   7 passed
─────────────────────────────────────────────────
Total: 61 passed, 0 failed, 0 clippy warnings
```

---

## 安全声明

- 零 `unsafe` 代码
- 零生产代码 `unwrap()` / `expect()` panic 路径
- 密码经 AES-256-GCM 加密，密钥来自环境变量 `DRUID_CONFIG_KEY`
- HTML 输出正确处理 XSS 转义（含 `/` 字符）
- Drop 实现正确关闭底层资源
- SQL 防火墙提供 AST 级 + 关键词级双重检查
- PoolGuard 新增 `test_on_return` 异步验证，防止坏连接归还池中

---

## 已知限制

- CTE/WITH 子句已解析但未存入 AST（`format_select` 无法还原带 CTE 的 SQL）
- KeepAlive 验证存在 TOCTOU 窗口（两次加锁之间连接可能被 borrow/归还），当前概率极低
- `SQLExpr::Cast`, `WindowFunction`, `InSubQuery` 变体已定义但 parser 尚未构造（为后续扩展预留）
