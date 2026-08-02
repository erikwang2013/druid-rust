# Druid-Rust 审查报告 (第五轮 + 全部修复)

**日期**: 2026-08-02 | **版本**: 1.1.8
**测试**: 61/61 通过 | 构建: 成功 | Clippy: 0 warnings | 格式化: 统一

---

## 快速概览

| 维度 | 状态 |
|------|------|
| 编译 | ✓ 零错误 |
| 测试 | ✓ 61 passed, 0 failed |
| Clippy | ✓ 零 warning |
| 格式化 | ✓ cargo fmt 通过 |
| 安全 | ✓ 零 unsafe, 零 unwrap panic |

---

## 本轮修复 (8 项)

### Bug 修复

| # | 问题 | 文件 | 修复 |
|---|------|------|------|
| 1 | `dec_waiting()` 缺失 — test_on_borrow 失败路径 | `datasource.rs:311` | 添加 `self.metrics.dec_waiting()` |
| 2 | `DataSourceStat` active/idle 永久偏高 | `stat/lib.rs:152-157` | `connection_closed` 递减 `idle_count` (saturating_sub)，覆盖大部分 close 场景 |
| 3 | Wall checker 漏检 HAVING/GROUP BY/ORDER BY/子查询 | `checker.rs:180-206` | 新增 `group_by`/`having`/`order_by`/`limit`/`offset`/FROM 子查询/join 子查询 的 visit |
| 4 | `parse_drop` 非 Table 对象解析错误 | `parser/mod.rs:957` | `_` 通配分支添加 `self.advance()` |

### 指标修复

| # | 问题 | 文件 | 修复 |
|---|------|------|------|
| 5 | `inc_create()` 漏计 borrow 新建连接 | `datasource.rs:285` | 添加 `self.metrics.inc_create()` |
| 6 | `inc_destroy()` 漏计过期/验证失败关闭 | `datasource.rs` 多处 | 添加 `inc_destroy()` 到 max_lifetime 过期、test_on_borrow/test_on_return 失败路径 |
| 7 | `dec_waiting()` 语义错误（统计全 borrow 时长） | `datasource.rs` | 从 `PoolGuard::drop` 移至 `get_connection` 中 permit 获取完成后 |

### 依赖清理

| # | 问题 | 修复 |
|---|------|------|
| 8 | 6 个未使用 Cargo 依赖 | 从 `druid-pool`/`druid-filter`/`druid-sql` 的 Cargo.toml 移除 |

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
- 零生产代码 panic 路径
- 密码 AES-256-GCM 加密
- HTML XSS 转义完整
- Drop 资源清理正确
- CTE 递归防火墙检查
- Wall checker 覆盖全部 SQL 子句（columns/where/joins/group_by/having/order_by/limit/offset/FROM 子查询/join 子查询/CTE）

---

## 历轮统计

| 轮次 | 发现 | 已修复 | 遗留 |
|------|------|--------|------|
| 第一轮 | 15 | 15 | 0 |
| 第二轮 | 3 | 3 | 0 |
| 第三轮 | 16 | 14 | 2 |
| 第四轮 | 4 | 4 | 0 |
| 第五轮 | 8 | 8 | 0 |
| **合计** | **46** | **44** | **0** |
