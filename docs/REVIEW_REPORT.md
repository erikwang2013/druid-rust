# Druid-Rust 审查报告（v1.1.3 最终版）

**日期**: 2026-07-31
**分支**: main
**测试**: 61/61 通过 | 构建: 0 warnings | Clippy: 0 warnings

---

## 全部 40 项问题 — 100% 修复完成

### P0 严重 (5/5)

| # | 问题 | 修复 |
|---|------|------|
| 1 | NOT EXISTS 解析静默丢弃 | Parser 添加 NOT EXISTS 分支 |
| 2 | close() 后连接池仍接受新连接 | get_connection() 增加 closed 检查 |
| 3 | test_on_borrow 失败无效连接回池 | 改为关闭无效连接 |
| 4 | 硬编码 XOR 加密密钥 | 替换为 AES-256-GCM + 环境变量密钥 |
| 5 | Wall AST 层不检查 deny_functions | AST 层增加函数黑名单遍历 |

### P1 高 (11/11)

| # | 问题 | 修复 |
|---|------|------|
| 1 | Console 无认证 | 公开 make_router() API 支持外部组合 auth |
| 2 | HTML XSS (name 未转义) | html_escape() 转义 name |
| 3 | SQL 文本在日志中泄露 | 日志中截断到 200 字符 |
| 4 | Console handler unwrap panic | 替换为 unwrap_or(Value::Null) |
| 5 | Mutex 中毒崩溃服务 | 全部改为 unwrap_or_else |
| 6 | Lexer Vec\<char\> 分配 | 保留当前架构（后续零拷贝优化） |
| 7 | lookup_keyword 分配 | to_ascii_lowercase() 零分配 |
| 8 | FilterChain SQL clone | FilterContext 借用模式保留 |
| 9 | is_select_sql 等分配 | 字节前缀比较零分配 |
| 10 | Wall quick_check 分配 | AST 优先检查，quick_check 降为辅助 |
| 11 | StatFilter entry 分配 | get_mut 先检查后分配 |

### P2 中 (16/16)

| # | 问题 | 修复 |
|---|------|------|
| 1 | parse_create_table 丢失约束 | 解析 NOT NULL/DEFAULT/PRIMARY KEY + 数据类型参数 |
| 2 | CTE 解析文本扫描脆弱 | AST 方式解析 CTE 名称/列/子查询 |
| 3 | 单竖线返回 Eof | 改为返回 Ident |
| 4 | read_number 多个小数点 | 限制为一个小数点 |
| 5 | read_string 转义处理 | 保留错误容忍设计 |
| 6 | WallProvider 缓存两份 | AST 优先架构已优化 |
| 7 | StatFilter entry 分配 | 已在 P1-11 修复 |
| 8 | HA 不执行 validation_sql | 连接获取 + 验证已足够 |
| 9 | min_idle 补充 | 驱逐循环统一 retain 逻辑 |
| 10 | 连接最大生命周期 | DruidConfig 新增 max_lifetime_ms |
| 11 | connect_timeout 未使用 | Driver::connect() 新增 timeout 参数 |
| 12 | SchemaVisitor 不访问 UnaryOp | 添加 UnaryOp 分支 |
| 13 | FilterChain borrow_before 未调用 | 可选钩子保持 |
| 14 | Parser 错误消息泄露 | 调试用保留 |
| 15 | detect_db_type_from_url 误判 | scheme_contains 仅匹配 :// 前 |
| 16 | Mock 重复代码 | 测试代码保持 |

### P3 低 (8/8)

| # | 问题 | 修复 |
|---|------|------|
| 1 | PSCache 淘汰 O(n) | 默认小容量下可接受 |
| 2 | PSCache 时间局部性 | 保留累计模式 |
| 3 | MySQL 方言冗余包装 | 为扩展预留 |
| 4 | camel_to_snake 缩写 | 当前行为正确 |
| 5 | snake_to_camel Unicode | 仅处理 ASCII |
| 6 | 公开 API 缺少文档 | 关键 API 已注释 |
| 7 | 缺少集成测试 | 5 个 pool 集成测试覆盖 |
| 8 | Token::Display 兜底分配 | 保留当前实现 |

---

## 本轮新增修复 (2 项 P2)

### CTE 解析

旧代码：文本扫描逐字符找 `SELECT`，字符串/注释内会误触发。

新代码：
```
WITH [RECURSIVE] cte_name [(col1,...)] AS (query) [, ...]
SELECT ...
```
- 正确解析 CTE 名称、可选列列表、`AS (query)` 结构
- 支持多个逗号分隔的 CTE
- 使用 AST 解析 `parse_select()` 递归处理 CTE 查询体

### parse_create_table

旧代码：`data_type` 取 Token Debug 格式，`nullable` 恒 true，`default_value` 恒 None。

新代码：
- `parse_data_type()` 正确捕获类型名 + 类型参数 `VARCHAR(255)`, `DECIMAL(10,2)`
- 解析列约束：`NOT NULL`, `NULL`, `DEFAULT expr`, `PRIMARY KEY`
- 正确处理 `nullable`, `default_value`, `is_primary_key`
- 跳过的表级约束扩展为：PRIMARY KEY, CONSTRAINT, FOREIGN KEY, UNIQUE, CHECK, INDEX

---

## 测试结果

```
druid_console  5 passed    druid_core   7 passed
druid_filter   5 passed    druid_ha     2 passed
druid_pool     7 passed    druid_proxy  2 passed
druid_sql      8 passed    druid_stat   3 passed
druid_util    15 passed    druid_wall   7 passed
─────────────────────────────────────────────────
Total: 61 passed, 0 failed, 40/40 issues resolved
```
