# Changelog

All notable changes to Druid-Rust.

## [1.3.1] - 2026-10-01

> 1.3.0 发布当日跟进的补丁。1.3.0 的对抗验证在发布后又挖出 **5 组 `DruidConfig` Debug
> 明文口令泄漏**与 1 个打包缺陷，本版一并修复。

### Fixed — 安全（`DruidConfig` 的 `Debug` 输出泄露口令）

1.3.0 已把脱敏从黑名单改为"逐段判定"，但对**判定范围与作用范围不一致**的边界仍漏了三次
（第一次在 1.3.0 内已修，后两次是本版修的）：

- **userinfo 旁边的参数值**：`userinfo_at` 取的是**最后一个** `@`，只看它所在段是否形如 `k=v`；
  是则**整串**跳过凭据打码 → `jdbc:mysql://root:<pw>@host/db?email=a@b.com` 的口令原样输出。
  现改为从右往左取第一个**不在参数值里**的 `@`/`%40`，豁免只作用于该分隔符本身。
- **scheme 段从未被判定**：`jdbcUrl=<口令>://host/db` 中 `://` 之前的整截被当"结构性位置"原样输出。
  现按 `SAFE_SCHEME_LABELS` 白名单 + `:` 逐段比对，未知标签整段打码（对照：`detect_db_type_from_url`
  是子串匹配，直接复用会放行 `<口令>mysql://h`）。
- **键名标签走子串匹配**：`?<口令>pass=x` 的键名整段是口令，却因含 `pass` 被当已知标签**原样打印**。
  现「值要不要打码」仍用子串（fail-closed，多打无妨），「键名能不能回显」改为精确白名单。

三次同源缺陷收敛为一条原则：**每一段都要判定，判定范围 = 作用范围**，且由
`declared_tradeoffs_are_bounded` 把三处结构性可读位置（用户名 / host / 库名）逐个钉住。

### Fixed — 打包

- **`druid-console` 此前无法发布**：`include_str!("../../docs/assets/mascot.svg")` 引用 crate
  目录外的文件，`cargo package` 打出的 tarball 缺文件、编译失败。资产移入 `druid-console/assets/`，
  并加防漂移测试保证与 `docs/assets/mascot.svg` 字节一致（从 registry 解包构建时自动跳过）。

### Added

- **`druid-wall` 与 `druid-console` 首次发布到 crates.io**（1.3.0 因限流与打包缺陷未上架）。
- 10 个 crate 补 `readme` 字段，crates.io 页面显示项目 README。

## [1.3.0] - 2026-10-01

> 本次为**深度审查 + 对抗验证**后的集中修复。审查（4 名审查员）与分析（2 名对抗验证员、3 轮）
> 共确认 **11 项**在"全绿"状态下未被发现的缺陷，含 3 个防火墙绕过、2 个资源泄漏、
> 2 个并发缺陷、4 组明文口令泄漏。核心结论：**1.2.0 的两个主打功能（SQL 防火墙、
> SQL 统计）在连接池路径上从不执行**——Filter 无处挂载，且 `PoolGuard` 只暴露裸连接。

### 破坏性 / 行为变更（升级前请阅读）

- **`druid-wall` 默认 fail-closed**：解析失败或语句类型无法识别的 SQL 一律**拒绝**。
  此前是解析失败即放行（fail-open），导致默认配置宣称要拦的 `TRUNCATE` / `ALTER TABLE` /
  `INTO OUTFILE` / `SLEEP` 全部可绕过。逃生开关 `WallConfig::deny_unparsable = false`。
  代价：parser 尚未支持的语法（`UNION`、`SHOW`、`USE`、`SET`、`ON DUPLICATE KEY UPDATE`、
  `SELECT ... FOR UPDATE` 等）会表现为"合法 SQL 被拒"。事务控制语句
  （`BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT`/`START TRANSACTION`）已开安全通道。
- **`DruidConfig::filters` 非空 → `init()` 返回 `DruidError::Config`**：该字段从未接线，
  原先"配置了却静默无效"。请改用 `DruidDataSource::with_filters(...)`。
