# Druid-Rust 审查报告 (第四轮 + 全部修复)

**日期**: 2026-07-31 | **版本**: 1.1.7
**测试**: 61/61 通过 | 构建: 成功 | Clippy: 0 warnings | 格式化: 统一

---

## 快速概览

| 维度 | 状态 |
|------|------|
| 编译 | ✓ 零错误 |
| 测试 | ✓ 61 passed, 0 failed |
| Clippy | ✓ 零 warning |
| 文档 | ✓ 10 crate |
| 格式化 | ✓ cargo fmt 通过 |
| 安全 | ✓ 零 unsafe, 零 unwrap panic |

---

## 本轮修复 (4 项 + 2 限制)

### Bug 修复

| # | 问题 | 文件 | 修复 |
|---|------|------|------|
| 1 | `waiting_count` 泄漏 | `datasource.rs` | 4 个错误返回路径添加 `self.metrics.dec_waiting()` |
| 2 | `connection_closed` 同时减 active 和 idle | `stat/lib.rs` | 移除无条件的 active/idle 递减，仅记录 `destroy_count` |

### 已知限制修复

| # | 限制 | 文件 | 修复 |
|---|------|------|------|
| 3 | CTE/WITH 解析后丢弃 | `ast/expr.rs`, `parser/mod.rs`, `format.rs`, `visitor/schema.rs`, `checker.rs` | 新增 `CteDef` 结构体 + `SelectStatement.with_cte` 字段；parser 收集 CTE 定义；formatter 输出 WITH 子句；visitor/checker 递归访问 CTE 查询 |
| 4 | KeepAlive TOCTOU 竞态 | `datasource.rs` | 快照改为 `(id, last_used_at, conn)` 三元组；驱逐时检查 `last_used_at` 是否变化（若变化说明被 borrow 并归还过，不再驱逐） |

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
- PoolGuard 支持 `test_on_return` 异步验证防坏连接入池
- KeepAlive 驱逐使用 `(id, last_used_at)` 双条件防止误驱逐
- SQL 防火墙支持 CTE 递归检查

---

## 历轮修复统计

| 轮次 | 发现 | 已修复 | 遗留 |
|------|------|--------|------|
| 第一轮 | 15 | 15 | 0 |
| 第二轮 | 3 | 3 | 0 |
| 第三轮 | 16 | 14 | 2 |
| 第四轮 | 2 bugs + 2 限制 | 4 | 0 |
| **合计** | **38** | **36** | **0** |
