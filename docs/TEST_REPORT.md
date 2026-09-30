# Druid Rust 单元测试报告

- 日期：2026-10-01
- 版本：1.3.0
- 命令：`cargo test --workspace`（分模块数字另以 `cargo test -p <crate>` 逐个核对）
- 结果：**423 通过 / 0 失败 / 2 忽略**（取数时间：2026-10-01 03:20 CST）

> 1.3.1 为「深度审查 + 三轮对抗验证」后的修复版，测试总数 231 → 423。
> 2 条 `#[ignore]` 均为**已声明限制的金丝雀**（固化当前行为，策略变化时报警），非未修复缺陷：
> `druid-wall/tests/adversarial.rs` 的一条（parser 覆盖缺口 + `USE`/`SET` 刻意拒绝）与
> `druid-pool/src/datasource.rs` 文档测试中的 `ignore` 代码块。
> 新增 11 个测试文件（另有 `druid-pool/tests/common/mod.rs` 共享 Mock 驱动），
> 见下方「1.3.0 新增测试套件」。1.2.0 及更早的数字见文末历史小节，保留原样。

## 各模块统计

| 模块 | 测试数 | 覆盖重点 |
|------|-------:|----------|
| druid-console | 22 | 端点 JSON 结构、404、XSS 转义、跨 crate serde round-trip、宠物 SVG 路由与页头内联、Bearer 鉴权与 `no-store`、分页 |
| druid-core | 44 | 配置解析与 `validate()`、错误类型、类型序列化、Debug 脱敏（redact）对抗用例 |
| druid-filter | 20 | 链调用顺序、execute_before 短路、init 失败中断、空链、补挂的五个钩子 dispatch |
| druid-ha | 12 | 加权轮询、零权重、健康检查状态机（失败/成功阈值、退避、单点超时、取消安全、shutdown） |
| druid-pool | 83（单元 7 + 集成/套件 76） | 池借还、生命周期过期（按 `created_at`）、驱逐、保活与借用互斥、校验失败路径、取消安全、并发竞态、对抗验证、PS 缓存容器 |
| druid-proxy | 9 | close 幂等、drop 关闭、filter 回调 |
| druid-sql | 86 | 词法边界、合法/非法解析、AST、格式化 roundtrip、visitor、对抗验证 |
| druid-stat | 22 | 指标增减/下溢、SQL 错误统计、统计表容量上限与淘汰、截断（多字节安全） |
| druid-util | 57 | 加解密、SQL 检测与 MySQL 转义、字符串、时间工具、Debug 脱敏 |
| druid-wall | 59（+1 忽略） | 防火墙规则、默认 fail-closed、结构化关键字匹配、绕过封堵对抗用例 |

## 1.3.0 新增测试套件

| 位置 | 用例 | 覆盖重点 |
|------|-----:|----------|
| `druid-pool/tests/adversarial_verify.rs` | 41 | 取消安全（permit 等待 / connect / 复用校验三处）、close 与交付竞态、计数守恒（active/idle/waiting 单锁快照一致）、StmtScope 钩子恰好闭合一次（含 panic 展开）、KeepAlive 与借用窗口互斥、DataSource Drop 回收、`with_filters` 接线 |
| `druid-pool/tests/pool_test.rs` | 19 | 初始化/关闭边界、borrow-return 周期与指标、`max_lifetime` 按 `created_at`、borrow/return 校验失败处置、驱逐与保活、非法配置在 init 拒绝 |
| `druid-pool/tests/pool_race_test.rs` | 11 | Filter 钩子顺序、WallFilter 端到端拦截（驱动之前）、并发压力下不双借且计数守恒、取消后计数恢复 |
| `druid-wall/tests/adversarial.rs` | 26 + 1 忽略 | fail-open 封堵、函数参数递归、`--` 注释变体与可执行注释、`INTO OUTFILE`、大小写/Unicode 规避、畸形输入不 panic；1 项金丝雀 `#[ignore]` 固化已声明限制 |
| `druid-wall/tests/wall_rules.rs` | 19 | `deny_operations` 逐项可达、`deny_unparsable` 开关、引号致盲三种形态（`\'` / `"` / 反引号）、`CREATE TABLE ... DEFAULT` 默认值、事务控制白通道、多语句策略 |
| `druid-sql/tests/adversarial_verify.rs` | 12 | 字面量/标识符/运算符优先级 roundtrip 语义保持、标识符紧邻引号不可被再词法化、表级约束现状固化 |
| `druid-util/tests/adversarial_verify.rs` | 14 | MySQL 转义（反斜杠、跨参数逃逸、NO_BACKSLASH_ESCAPES 分歧）、`substitute_params` 的 deprecated 可注入性留证、Debug 脱敏、配置边界校验 |
| `druid-core/tests/redact_adversarial.rs` | 9 | 4 组明文口令泄漏的回归（URL userinfo / query 参数 / connection_properties / 未建模键），首轮 `#[ignore]` 留证转正 |
| `druid-{console,ha,stat}/src/tests.rs` | 见上表 | 单元测试从 `lib.rs` 拆分至独立文件 |

## 1.2.0 新增测试（2 项）

| 模块 | 测试 | 断言 |
|------|------|------|
| druid-console | `test_mascot_svg_endpoint` | `/druid/mascot.svg` 返回 200 + `image/svg+xml`，正文以 `<svg` 开头且含宠物标题 |
| druid-console | `test_index_embeds_mascot` | 页头内联宠物 SVG（`<div class="hdr"><svg`）、favicon 指向 `/druid/mascot.svg`、页面仅一个 `<svg>` 根 |

## 发现并修复的问题

**1.3.0**：11 项在「全绿」状态下未被发现的缺陷（3 个防火墙绕过、2 个资源泄漏、2 个并发缺陷、4 组明文口令泄漏）。
完整清单见 [CHANGELOG.md](../CHANGELOG.md) 的 `[1.3.0]` 条目「Fixed」小节，此处不再复制。

**1.2.0 及更早**：以下 14 项由 v1.1.9 的测试补全工作发现并修复。

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

- 真实 DB/网络连接：以内存 Mock 驱动替代（现由 `druid-pool/tests/common/mod.rs` 共享）
- SQL 防火墙的真实驱动行为已由 `druid-wall/tests/adversarial.rs` / `druid-pool/tests/pool_race_test.rs` 以「数据源 → Filter 链 → 驱动」真实路径覆盖；`spawn_health_check_loop` 现提供 `shutdown()` 并有专门用例（`test_health_check_loop_shutdown`），其余健康检查状态机仍以单轮 `run_health_check` 验证
- `ROW_NUMBER() OVER (...)`、`COUNT(DISTINCT x)`（`COUNT` 为专用 token，其解析分支不处理 `DISTINCT`）、`!=`：解析器既有设计不支持，测试按现状断言（已实测确认）
- `WallConfig` 的 serde 反序列化：`druid-wall` 无 serde_json 依赖（仅有 `serde` derive），未直接测试
- `druid-pool/src/datasource.rs` 的 `metrics_arc` 文档示例标注 `ignore`（需完整数据源上下文），计 1 项 doc-test 忽略
- `druid-wall/tests/adversarial.rs` 中「已声明的已知限制」金丝雀为 `#[ignore]`：固化「parser 覆盖缺口 + USE/SET 刻意拒绝」的当前行为，若其转绿说明声明需同步更新

## 运行方式

```bash
cargo test --workspace     # 全部
cargo test -p druid-sql    # 单模块
cargo test -p druid-pool --test adversarial_verify   # 单个对抗套件
```