- **`DruidError` 加 `#[non_exhaustive]`**：下游穷尽 `match` 会编译失败，需补 `_ =>`。
- **`max_lifetime_ms` 语义修正**：改为按物理连接**创建时刻**（`created_at`）判定。
  原按 `last_used_at`，热点连接每次归还都刷新，导致该项**实际从未生效**。
- **`initial_size > max_active` 等非法配置现在 `init()` 报错**（新增 `DruidConfig::validate()`），
  不再静默创建借不出去的连接。
- **`druid-ha`：`NodeStatus::Testing` 已删除**（该状态无消费者，且取消会导致节点永久卡死）。
- **`druid-util`：`substitute_params` 标记 `#[deprecated]`**（对 MySQL 不安全，见下方安全修复），
  请改用 `substitute_params_mysql`。
- **`druid-util` crypto：`encrypt` 返回 `Result<String, CryptoError>`、
  `decrypt` 返回 `Result<Zeroizing<String>, CryptoError>`**，并对 `DRUID_CONFIG_KEY`
  强制 32 字节（hex/base64）校验，不再零填充/静默截断。

### Added

- `druid-pool`：**`DruidDataSource::with_filters(driver, config, Vec<Box<dyn Filter>>)`** 与
  `with_chain` —— Filter 在构造期注入，链在包装为 `Arc` 前完成配置，无注册竞态。
- `druid-pool`：**`PoolGuard::execute()` / `query()`** —— 唯一会让 SQL 流经 Filter 链的入口，
  钩子顺序 `statement_created → execute_before → 驱动 → execute_after | statement_error → statement_closed`
  （取消/panic 也由 `StmtScope` 的 Drop 恰好闭合一次）。`connection()` 保留为**绕过 Filter 的逃生通道**。
- `druid-filter`：补齐 `statement_created` / `statement_closed` / `statement_error` /
  `connection_borrow_before` / `connection_return_before` 五个此前**定义了却从不 dispatch** 的钩子；
  新增 `impl<T: Filter + ?Sized> Filter for Arc<T>`（可传 `Arc<StatFilter>` 进链并保留句柄）。
- `druid-core`：`DruidConfig::validate()`；`redact` 模块（Debug 脱敏）。
- `druid-pool`：`metrics_arc()`、`pool_size()`（单锁快照，供一致性断言）。
- `druid-stat`：`StatFilter::with_max_sql_size()`（SQL 统计表容量上限）、`bind_metrics()`、
  `get_slow_sql_from()`。
- `druid-console`：**`start_server_with_token()` / `make_router_with_token()`**（`Authorization: Bearer`
  校验，常量时间比较，空 token 一律 401），所有响应加 `Cache-Control: no-store`；
  `/druid/sql.json`、`/druid/slow-sql.json` 支持 `?limit=` / `?offset=`。
- `druid-wall`：`WallConfig::deny_unparsable`；事务控制语句安全通道；`DenyOperation`
  的 `Truncate`/`AlterTable`/`CreateIndex`/`Grant`/`Revoke`/`Call`/`Execute` 按首关键字归类后可命中。
- `druid-sql`：支持 `LIMIT offset, count`、`@var` / `@@var`、`EXPLAIN [ANALYZE] <stmt>`；
  `SQLExpr::Variable`、`Token::Variable`。
- `druid-ha`：`failure_threshold` / `success_threshold` / `max_backoff` / `set_probe_timeout()`；
  `spawn_health_check_loop()` 返回 `JoinHandle` 并提供 `shutdown()`。

### Fixed — 安全

- **`druid-wall` 防火墙 fail-open**：`WallProvider::check` 在解析失败时只 `warn!` 就放行，
  且 `quick_check` 的 deny 结果被丢弃 → 8 条绕过全部封堵；`check_unparsable` 改为 fail-closed。
- **`druid-wall` 函数黑名单不递归参数**：`Function{name,..}` 的 `..` 吞掉 `args` →
  `COALESCE(SLEEP(5),0)` / `COUNT(SLEEP(1))` 可绕过；现递归遍历。
