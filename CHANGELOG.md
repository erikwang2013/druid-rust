# Changelog

All notable changes to Druid-Rust.

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
