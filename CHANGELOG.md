# Changelog

All notable changes to Druid-Rust.

## [1.1.8] - 2026-08-02

### Added
- CTE/WITH clause support in SQL parser, AST, formatter, and visitor
- KeepAlive TOCTOU fix using (id, last_used_at) double-condition eviction
- `inc_cache_hit` counter in PoolMetrics
- `dec_waiting` on all error paths in get_connection
- `inc_create` for borrow-path new connections
- `inc_destroy` in eviction, expiry, and validation-failure paths
- Wall checker covers all SQL clauses (group_by, having, order_by, limit, offset, subqueries)
- CI workflow (`.github/workflows/ci.yml`)
- `LICENSE` file (Apache 2.0)
- `CHANGELOG.md` and `CONTRIBUTING.md`
- Payment QR codes in README

### Changed
- `PoolGuard<C: Connection>` → `PoolGuard<D: Driver>` for test_on_return support
- `DropTableStatement` → `DropStatement` with `DropObjectType` enum
- `dec_waiting` moved from PoolGuard::drop to get_connection permit acquisition
- Repository URL updated to `alibaba/druid-rust`

### Removed
- Unused dependencies: `sqlx`, `rand`, `tracing-subscriber` from workspace
- Unused deps from `druid-pool`, `druid-filter`, `druid-sql` crate manifests
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