- **`druid-wall` `CREATE TABLE ... DEFAULT (RAND())` 绕过**：默认值表达式从不遍历 +
  `CreateTable` 不在默认 `deny_operations`。已在真机（MySQL 8.0.46 / MariaDB 11.8.9）复现确认可执行。
- **`druid-wall` `deny_keywords` 被引号致盲（两次）**：`quick_check` 自行按字符数引号奇偶剥离字符串，
  先后被 `\'`（`SELECT 'a\'', secret FROM t`）与 `"`/反引号（`SELECT "O'Brien", secret FROM t`，
  真机确认 UPDATE/DELETE 可执行）击穿。**修法改为结构化**：匹配文本由 `druid_sql` 的**词法器** token 流
  重建（`bare_text()`），字符串/标识符/注释边界只有一份实现。
- **`druid-util::substitute_params` 反斜杠 SQL 注入**：只翻倍单引号、不处理 `\`，
  在 MySQL 默认 `sql_mode` 下 `["\\", " OR 1=1 -- "]` 可注入。新增 `substitute_params_mysql`。
- **`DruidConfig` 的 `Debug` 泄露口令**：只屏蔽 `password` 字段，`url` 内嵌凭据
  （`user:pass@`、`?password=`、SQL Server `;password=`、Oracle thin）与 `connection_properties`
  原样打印。现按**白名单**逐段脱敏（未建模的键一律打码）。
- **`druid-console` 无鉴权**：`/druid/sql.json` 把 SQL 原文（含字面量）暴露给任意访问者。
- **`druid-util` crypto 的 `aes`/`aes-gcm` 未启用 `zeroize` feature**：轮密钥与 GHASH 临时密钥
  的 `Drop` 清零被 `#[cfg]` 编译掉，上层的 `Zeroizing` 保护被底层 feature 关掉。
- **`druid-proxy` `close()` 先记账后关闭**：真实 close 失败时统计已记"销毁成功"、
  `closed` 已置位 → fd 泄漏且监控显示正常。

### Fixed — 正确性与并发

- **连接池取消安全**：`get_connection` 在构造 `PoolGuard` 前有两个无 guard 的 `.await`，
  被 `timeout`/`select!` 取消时**物理连接泄漏**且 `active_count`/`waiting` 虚高。
  现提前构造 `PoolGuard::pending()`，取消由 Drop 兜底。
- **`close()` 返回后仍创建并交付连接**：connect/validate 后未复查 `is_closed()`。
- **归还校验窗口计数失真**：连接在"已扣 active、未入 idle"期间对指标与 `close()` 双双不可见。
- **`close()` 不唤醒 semaphore 排队者** → `max_wait_ms = 0`（默认无限等）下永久挂起。
- **KeepAlive 两处**：会关掉正在使用的连接；且校验期间连接仍在 idle 中可被借用 →
  真驱动上 ping 与业务 SQL 撞同一 socket（协议错乱）。现与借用路径**共用同一套状态语义**
  （取许可 → 同一锁内摘出并计 active → 锁外校验 → 归还或销毁）。
- **permit 先于连接归位释放** → 物理连接数可超 `max_active`。
- **`DruidDataSource` 无 `Drop`** → 丢弃数据源泄漏 2 个后台任务与全部空闲连接。
- **`druid-ha`**：节点永久卡在 `Testing` 被静默移出轮询；巡检无单点超时（一个节点挂住即全停摆）；
  单次失败即判死、单次成功即复活（抖动摆动）；健康检查循环无法停止。
- **`druid-stat`**：SQL 统计表无上限且 key 是完整 SQL 原文（动态 SQL 场景 OOM）；
  active/idle 计数单向漂移（active 侧永不回收）；慢 SQL 日志在持锁状态下打印未截断 SQL。
