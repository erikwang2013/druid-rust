# Druid Rust 单元测试报告

- 日期：2026-09-26
- 版本：1.2.0
- 命令：`cargo test --workspace`
- 结果：**231 通过 / 0 失败 / 0 忽略**

## 各模块统计

| 模块 | 测试数 | 覆盖重点 |
|------|-------:|----------|
| druid-console | 16 | 端点 JSON 结构、404、XSS 转义、跨 crate serde round-trip、宠物 SVG 路由与页头内联 |
| druid-core | 23 | 配置解析、错误类型、类型序列化 |
| druid-filter | 18 | 链调用顺序、execute_before 短路、init 失败中断、空链 |
| druid-ha | 8 | 加权轮询、零权重、健康检查状态机 |
| druid-pool | 23（含集成 5） | 池借还、生命周期过期、驱逐、保活、校验失败路径、PS 缓存命中/淘汰 |
| druid-proxy | 7 | close 幂等、drop 关闭、filter 回调 |
| druid-sql | 53 | 词法边界、合法/非法解析（15 种错误路径）、AST、格式化 roundtrip、visitor |
| druid-stat | 14 | 指标增减/下溢、SQL 错误统计 |
| druid-util | 35 | 加解密、SQL 检测、字符串、时间工具 |
| druid-wall | 34 | SQL 防火墙规则（字面量、词边界、子查询、INTO OUTFILE、超长 SQL） |

## 1.2.0 新增测试（2 项）

| 模块 | 测试 | 断言 |
|------|------|------|
| druid-console | `test_mascot_svg_endpoint` | `/druid/mascot.svg` 返回 200 + `image/svg+xml`，正文以 `<svg` 开头且含宠物标题 |
| druid-console | `test_index_embeds_mascot` | 页头内联宠物 SVG（`<div class="hdr"><svg`）、favicon 指向 `/druid/mascot.svg`、页面仅一个 `<svg>` 根 |

## 发现并修复的问题

以下 14 项由 v1.1.9 的测试补全工作发现并修复。

| 模块 | 问题 | 修复 |
|------|------|------|
| druid-util/sql.rs | `is_select_sql` 对多字节 UTF-8 输入字节切片 panic | 改用 `get(..n).is_some_and` |
| druid-core/config.rs | `password` 反序列化无默认值，round-trip 失败 | 加 `#[serde(default)]` |
| druid-sql/lexer | `''` 双写引号转义不支持，`'it''s'` 被错误拆分 | 支持双写转义 |
| druid-sql/parser | `SELECT "col"` QuotedIdent 被拒 | 接受 QuotedIdent |
| druid-sql/parser | 裸引号别名（`SELECT 1 "a"`）不识别 | 支持裸引号别名 |
| druid-sql/parser | `IN (SELECT ...)` / `NOT IN` 解析失败（InSubQuery 死代码） | 接通 InSubQuery 路径 |
| druid-sql/parser | `SELECT t.*` 解析失败（Wildcard(Some) 不可达） | 中间标识符还原 |
| druid-sql/visitor | SchemaVisitor 漏访问 JOIN ON 表达式，ON 列丢失 | 遍历 ON 条件 |
| druid-wall/checker | 字符串字面量内关键字（`y='not drop here'`）误判拦截 | 剔除单引号字面量 |
| druid-wall/checker | `SLEEP (1)` 带空格绕过函数检测 | 空白压缩后匹配 |
| druid-ha | weight=0 节点导致 total_weight=0 取模除零 panic | `weight.max(1)` |
| druid-pool | test_on_borrow 校验失败的新建连接未 close（泄漏） | 补异步 close |
| druid-pool/pscache | max_size=0（未开启）时 put 仍写入 | 直接返回 |
| druid-stat | dec_waiting 下溢回绕 u64::MAX；statement_error 缺条目不创建 | saturating 减；缺条目时创建 |

另修复：druid-sql/src/parser/lexer.rs:361 历史断行（缺 `assert_eq!(`，导致编译失败）。

## 跳过项

- 真实 DB/网络连接：以内存 Mock 驱动替代（datasource 的 Mock 驱动）
- SQL 防火墙的真实驱动行为、spawn_health_check_loop 无限循环（以单轮 run_health_check 验证状态机）
- `ROW_NUMBER() OVER (...)`、`COUNT(DISTINCT x)`、`!=`：解析器既有设计不支持，测试按现状断言
- WallConfig 的 serde 反序列化：druid-wall 无 serde_json 依赖（未加依赖），由 druid-console 跨 crate 测试覆盖同型配置

## 运行方式

```bash
cargo test --workspace     # 全部
cargo test -p druid-sql    # 单模块
```
