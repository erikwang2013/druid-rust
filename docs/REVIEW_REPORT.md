# Druid-Rust 审查报告

**日期**: 2026-07-31 — 全部已修复
**测试**: 61/61 通过 | 构建: 成功 | Clippy: 0 warnings

---

## 修复清单

### Clippy 警告 (3/3)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 1 | `druid-stat/src/metrics.rs:36` | `#[expect(dead_code)]` 无效 | 删除该属性 |
| 2 | `druid-sql/src/parser/lexer.rs:247` | 无意义 `return` | 移除 `return` |
| 3 | `druid-sql/src/parser/mod.rs:911` | `loop { if let }` | 改为 `while let` |

### Bug 修复 (3/3)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 1 | `druid-pool/src/datasource.rs:103-115` | 驱逐不尊重 min_idle | 增加 `evicted` 计数器，动态递减判断 |
| 2 | `druid-pool/src/pscache.rs:55` | 新条目 hit_count=0 被立即淘汰 | 初始化为 1 |
| 3 | `druid-sql/src/parser/mod.rs:963-990` | 纯分号输入空转 | 分号分支增加迭代计数器检查 |

### 设计/代码质量 (5/5)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 1 | `druid-util/src/crypto.rs` | 手动 base64 实现 | 替换为 `base64` crate v0.22 |
| 2 | `druid-sql/src/token.rs` + `parser/mod.rs` | Debug 格式化做语义输出 | 新增 `Token::as_type_name()` 方法 |
| 3 | `druid-sql/src/parser/mod.rs:468` | NOT 组合静默忽略 | 添加 `tracing::warn!` |
| 4 | `druid-console/src/lib.rs:6-12` | HTML 转义缺 `/` | 添加 `/` → `&#x2F;` 转义 |
| 5 | `druid-sql/src/token.rs:95` | Token::Numeric 死代码 | 删除该变体 |

### 依赖变更

- `druid-util`: 新增 `base64 = "0.22"` 依赖

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