- **`druid-sql` parser**：`NOT` 优先级错误（`WHERE NOT name LIKE 'a%'` **静默返回 0 行**）；
  递归无深度上限（`"(".repeat(20000)` → 栈溢出 abort）；`parse_sql` 达迭代上限后**静默截断**
  并返回 `Ok`（尾部语句完全不检查）；lexer 把 `--` 一律当注释（MySQL 要求后跟空白）。
- **`druid-sql` format.rs 语义丢失**：字符串字面量不转义引号（格式化后再执行**可变注入**）；
  `CREATE TABLE` 丢 `NOT NULL`/`PRIMARY KEY`/`DEFAULT`；标识符不按需加引号；
  二元运算不按优先级补括号；`TableReference::Join` 输出字面量 `...`。
- **`druid-sql` SchemaVisitor** 漏访 `HAVING` / `LIMIT` / `OFFSET` / `INSERT ... VALUES` 子查询。

### Changed

- `druid-stat` 的 SQL 展示截断改用 `druid-util::truncate_sql`（多字节安全）；
  `druid-stat`/`druid-console`/`druid-proxy` 补 crate 级文档与 `#![warn(missing_docs)]`。
- README / README_EN：防火墙、监控、控制台示例改为**可编译的正确用法**；
  新增 `deny_unparsable` 配置项说明；明确标注 `guard.connection()` 为绕过 Filter 的逃生通道。

### 已知边界（本次刻意不修，已在代码与文档中标注）

- `druid-wall`：`UNION` / `SHOW` / `USE` / `SET` 维持拒绝。`USE` 会拆掉 `deny_schemas`
  （`USE mysql` 后 `SELECT * FROM user` 不再带 schema 前缀）；`SET sql_mode='NO_BACKSLASH_ESCAPES'`
  会改变**后续语句**的字符串解析语义——无状态文本防火墙无法安全放行。
- `druid-wall`：引号标识符（`` `SLEEP` ``、ANSI_QUOTES 下的 `"SLEEP"`）取引号内原文参与匹配，
  默认 `sql_mode` 下可能**多报**（`SELECT "the secret"` 命中 `secret`）。方向为宁可误报。
- `druid-wall`：`tokenize()` 不回传词法错误，故 `quick_check` 看不到 `/*!` 嵌套超深的词法错误。
  触发需 `deny_unparsable: false` + 嵌套 `/*!`，而 MySQL/MariaDB 对该输入均报 1064（不执行）。
- `druid-pool`：`test_on_return` 的归还校验在 `validation_query_timeout_secs = 0`（默认）下无超时，
  网络黑洞时 permit 被后台任务长期占用（该 `max_active` 名额缺失）。
- `druid-sql`：表级约束 `PRIMARY KEY (id)` 不可解析；`EXPLAIN FORMAT=JSON|TREE` 不可解析。
- `druid-util` crypto 未实现 KDF（无 hkdf/sha2 依赖），口令不能作为密钥；
  该模块**目前全仓库零调用方**，CHANGELOG 早先宣称的 "Password encryption" 尚未接线。

## [1.2.0] - 2026-09-26

### Added
- **项目宠物「小德」**（德鲁伊猫头鹰）—— `docs/assets/mascot.svg`，README 与控制台共用同一份资产
- `druid-console`: `/druid/mascot.svg` 路由，并注册为站点 favicon
- `druid-console`: 监控页面页头内联「小德」（`include_str!` 引入，无资产副本）
- 架构设计图 `docs/assets/architecture.svg`（7 层 / 10 crate，依赖单向向下）
- 功能设计图 `docs/assets/features.svg`（六大功能域）
- 连接生命周期图 `docs/assets/lifecycle.svg`（创建 → 空闲 → 活跃 → 验证 → 销毁）
- README: 「项目宠物」「连接生命周期」章节；项目结构补充 `docs/assets/` 与控制台路由
- docs/README_EN.md: 同步宠物、生命周期、图表与结构说明（图注为中文，已注明）

### Fixed
- **CI 转绿**：`cargo fmt --all --check` 全量通过（此前 15 个文件 50 处差异）
- **CI 转绿**：`cargo clippy --all-targets -- -D warnings` 清零
  - `druid-sql/token.rs`: `lookup_keyword` 位于 `mod tests` 之后，移至测试模块之前
  - `druid-sql/visitor/schema.rs`: `get("src").is_none()` → `!contains_key("src")`

### Changed
- README / README_EN 测试徽章与测试分布表：61 → **231**（实测）
- README 部署运维章节版本号示例：1.0.8 → 1.2.0
- `.github/workflows/release.yml`: `git tag -a` + `git push` 改为
  `gh release create --target`。runner 已不再预置 git 身份，原先的
  `git tag -a` 会以 `fatal: empty ident name` 失败，导致 Release workflow
  连续失败且不发版（v1.1.9 之后一直如此）

### 含自上一个 release（v1.1.9）以来的变更（未单独发版）

- 全模块单元测试覆盖，测试数 61 → 229（每个 crate 补齐边界与错误路径）
- `docs/TEST_REPORT.md` 单元测试报告
- Release 工作流推送规则（`.github/workflows/release.yml` 增量打 tag + 建 release）
- 14 项缺陷修复，详见 [TEST_REPORT.md](docs/TEST_REPORT.md#发现并修复的问题)，含：
  `is_select_sql` 多字节 panic、`password` 反序列化缺省、SQL 双写引号转义、
  `IN (SELECT ...)` 解析、SchemaVisitor 漏访问 JOIN ON、WallChecker 字面量误判、
  带空格函数名绕过、druid-ha 权重 0 除零 panic、test_on_borrow 失败连接泄漏等

## [1.1.9] - 2026-08-02

> 本条目此前被并入下方的 `[1.1.8]`（两次提交同日）。按实际 tag 拆开：v1.1.9 指向
> `生态配置修复 (7 项)`，v1.1.8 指向 `修复问题`。

### Added
- CI workflow (`.github/workflows/ci.yml`) —— check / test / clippy / fmt 四个 job
- `LICENSE` file (Apache 2.0)
- `CHANGELOG.md` and `CONTRIBUTING.md`
- Payment QR codes in README

### Changed
- 10 个子 crate 补齐 `description` / `keywords` / `categories` 元数据
- Repository URL updated to `alibaba/druid-rust`

### Removed
- Unused dependencies: `sqlx`, `rand`, `tracing-subscriber` from workspace
- Unused deps from `druid-pool`, `druid-filter`, `druid-sql` crate manifests

## [1.1.8] - 2026-08-02

### Added
- CTE/WITH clause support in SQL parser, AST, formatter, and visitor
- KeepAlive TOCTOU fix using (id, last_used_at) double-condition eviction
- `inc_cache_hit` counter in PoolMetrics
- `dec_waiting` on all error paths in get_connection
- `inc_create` for borrow-path new connections
- `inc_destroy` in eviction, expiry, and validation-failure paths
- Wall checker covers all SQL clauses (group_by, having, order_by, limit, offset, subqueries)

### Changed
- `PoolGuard<C: Connection>` → `PoolGuard<D: Driver>` for test_on_return support
- `DropTableStatement` → `DropStatement` with `DropObjectType` enum
- `dec_waiting` moved from PoolGuard::drop to get_connection permit acquisition
- `DROP` 语句解析补齐 `advance()`，修复对象类型判断
- `StatFilter.idle_count` 改用 `saturating_sub`，避免 underflow

### Removed
- `Token::NationalString`, `Token::Whitespace` dead variants

## [1.1.0] - 2026-07-31

### Added
- Initial release with 10 crates
- Async connection pool with semaphore-based concurrency
- SQL parser (lexer, recursive-descent parser, 30 dialect types)
- SQL firewall (WallFilter with AST-level checks)
- SQL monitoring (StatFilter with slow SQL detection)
- High-availability data source (weighted round-robin)
- Web monitoring console (axum-based)
- Filter chain architecture (20+ lifecycle hooks)
- PSCache for prepared statement caching
- Password encryption (AES-256-GCM)
- 61 unit/integration tests
