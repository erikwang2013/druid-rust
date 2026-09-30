//! druid-wall 对抗验证（第三方攻击视角）。
//!
//! 目标：证伪「fail-closed 已落地、8 条绕过输入全部封堵」。
//! 所有用例走真实路径 —— `WallProvider::check()`（quick_check → 解析 → AST）；
//! 端到端用例走 `DruidDataSource::with_filters` + `PoolGuard::execute/query`，
//! 不直接调 `WallChecker::check(sql, &手工 stmt)`（那正是上一轮测试全绿却没用上的原因）。
//!
//! 判定依据除本仓库代码外，还用真实后端实测（docker 一次性容器）：
//!   mysql:8.0.46 与 mariadb:11.8.9。关键结论：
//!   - `/*!` 可执行注释**不可嵌套**（2 层即 1064 语法错误）→ 深度上限暂不可利用
//!   - `` `SLEEP`(1) `` 会被真正执行（反引号不阻止函数调用）→ 本仓库解析器拒绝，安全
//!   - `--` 后跟 U+00A0/U+3000/U+0085 → 两个后端都报语法错误（不视为注释）
//!   - `/*! INTO OUTFILE ... */` 会被执行（MySQL 侧受 secure_file_priv 限制）
//!   - MySQL 3770 / MariaDB 1901：SLEEP/BENCHMARK/LOAD_FILE 不允许出现在
//!     DEFAULT / CHECK 表达式里；但 RAND()/USER()/UUID()/CONNECTION_ID() 两边都允许
//!
//! Round 2 状态：**2 个真实绕过均已修复**（`deny_keywords` 转义引号致盲、
//! `CREATE TABLE` DEFAULT 函数黑名单空转），用例已转正为回归锚点。
//!
//! Round 3 状态：**绕过已修复**——`"O'Brien"` / `` `a'b` `` 里的奇数个 `'` 曾让
//! deny_keywords 整体失效（真机实测 UPDATE 改数据）。根因是 checker 自建字符串扫描；
//! 修法是停止自查、匹配文本改由 `druid-sql` 词法器 token 重建（见 `checker.rs::bare_text`），
//! 用例已转正为回归锚点 `finding_deny_keywords_blinded_by_double_quoted_string`。
//! 事务控制放行通道与脱敏白名单本轮未发现可利用缺口（尝试清单与后端实测见对应用例注释）。

use druid_core::{DruidConfig, DruidError};
use druid_filter::Filter;
use druid_pool::{Connection, Driver, DruidDataSource};
use druid_wall::{WallChecker, WallConfig, WallFilter, WallProvider};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ────────────────────────── 工具 ──────────────────────────

/// 走真实链路判定一条 SQL：Ok=放行，Err=拒绝原因
fn verdict(cfg: WallConfig, sql: &str) -> Result<(), String> {
    let mut p = WallProvider::new(WallChecker::new(cfg), 64);
    let r = p.check(sql);
    if r.allowed {
        Ok(())
    } else {
        Err(r.violations[0].message.clone())
    }
}

/// 断言：必须被拒绝（expect 为空表示只要求拒绝，不校验文案）
fn deny(cfg: WallConfig, sql: &str, expect: &str) {
    match verdict(cfg, sql) {
        Err(msg) => assert!(
            msg.contains(expect),
            "SQL {:?} 被拒但信息 {:?} 不含 {:?}",
            sql,
            msg,
            expect
        ),
        Ok(()) => panic!("绕过！SQL {:?} 被放行（期望拒绝，含 {:?}）", sql, expect),
    }
}

fn allow(sql: &str) {
    if let Err(msg) = verdict(WallConfig::default(), sql) {
        panic!("误伤！合法 SQL {:?} 被拒绝：{}", sql, msg);
    }
}

fn schema_cfg() -> WallConfig {
    WallConfig {
        deny_schemas: vec!["mysql".into()],
        ..Default::default()
    }
}

// ────────────── 1. 注释与空白拆词 ──────────────

#[test]
fn test_comment_and_whitespace_splitting() {
    let cases: &[(&str, &str)] = &[
        ("SELECT SLEEP/**/(1)", ""), // 解析失败 → fail-closed
        ("SELECT SLEEP /*x*/ (1)", ""),
        ("SELECT SLEEP\t(1)", "SLEEP"),
        ("SELECT SLEEP\n(1)", "SLEEP"),
        ("SELECT SLEEP\r(1)", "SLEEP"),
        ("SELECT SLEEP\x0b(1)", "SLEEP"),
        ("SELECT SLEEP\x0c(1)", "SLEEP"),
        ("SELECT SLEEP\u{200b}(1)", ""),
        ("TRUNCATE/**/TABLE t", "TRUNCATE"),
        ("/* c */ TRUNCATE TABLE t", "TRUNCATE"),
        ("TRUNCATE\nTABLE t", "TRUNCATE"),
        ("TRUNCATE\x0bTABLE t", "TRUNCATE"),
        ("TRUNCATE\x0cTABLE t", "TRUNCATE"),
        ("truncate/**/table t", "TRUNCATE"),
        ("DROP/**/TABLE t", "DROP TABLE"),
        ("ALTER/**/TABLE t ADD c INT", "ALTER TABLE"),
        ("SELECT * FROM t INTO/**/OUTFILE '/tmp/x'", "INTO OUTFILE"),
    ];
    for (sql, expect) in cases {
        deny(WallConfig::default(), sql, expect);
    }
}

// ────────────── 2. MySQL 可执行注释 /*! ──────────────

#[test]
fn test_executable_comments() {
    let cases: &[(&str, &str)] = &[
        ("SELECT /*!50000 SLEEP(1)*/", "SLEEP"),
        ("SELECT /*! SLEEP(1) */", "SLEEP"),
        ("SELECT /*!SLEEP(1)*/", "SLEEP"),
        ("SELECT 1 /*!50000 , SLEEP(1) */", "SLEEP"),
        ("SELECT 1 /*!50000 /*!50000 SLEEP(1) */ */", "SLEEP"),
        ("SELECT 1 /*!50000 INTO OUTFILE '/tmp/x' */", "INTO OUTFILE"),
        (
            "SELECT 1 /*!50000 INTO/**/OUTFILE '/tmp/x' */",
            "INTO OUTFILE",
        ),
        ("/*!50000 DROP TABLE t */", "DROP TABLE"),
        ("SELECT 1 /*!; DROP TABLE t */", "multiple statements"),
        ("SELECT 1 /*! ; DROP TABLE t */", "multiple statements"),
        ("SELECT /*!50000 LOAD_FILE('/etc/passwd')*/", "LOAD_FILE"),
        // 反引号函数名：MySQL/MariaDB 会真正执行 `SLEEP`(1)；本仓库解析器不支持 → fail-closed
        ("SELECT `SLEEP`(1)", ""),
        ("SELECT `SLEEP`/**/(1)", ""),
    ];
    for (sql, expect) in cases {
        deny(WallConfig::default(), sql, expect);
    }
}

// ────────────── 3. `--` 行注释变体 ──────────────

#[test]
fn test_dash_dash_variants() {
    let cases: &[(&str, &str)] = &[
        // MySQL: `--` 后无空白时不是注释，而是两个减号，SLEEP 会真的执行
        ("SELECT 1--SLEEP(1)", "SLEEP"),
        ("SELECT 1--x\nSLEEP(1)", ""),
        ("SELECT 1 --SLEEP(1)", ""),
        ("SELECT 1--\tSLEEP(1)", ""),
        ("SELECT 1--\nSLEEP(1)", ""),
        // 真注释但后面还有语句/函数
        ("SELECT 1 -- ok\nDROP TABLE t", ""),
    ];
    for (sql, expect) in cases {
        deny(WallConfig::default(), sql, expect);
    }
    // 明确的强断言：无空白 `--` 后的 SLEEP 必须被识别为函数（而非注释内容）
    deny(WallConfig::default(), "SELECT 1--SLEEP(1)", "SLEEP");
}

// ────────────── 4. 大小写 / Unicode ──────────────

#[test]
fn test_case_and_unicode_evasion() {
    let cases: &[(&str, &str)] = &[
        ("sLeEp(1)", "SLEEP"),
        ("SELECT * FROM t WHERE x = sLeEp(1)", "SLEEP"),
        ("sElEcT 1 fRoM t WhErE bEnChMaRk(1,1)", "BENCHMARK"),
        ("SELECT * FROM t INTO/**/oUtFiLe '/tmp/x'", "INTO OUTFILE"),
        ("truncate table t", "TRUNCATE"),
        ("\u{200b}SELECT 1", ""), // 零宽字符
        ("SELECT \u{200b}SLEEP(1)", "SLEEP"),
        ("SELECT SLEEP\u{ff08}1\u{ff09}", ""), // 全角括号
    ];
    for (sql, expect) in cases {
        deny(WallConfig::default(), sql, expect);
    }
}

/// 非绕过记录：非 ASCII 标识符被墙放行，但真实后端同样不执行 —— 两个后端实测：
///   `SELECT ЅLEEP(1)`（西里尔 Ѕ）    => MySQL 1064 / MariaDB 1064
///   `SELECT ＳＬＥＥＰ(1)`（全角字母） => MySQL 1064 / MariaDB 1064
/// MySQL 不归一化标识符，这些字符永远不等于 ASCII 的 SLEEP，故不可利用。
/// 此处固化现状（避免误报为绕过），同时作为「词法器按 is_alphabetic 收标识符」
/// 这一行为的回归锚点。
#[test]
fn non_finding_exotic_identifiers_are_not_executable_in_mysql() {
    for sql in [
        "SELECT ЅLEEP(1)",
        "SELECT \u{ff33}\u{ff2c}\u{ff25}\u{ff25}\u{ff30}(1)",
    ] {
        assert!(
            verdict(WallConfig::default(), sql).is_ok(),
            "现状固化：本仓库放行非 ASCII 同形字标识符 {:?}",
            sql
        );
    }
}

// ────────────── 5. 函数嵌套与位置 ──────────────

#[test]
fn test_function_nesting_and_positions() {
    let cases: &[&str] = &[
        "SELECT COALESCE(SLEEP(1),0)",
        "SELECT ABS(SLEEP(1))",
        "SELECT COUNT(SLEEP(1)) FROM t",
        "SELECT CASE WHEN SLEEP(1) THEN 1 END",
        "SELECT CASE 1 WHEN 1 THEN SLEEP(1) ELSE 0 END",
        "SELECT 1 FROM t WHERE SLEEP(1)",
        "SELECT 1 FROM t ORDER BY SLEEP(1)",
        "SELECT 1 FROM t GROUP BY SLEEP(1)",
        "SELECT 1 FROM t HAVING SLEEP(1)",
        "SELECT 1 FROM t LIMIT SLEEP(1)",
        "SELECT 1 FROM t LIMIT 1 OFFSET SLEEP(1)",
        "SELECT (SELECT SLEEP(1))",
        "SELECT * FROM (SELECT SLEEP(1)) x",
        "SELECT * FROM t JOIN u ON SLEEP(1)",
        "SELECT * FROM t WHERE x IN (SELECT SLEEP(1))",
        "SELECT EXISTS(SELECT SLEEP(1))",
        "SELECT CAST(SLEEP(1) AS CHAR)",
        "SELECT ROW_NUMBER() OVER (ORDER BY SLEEP(1)) FROM t",
        "SELECT -SLEEP(1)",
        "SELECT NOT SLEEP(1)",
        "WITH c AS (SELECT SLEEP(1)) SELECT * FROM c",
        "INSERT INTO t (a) VALUES (SLEEP(1))",
        "INSERT INTO t SELECT SLEEP(1)",
        "UPDATE t SET a = SLEEP(1) WHERE id=1",
        "DELETE FROM t WHERE SLEEP(1)",
        "SELECT * FROM t WHERE a BETWEEN SLEEP(1) AND 2",
        "SELECT * FROM t WHERE a LIKE SLEEP(1)",
        "SELECT SLEEP(1) /* trailing */",
        "SELECT SLEEP(/* x */ 1)",
        // 引号奇偶被转义引号打乱后，AST 必须兜住（SELECT 类语句的对照证据）
        "SELECT 'a\\'', SLEEP(1)",
        "SELECT 'a\\'', LOAD_FILE('/etc/passwd')",
        "INSERT INTO t VALUES ('x\\'', SLEEP(1))",
    ];
    for sql in cases {
        deny(WallConfig::default(), sql, "");
    }
    deny(
        WallConfig::default(),
        "SELECT LOAD_FILE('/etc/passwd')",
        "LOAD_FILE",
    );
    deny(
        WallConfig::default(),
        "SELECT BENCHMARK(1000000, MD5('a'))",
        "BENCHMARK",
    );
}

// ────────────── 6. 语句类型 / 多语句 / 截断 ──────────────

#[test]
fn test_statement_types_are_denied() {
    let cases: &[(&str, &str)] = &[
        ("TRUNCATE TABLE users", "TRUNCATE"),
        ("ALTER TABLE t ADD c INT", "ALTER TABLE"),
        ("DROP TABLE users", "DROP TABLE"),
        ("DROP DATABASE d", "DROP TABLE"),
        ("CREATE INDEX i ON t(a)", ""),
        ("GRANT ALL ON *.* TO 'a'@'b'", ""),
        ("REVOKE ALL ON *.* FROM 'a'@'b'", ""),
        ("CALL some_proc()", ""),
        ("EXECUTE stmt", ""),
        ("RENAME TABLE a TO b", ""),
        ("PREPARE s FROM 'DROP TABLE t'", ""),
        ("EXPLAIN ANALYZE DELETE FROM t", "DELETE without WHERE"),
        ("EXPLAIN ANALYZE DROP TABLE t", ""),
        ("/*!50000 DROP TABLE t */", "DROP TABLE"),
        ("SELECT 1; DROP TABLE t", "multiple statements"),
        // 多语句若含被禁函数，quick_check 的函数扫描先命中（两者都拦，只是命中点不同）
        ("SELECT 1;SELECT SLEEP(1)", "SLEEP"),
        ("; ; DROP TABLE t", ""),
        // 命中点视语句类型而定：TRUNCATE 在解析层就被拒（unparseable），DROP 走到语句计数
        ("SELECT 1 ; TRUNCATE TABLE t", ""),
    ];
    for (sql, expect) in cases {
        deny(WallConfig::default(), sql, expect);
    }
    // 默认 deny_operations 只含 Truncate/DropTable/AlterTable，
    // CREATE TABLE 不在其中（属配置边界，不是绕过）——固化现状
    assert!(
        verdict(WallConfig::default(), "CREATE TABLE t (a INT)").is_ok(),
        "默认配置不拒绝 CREATE TABLE（deny_operations 未含 CreateTable）"
    );
}

#[test]
fn test_overlong_sql_is_denied() {
    let long = format!("SELECT {} FROM t", "1,".repeat(5000)); // > 8192 字节
    deny(WallConfig::default(), &long, "");
}

// ────────────── 7. INTO OUTFILE / DUMPFILE ──────────────

#[test]
fn test_into_outfile_variants() {
    let cases: &[&str] = &[
        "SELECT * FROM users INTO OUTFILE '/tmp/x'",
        "SELECT * FROM users INTO  OUTFILE '/tmp/x'",
        "SELECT * FROM users INTO/**/OUTFILE '/tmp/x'",
        "SELECT * FROM users INTO\nOUTFILE '/tmp/x'",
        "SELECT * FROM users INTO\tOUTFILE '/tmp/x'",
        "SELECT * FROM users INTO\rOUTFILE '/tmp/x'",
        "SELECT * FROM users INTO\x0bOUTFILE '/tmp/x'",
        "SELECT * FROM users INTO\x0cOUTFILE '/tmp/x'",
        "SELECT * FROM users INTO DUMPFILE '/tmp/x'",
        "SELECT * FROM users INTO/**/DUMPFILE '/tmp/x'",
        "SELECT id INTO OUTFILE '/tmp/x' FROM users",
        "select * from users into outfile '/tmp/x'",
        "SELECT 1 INTO OUTFILE '/tmp/x'",
    ];
    for sql in cases {
        deny(WallConfig::default(), sql, "INTO OUTFILE");
    }
}

// ────────────── 8. schema 逃逸（deny_schemas=[mysql]） ──────────────

#[test]
fn test_schema_escaping() {
    let cases: &[&str] = &[
        "SELECT * FROM mysql.user",
        "SELECT * FROM MySQL.user",
        "SELECT * FROM MYSQL.USER",
        "SELECT * FROM mysql . user",
        "SELECT * FROM mysql\n.\nuser",
        "SELECT * FROM `mysql`.`user`",
        "SELECT * FROM a JOIN mysql.user u ON 1=1",
        "SELECT * FROM a LEFT JOIN mysql.user u ON 1=1",
        "SELECT * FROM (SELECT * FROM mysql.user) x",
        "SELECT * FROM t WHERE x IN (SELECT 1 FROM mysql.user)",
        "SELECT 1 FROM t WHERE x = (SELECT COUNT(*) FROM mysql.user)",
        "SELECT * FROM mysql.user INTO OUTFILE '/tmp/x'",
        // 注释插在 schema 与点之间：解析失败 → fail-closed（不是 schema 规则命中，但同样拒绝）
        "SELECT * FROM mysql/**/.user",
    ];
    for sql in cases {
        deny(schema_cfg(), sql, "");
    }
    assert!(verdict(schema_cfg(), "SELECT * FROM other.user").is_ok());
    assert!(verdict(schema_cfg(), "SELECT * FROM users").is_ok());
}

// ────────────── 9. 反向验证：任务指定的合法 SQL 必须放行 ──────────────

#[test]
fn test_legit_sql_must_pass() {
    for sql in [
        "SELECT 1",
        "SELECT COUNT(*) FROM t",
        "INSERT INTO t VALUES (1)",
        "SELECT a.id FROM a JOIN b ON a.id=b.id",
        "SELECT * FROM (SELECT 1) x",
        "WITH c AS (SELECT 1) SELECT * FROM c",
        "SELECT UPPER(name) FROM t",
        "SELECT 'sleep(1)' FROM t",
        "SELECT 1 LIMIT 10",
        "SELECT 1 LIMIT 5,10",
        "SELECT 1 ORDER BY x DESC",
        "EXPLAIN SELECT 1",
        "SELECT @v",
        "SELECT @@version",
    ] {
        allow(sql);
    }
    for sql in [
        "SELECT `id` FROM `users` WHERE `x`=1",
        "SELECT COUNT(*) FROM t WHERE a BETWEEN 1 AND 2",
        "SELECT CASE WHEN a=1 THEN 'x' ELSE 'y' END FROM t",
        "SELECT DATE_FORMAT(NOW(), '%Y-%m-%d')",
        "SELECT * FROM t WHERE a IS NOT NULL",
        "UPDATE t SET a=1 WHERE id=1",
        "DELETE FROM t WHERE id=1",
        "SELECT id FROM t ORDER BY id DESC LIMIT 0,10",
    ] {
        allow(sql);
    }
}

// ────────────── 10. 病态输入：不得 panic（DoS 面） ──────────────

#[test]
fn test_pathological_inputs_do_not_panic() {
    // (a) 拒绝且与后端一致：后端同样报 1064
    for sql in ["SELECT /*", "\0\0\0"] {
        assert!(
            verdict(WallConfig::default(), sql).is_err(),
            "病态输入被放行：{:?}",
            sql
        );
    }
    // (b) 拒绝但后端接受（fail-closed 的过度拦截，非安全问题）：
    //     500 层括号 → 墙 "nesting too deep (> 128)"，两个后端 rc=0 正常执行
    //     3000 条语句 / 10000 位数字 → 墙 "SQL too long"
    let deep_parens = format!("SELECT {}1{}", "(".repeat(500), ")".repeat(500));
    let many_stmts = "SELECT 1;".repeat(3000);
    for sql in [
        deep_parens,
        many_stmts,
        format!("SELECT {}", "9".repeat(10000)),
    ] {
        assert!(
            verdict(WallConfig::default(), &sql).is_err(),
            "病态输入被放行：{:?}",
            &sql[..sql.len().min(40)]
        );
    }
    // (c) 放行但后端报错，不可利用：未闭合引号/反引号 ——
    //     本仓库词法器把它吞到行尾当字符串，实测 MySQL 8.0.46 与 MariaDB 11.8.9
    //     对 `SELECT '` / `SELECT "` / `` SELECT ` `` 一律 1064（不构成合法语句）。
    //     ⚠️ 与文件末尾 finding_deny_keywords_blinded_by_escaped_quote 同源：
    //     字符串剥离逻辑一旦被扰动，其后的文本就不再被扫描。
    for sql in ["SELECT '", "SELECT \"", "SELECT `"] {
        assert!(
            verdict(WallConfig::default(), sql).is_ok(),
            "现状固化：未闭合引号被吞成字符串"
        );
    }
    // (d) 放行且后端接受：200 位数字字面量（两边都原样返回），无误判
    assert!(verdict(
        WallConfig::default(),
        &format!("SELECT {}", "9".repeat(200))
    )
    .is_ok());
    // 深嵌套可执行注释「炸弹」：本仓库放行（内容被丢弃），实测两个后端都报 1064
    // （MySQL/MariaDB 不支持嵌套 /*!），故不构成绕过 —— 见 test_exec_comment_depth_ceiling_denied。
    let exec_bomb = format!("SELECT 1 /*!{}*/", "/*!".repeat(200) + &"*/".repeat(200));
    let _ = verdict(WallConfig::default(), &exec_bomb); // 只要求不 panic
}

// ────────────── 11. 端到端：墙必须挡在驱动之前 ──────────────

#[derive(Debug)]
struct MockConn {
    id: u64,
    exec_calls: Arc<AtomicU64>,
    query_calls: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl Connection for MockConn {
    async fn execute(&self, _sql: &str) -> Result<u64, DruidError> {
        self.exec_calls.fetch_add(1, Ordering::SeqCst);
        Ok(1)
    }
    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DruidError> {
        self.query_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![vec!["ok".into()]])
    }
    async fn close(&self) -> Result<(), DruidError> {
        Ok(())
    }
    async fn ping(&self) -> Result<(), DruidError> {
        Ok(())
    }
    fn connection_id(&self) -> u64 {
        self.id
    }
}

#[derive(Debug)]
struct MockDriver {
    connect_count: AtomicU64,
    exec_calls: Arc<AtomicU64>,
    query_calls: Arc<AtomicU64>,
}

impl MockDriver {
    fn new() -> Self {
        MockDriver {
            connect_count: AtomicU64::new(0),
            exec_calls: Arc::new(AtomicU64::new(0)),
            query_calls: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl Driver for MockDriver {
    type Connection = MockConn;
    async fn connect(
        &self,
        _url: &str,
        _user: &str,
        _pass: &str,
        _timeout: Option<Duration>,
    ) -> Result<MockConn, DruidError> {
        let id = self.connect_count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(MockConn {
            id,
            exec_calls: self.exec_calls.clone(),
            query_calls: self.query_calls.clone(),
        })
    }
    fn name(&self) -> &'static str {
        "MockDriver"
    }
    async fn validate(&self, _conn: &MockConn) -> Result<(), DruidError> {
        Ok(())
    }
}

fn pool_cfg(filters: Vec<String>) -> DruidConfig {
    let mut c = DruidConfig::new("mock://adversarial", "u", "p");
    c.initial_size = 1;
    c.time_between_eviction_runs_ms = 0; // 关闭后台循环，避免测试悬挂
    c.test_on_borrow = false;
    c.filters = filters;
    c
}

/// 核心证据：`with_filters(WallFilter)` 挂载后，被禁 SQL 在**到达驱动之前**被拦住。
#[tokio::test]
async fn e2e_wall_blocks_before_driver() {
    let driver = MockDriver::new();
    let exec_calls = driver.exec_calls.clone();
    let query_calls = driver.query_calls.clone();
    let ds = DruidDataSource::with_filters(
        driver,
        pool_cfg(vec![]),
        vec![Box::new(WallFilter::new(WallConfig::default())) as Box<dyn Filter>],
    );
    assert_eq!(
        ds.filter_chain().filter_names(),
        vec!["wall"],
        "WallFilter 必须在构造期进入 Filter 链"
    );
    ds.init().await.unwrap();
    let g = ds.get_connection().await.unwrap();

    // 对照组：合法 SQL 真的到达驱动（证明链路是通的，不是「什么都没发生」）
    assert_eq!(g.execute("INSERT INTO t VALUES (1)").await.unwrap(), 1);
    assert_eq!(exec_calls.load(Ordering::SeqCst), 1);
    assert_eq!(g.query("SELECT 1").await.unwrap().len(), 1);
    assert_eq!(query_calls.load(Ordering::SeqCst), 1);

    // 实验组：被禁 SQL 返回 Err，且驱动计数**不变**
    let denials = [
        "DROP TABLE users",
        "TRUNCATE TABLE users",
        "ALTER TABLE users ADD c INT",
        "SELECT SLEEP(5)",
        "SELECT COALESCE(SLEEP(1),0)",
        "SELECT 1; DROP TABLE users",
        "SELECT * FROM users INTO OUTFILE '/tmp/x'",
        "SELECT * FROM mysql.user INTO OUTFILE '/tmp/y'",
        "NOT VALID SQL !!!",
    ];
    for sql in denials {
        let err = g
            .execute(sql)
            .await
            .expect_err(&format!("SQL {:?} 必须被拒绝", sql));
        assert!(
            matches!(err, DruidError::Wall(_)),
            "SQL {:?} 应以 Wall 错误拒绝，实际 {:?}",
            sql,
            err
        );
        // 重复执行：走 provider 缓存路径，拒绝结果不得被缓存反转成放行
        assert!(g.execute(sql).await.is_err());
        assert!(g.query(sql).await.is_err());
    }
    assert_eq!(
        exec_calls.load(Ordering::SeqCst),
        1,
        "被拦截的 SQL 绝不能到达驱动（execute）"
    );
    assert_eq!(
        query_calls.load(Ordering::SeqCst),
        1,
        "被拦截的 SQL 绝不能到达驱动（query）"
    );

    drop(g);
    ds.close().await.unwrap();
}

// ═══════════ 修复锚点与已声明的已知限制 ═══════════
// 已修复的绕过已转为普通用例（必须绿）。已声明的已知限制保留 #[ignore]，
// 固化「当前行为 + 后端实测结论」，任一前提变化（后端支持/中间层归一化）时会失败提醒。

/// 【Round 3 绕过，已修复】deny_keywords 曾被**双引号字符串/反引号标识符里的单引号**整体关闭。
///
/// 根因：`checker.rs::scan_outside_strings` 只把 `'` 当字符串定界符，
/// 但 MySQL 默认 sql_mode 下 `"..."` 也是字符串字面量、`` `...` `` 是标识符。
/// 两者内部出现**奇数个** `'` 时，扫描器把其后所有文本当成「单引号字符串内部」跳过
/// （两遍扫描此时都判定为「未闭合」→ 取 esc 结果 → 放行），
/// 而 deny_keywords **只在 quick_check 里检查**（AST 不查关键词），没有兜底。
///
/// 修法：停止在 quick_check 里自建字符串扫描，匹配文本改由 `druid-sql` 词法器
/// token 重建（`checker.rs::bare_text`）——字符串/标识符/注释的边界只有一份实现；
/// 自建扫描只保留「两种 sql_mode 是否分歧」的 fail-closed 闸门。
///
/// 后端实测（mysql:8.0.46，库 advdb，表 t(id,secret,name)）：
///   `SELECT "O'Brien", secret FROM t`                          => rc=0，返回 secret=42
///   `SELECT id FROM t WHERE name = "O'Brien" AND secret = 42`  => rc=0（合法并执行）
///   `SELECT `name`, secret FROM t`                             => rc=0
///   `INSERT INTO t (name) VALUES ("O'Brien")`                  => rc=0
///   即这些都是**默认 sql_mode 下的合法语句**，不是靠语法错误兜着的。
///
/// 本机 wall Provider::check 实测（deny_keywords=["secret"]）：
///   对照 SELECT 'O''Brien' , secret FROM t    => DENY  双写引号是偶数，不翻转
///   对照 SELECT "O''Brien" , secret FROM t    => DENY
///   对照 SELECT `name` , secret FROM t        => DENY
///   绕过 SELECT "O'Brien" , secret FROM t     => ALLOW ←
///   绕过 SELECT id FROM t WHERE name = "O'Brien" AND secret = 42 => ALLOW ←
///   绕过 UPDATE t SET name = "O'Brien" WHERE secret = 42         => ALLOW ←
///   绕过 SELECT `a'b` , secret FROM t                            => ALLOW ←
/// 影响面：仅 deny_keywords（deny_functions/deny_schemas 由 AST 兜住，INTO OUTFILE 由
/// token 扫描兜住 —— 见 test_round3_quote_forms_do_not_blind_other_layers）。
#[test]
fn finding_deny_keywords_blinded_by_double_quoted_string() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    // 对照：这些形态被正确拒绝
    for sql in [
        "SELECT secret FROM t",
        "SELECT 'O''Brien' , secret FROM t",
        "SELECT \"O''Brien\" , secret FROM t",
        "SELECT `name` , secret FROM t",
    ] {
        deny(cfg.clone(), sql, "forbidden");
    }
    // 原绕过：引号形态里的奇数个 `'`，其后的 secret 曾被整体跳过（全部必须拒绝）
    for sql in [
        "SELECT \"O'Brien\" , secret FROM t",
        "SELECT id FROM t WHERE name = \"O'Brien\" AND secret = 42",
        "UPDATE t SET name = \"O'Brien\" WHERE secret = 42",
        "DELETE FROM t WHERE name = \"O'Brien\" OR secret = 42",
        "SELECT `a'b` , secret FROM t",
    ] {
        deny(cfg.clone(), sql, "forbidden");
    }
}

/// Round 3 对照：同样的引号形态**没有**击穿其它三层（证明影响面确实只有 deny_keywords）。
#[test]
fn test_round3_quote_forms_do_not_blind_other_layers() {
    // 函数黑名单：AST 递归（quick_check 被致盲也照拦）
    for sql in [
        "SELECT \"O'Brien\", SLEEP(1)",
        "SELECT \"O'Brien\", LOAD_FILE('/etc/passwd')",
        "UPDATE t SET a = SLEEP(1) WHERE name = \"O'Brien\"",
        "INSERT INTO t VALUES (\"O'Brien\", SLEEP(1))",
        "DELETE FROM t WHERE name = \"O'Brien\" AND SLEEP(1)",
        "CREATE TABLE t (a VARCHAR(5) DEFAULT \"x'y\", b INT DEFAULT (SLEEP(1)))",
    ] {
        deny(WallConfig::default(), sql, "forbidden function");
    }
    // INTO OUTFILE：token 扫描
    deny(
        WallConfig::default(),
        "SELECT \"O'Brien\" INTO OUTFILE '/tmp/x'",
        "INTO OUTFILE",
    );
    // schema 黑名单：AST
    deny(
        schema_cfg(),
        "SELECT \"O'Brien\" FROM mysql.user",
        "forbidden schema",
    );
    deny(
        schema_cfg(),
        "SELECT `a'b` FROM mysql.user",
        "forbidden schema",
    );
}

/// 事务控制安全通道（Round 3 新增面）：精确 token 形状放行，夹带一律拒绝。
/// 放行的形状 MySQL 8.0.46 实测全部 rc=0；被拒的形态里凡含载荷 token 的一律拦截。
#[test]
fn test_transaction_control_channel() {
    // 放行：标准事务控制
    for sql in [
        "BEGIN",
        "BEGIN WORK",
        "BEGIN;",
        "begin work",
        "BeGiN",
        "BEGIN /* x */",
        "BEGIN/*x*/;",
        "END",
        "COMMIT",
        "COMMIT WORK",
        "ROLLBACK",
        "ROLLBACK WORK",
        "ROLLBACK TO a",
        "ROLLBACK TO SAVEPOINT a",
        "SAVEPOINT a",
        "SAVEPOINT `x`",
        "RELEASE SAVEPOINT a",
        "START TRANSACTION",
        "START TRANSACTION READ ONLY",
        "START TRANSACTION READ WRITE",
        "START TRANSACTION WITH CONSISTENT SNAPSHOT",
    ] {
        assert!(
            verdict(WallConfig::default(), sql).is_ok(),
            "事务控制语句被误伤：{sql:?} → {:?}",
            verdict(WallConfig::default(), sql)
        );
    }
    // 拒绝：任何夹带（多语句 / 载荷 token / 可执行注释展开出的 token）
    for sql in [
        "BEGIN; DROP TABLE users",
        "SAVEPOINT a; DROP TABLE users",
        "ROLLBACK TO a; DROP TABLE users",
        "START TRANSACTION; DROP TABLE users",
        "BEGIN DROP TABLE users",
        "SAVEPOINT a DROP TABLE users",
        "BEGIN /*! DROP TABLE users */",
        "START TRANSACTION /*!; DROP TABLE t */",
        "BEGIN TRUNCATE TABLE t",
        "SAVEPOINT a SELECT 1",
        "ROLLBACK TO ;",
        "SAVEPOINT ;",
        "END WORK",
        "COMMIT AND CHAIN",
        "SAVEPOINT drop",
        "ROLLBACK TO DROP",
        "START TRANSACTION READ ONLYS",
    ] {
        deny(WallConfig::default(), sql, "");
    }
}

/// 非绕过记录（当前行为 + 后端实测）：事务通道放行了几种 MySQL 会报 1064 的残缺形状。
/// 它们是「过度放行」，但后端直接语法错误、不执行任何东西，故不可利用；
/// 此处固化现状：一旦有人收紧形状匹配，本用例会失败提醒更新。
///   `START TRANSACTION READ` / `ROLLBACK TO SAVEPOINT`（缺名字）/ `SAVEPOINT \` /
///   `START TRANSACTION READ WRITE READ ONLY` / `SAVEPOINT "x"` → mysql:8.0.46 全部 1064
#[test]
fn non_finding_transaction_shapes_mysql_rejects() {
    for sql in [
        "START TRANSACTION READ",
        "ROLLBACK TO SAVEPOINT",
        "SAVEPOINT \\",
        "START TRANSACTION READ WRITE READ ONLY",
        "SAVEPOINT \"x\"",
    ] {
        assert!(
            verdict(WallConfig::default(), sql).is_ok(),
            "现状固化：{sql:?} 当前被放行（MySQL 1064，不可利用）"
        );
    }
}

/// 已修复（Round 2）：`deny_keywords` 曾被一个转义单引号整体关闭。
///
/// 原根因：`quick_check` 剥离字符串时只按 `'` 计数、不处理 `\'` 转义 →
/// 奇偶翻转 → 其后所有文本被当作「字符串内部」跳过，deny_keywords 子串扫描失灵
/// （关键词只在 quick_check 检查，无 AST 兜底）。
/// 修复：`checker.rs` 双模式扫描（默认转义 / NO_BACKSLASH_ESCAPES），两遍边界
/// 一致才放行；不一致（`'a\'` 这类默认不闭合、NBE 闭合的输入，NBE 下分歧区间
/// 会被当代码执行）fail-closed 拒绝 "ambiguous string literal"。
///
/// 实测（本机 wall Provider::check）：
///   deny_keywords=["secret"]
///   "SELECT secret FROM t"                => DENY  forbidden: secret
///   "SELECT 'a\'', secret FROM t"         => DENY  forbidden: secret（Round 2 修复）
///   "SELECT 'it\'s', secret FROM t"       => ALLOW 不误伤（默认模式合法完整字符串）
#[test]
fn finding_deny_keywords_blinded_by_escaped_quote() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    deny(cfg.clone(), "SELECT secret FROM t", "forbidden");
    for sql in [
        "SELECT 'a\\'', secret FROM t",
        "SELECT 'a\\'', (SELECT secret FROM t)",
    ] {
        deny(cfg.clone(), sql, "forbidden");
    }
    // 合法转义不误伤
    assert!(verdict(cfg.clone(), "SELECT 'it\\'s' FROM t").is_ok());
    assert!(verdict(cfg, "SELECT 'it''s' FROM t").is_ok());
    // 两种模式对字符串边界分歧 → fail-closed（NBE 模式下分歧区间是代码）
    deny(
        WallConfig::default(),
        "SELECT 'a\\', SLEEP(1), 'x'",
        "ambiguous string literal",
    );
}

/// 已修复（Round 2）：`deny_functions` 曾在 `CREATE TABLE` 上完全空转。
///
/// 原根因（两个缺陷叠加）：
///   a) `rules.rs::visit_stmt` 对 `SQLStatement::CreateTable` 走 `_ => {}`，
///      `ColumnDef.default_value` 里的函数永不检查；
///   b) quick_check 的字符串剥离可被 `\'` 致盲（同上一个用例）。
/// 默认 deny_operations 不含 CreateTable，因此当时没有任何一层兜底。
/// 修复：`rules.rs` 新增 CreateTable 分支遍历 `default_value`（AST 层），
/// 叠加 quick_check 的双模式剥离（文本层）。
///
/// 后端实测：mysql:8.0.46 与 mariadb:11.8.9 **都接受**该类建表语句
///   （RAND()/USER()/UUID()/CONNECTION_ID() 均可用在 DEFAULT 表达式，插入时执行）。
/// 默认黑名单里的 SLEEP/BENCHMARK/LOAD_FILE 在此位置被后端自身拦下
///   （MySQL 3770 / MariaDB 1901），所以默认配置下这是「防御纵深失效」；
///   但只要用户按配置扩展 deny_functions，即为可直接利用的绕过（现已封堵）。
#[test]
fn finding_function_blacklist_bypass_in_create_table() {
    let cfg = WallConfig {
        deny_functions: vec!["RAND".into(), "USER".into(), "SLEEP".into()],
        ..Default::default()
    };
    deny(cfg.clone(), "SELECT RAND()", "forbidden");
    for (sql, f) in [
        (
            "CREATE TABLE t (a VARCHAR(5) DEFAULT 'x\\'', b INT DEFAULT (RAND()))",
            "RAND",
        ),
        (
            "CREATE TABLE t (a VARCHAR(5) DEFAULT 'x\\'', b VARCHAR(60) DEFAULT (USER()))",
            "USER",
        ),
        (
            "CREATE TABLE t (a VARCHAR(5) DEFAULT 'x\\'', b INT DEFAULT (SLEEP(1)))",
            "SLEEP",
        ),
    ] {
        deny(cfg.clone(), sql, &format!("forbidden function: {}", f));
    }
}

/// 契约（原缺口，pool-fixer 已修）：Java Druid 惯用的配置面 `filters: ["wall"]`
/// 无法按名实例化 Filter，曾经是**静默空操作** —— 用户配了防火墙却没有任何墙。
/// 现在 `init()` 硬报错（fail-fast）而不是打日志放过：安全配置的静默失效比报错危险得多。
/// 生产接线方式是 `DruidDataSource::with_filters(...)`（见 e2e_wall_blocks_before_driver）。
#[tokio::test]
async fn config_filters_are_rejected_at_init_not_silently_ignored() {
    let driver = MockDriver::new();
    let exec_calls = driver.exec_calls.clone();
    let ds = DruidDataSource::new(driver, pool_cfg(vec!["wall".into()]));
    match ds.init().await {
        Err(DruidError::Config(msg)) => {
            assert!(
                msg.contains("with_filters"),
                "Config 错误信息应指向正确接线方式，实际: {}",
                msg
            );
        }
        other => panic!("filters=[wall] 必须在 init() fail-fast，实际 {:?}", other),
    }
    // 未成功初始化 → 连接不可交付，驱动从未被调用
    assert!(ds.get_connection().await.is_err());
    assert_eq!(exec_calls.load(Ordering::SeqCst), 0);
    ds.close().await.unwrap();
}

/// 已修复（金丝雀生效后转正）：`--` 后跟 Unicode 空白。
/// 旧词法器用 Rust `char::is_whitespace`（含 U+00A0/U+3000/U+0085）把整行当注释吞掉，
/// `; DROP TABLE t` 不进语句计数；实测 mysql:8.0.46 与 mariadb:11.8.9 对这些字符
/// 一律报语法错误（不视为注释）。druid-sql 词法器已收紧 `--` 注释规则（需 ASCII 空白），
/// 这些输入现在与后端一致地被拒。
#[test]
fn test_dashdash_unicode_whitespace_is_not_a_comment() {
    for sql in [
        "SELECT 1 --\u{00a0}; DROP TABLE users",
        "SELECT 1--\u{00a0}DROP TABLE users",
        "SELECT 1 --\u{3000};DROP TABLE users",
        "SELECT 1--\u{0085}DROP TABLE users",
    ] {
        deny(WallConfig::default(), sql, "");
    }
}

/// 已修复（金丝雀生效后转正）：可执行注释展开深度上限 `MAX_EXEC_COMMENT_DEPTH = 4`。
/// 旧词法器对第 5 层 `/*!` 静默丢弃内容（解析器与墙都看不到）→ 放行；
/// 实测两个后端都不支持嵌套 `/*!`（2 层即 1064）→ 当时不可利用。
/// druid-sql 词法器现已改为超限即词法错误 → 解析失败 → 墙 fail-closed 拒绝。
#[test]
fn test_exec_comment_depth_ceiling_denied() {
    for sql in [
        "SELECT 1 /*! /*! /*! /*! /*! INTO OUTFILE '/tmp/x' */ */ */ */ */",
        "/*! /*! /*! /*! /*! DROP TABLE users */ */ */ */ */",
    ] {
        deny(WallConfig::default(), sql, "");
    }
}

/// 已声明的已知限制（非缺陷，金丝雀）：以下语句在默认配置下被拒，分两类。
/// （a）druid-sql parser 覆盖缺口：SET / SHOW / UNION / ON DUPLICATE KEY /
/// FOR UPDATE / CREATE TABLE ... ENGINE|CHECK —— 逃生开关 `deny_unparsable: false`
/// （放开后仍保留关键字归类、INTO OUTFILE、多语句检查）；
/// （b）刻意拒绝：`USE`（会绕过 deny_schemas，如 `USE mysql` 后 `SELECT * FROM user`）
/// 与 `SET`（改变会话安全状态，如 `SET sql_mode` 影响后续语句的字符串解析语义）。
/// 无状态文本墙无法安全放行，放行需先做专项风险决策。
/// 事务控制语句已修复并放行，其断言在 wall_rules.rs 独立用例中。
#[test]
#[ignore = "已声明的已知限制：parser 覆盖缺口 + USE/SET 刻意拒绝（金丝雀，固化当前行为）"]
fn known_limit_parser_gaps_and_deliberate_denials() {
    for sql in [
        "SET autocommit=0",
        "INSERT INTO t VALUES (1) ON DUPLICATE KEY UPDATE a=1",
        "SELECT * FROM t FOR UPDATE",
        "SELECT a FROM t1 UNION SELECT a FROM t2",
        "CREATE TABLE t (a INT) ENGINE=InnoDB",
        "CREATE TABLE t (a INT, CHECK (a > 0))",
        "SHOW TABLES",
        "USE mydb",
    ] {
        assert!(
            verdict(WallConfig::default(), sql).is_err(),
            "{} 现在被放行了 —— 若是 parser/策略已更新请更新本用例",
            sql
        );
    }
}

// ══════════════ Round 4：bare_text（token 流重建）对抗 ══════════════

/// Round 4 主结论（回归）：匹配文本改由 `druid_sql::parser::lexer::tokenize()` 重建后，
/// 前两轮的全部绕过输入仍被拦（`\'` 致盲、`"O'Brien"`/`` `a'b` `` 致盲、CREATE TABLE
/// DEFAULT 函数黑名单 —— 见上方三个已转正用例，本轮复核全绿）。
///
/// 本用例覆盖**新的渲染面**：词法器把字符串/十六进制字面量当数据（渲染成空格），
/// 其余 token 走 `Display`（注释连内容一起 Debug 渲染，`/*!` 展开进 token 流）。
/// 其中「MySQL 当代码、词法器当数据」的两种形态是重点：
///   `SELECT 1 /* /* */ , secret FROM t` —— MySQL 注释**不嵌套**，`*/` 提前闭合，
///     真正执行的是 `SELECT 1 , secret FROM t`（列引用）；
///   `SELECT /*! secret */ FROM t` —— MySQL 展开可执行注释，`secret` 是列引用。
/// 两者都被拦（token 流不丢内容），未出现反向（少报）。
///
/// mysql:8.0.46 仲裁（库 advdb，`t.secret='V7f21'`）：上面两条 rc=0 且取到列值；
/// `/* secret */` / `-- secret` / `# secret` 在 MySQL 侧是注释或数据，墙拦下属**多报**。
#[test]
fn test_round4_token_render_surface_is_denied() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    for sql in [
        "SELECT `secret` FROM t", // 反引号标识符：MySQL 真执行（同族 `SLEEP`(1) 实测）
        "SELECT 1 /* /* */ , secret FROM t", // 嵌套注释：MySQL 提前闭合 → secret 是列引用
        "SELECT /*! secret */ FROM t", // `/*!` 展开 → secret 是列引用
        "SELECT 1 /* secret */ FROM t", // 注释内容进匹配文本（MySQL 侧是注释 → 多报）
        "SELECT 1 -- secret\nFROM t", // `--` 注释（多报）
        "SELECT 1 # secret\nFROM t", // `#` 不是本仓库注释 → 多报
        "SELECT \"a'b\" , secret FROM t", // 双引号标识符里的 `'` 不再致盲
        "SELECT 'x' , secret FROM t -- '", // 尾部注释里的孤立 `'` 不再致盲
    ] {
        deny(cfg.clone(), sql, "forbidden");
    }
}

/// Round 4 反向核对：token 渲染只会**多报**，不会少报。
/// 下面这批实测在 MySQL 侧合法或纯数据（`rc=0`，输出见注释），墙拦下属于 fail-closed
/// 的过量拦截 —— 可接受；若哪天变成「放行但 MySQL 会执行」才是缺陷。
#[test]
fn non_finding_data_shapes_are_over_denied_only() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    // mysql:8.0.46 实测 rc=0
    for sql in [
        "SELECT X'736563726574' FROM t", // 输出 'secret'（十六进制是数据；解析器不支持 X'' 形式 → 拦）
        "SELECT _utf8'secret' FROM t",   // 输出 'secret'
        "SELECT n'secret' FROM t",       // 输出 'secret'
        "SELECT 'a' 'secret' FROM t",    // 输出 'asecret'（相邻字面量拼接）
        "SELECT 1 # secret\nFROM t",     // 输出 1（MySQL 里 `#` 是注释）
    ] {
        deny(cfg.clone(), sql, "");
    }
    // 字面量（数据）正确放行，证明「多报」不是无差别拦截
    for sql in [
        "SELECT 'secret' FROM t",
        "SELECT \"asecret\" FROM t", // 反引号回显内容但边界判定不命中（多报只在整词时发生）
        "SELECT @'secret' FROM t",   // MySQL：用户变量名，纯数据
        "SELECT 0x736563726574 FROM t", // 输出 'secret'；`0x..` 走 Number token，不误报
    ] {
        assert!(
            verdict(cfg.clone(), sql).is_ok(),
            "字面量里的 secret 是数据，不该拦：{sql}"
        );
    }
}

/// 已知边界（**非本轮回归**）：`<函数名> <注释> (` 形态在 `deny_unparsable:false` 下放行，
/// 而 MySQL 会真正执行该函数调用。
///
/// 机制：`bare_text` 按 token 顺序用空格拼接，`SLEEP/**/(1)` 里函数名与 `(` 之间隔着
/// 注释 token → compact 文本是 `sleep/**/(`，`contains("sleep(")` 不命中；`--`/`#`/换行
/// 变体同理（compact 不跨注释/换行拼接）。默认配置不可利用：这些语句解析失败 →
/// `deny_unparsable=true` → DENY。
///
/// mysql:8.0.46 实测（对照 `SELECT SLEEP(1)` 1168ms）：
///   `SELECT SLEEP/**/(1)`      rc=0 1156ms      `SELECT SLEEP /*x*/ (1)`  rc=0 1181ms
///   ``SELECT `SLEEP`/**/(1)``  rc=0 1221ms      `SELECT SLEEP -- x\n(1)`  rc=0 1274ms
///   `SELECT SLEEP # x\n(1)`    rc=0 1102ms
/// 旧检查器（`git show HEAD:druid-wall/src/checker.rs`）同样是 `"sleep("` 子串匹配、
/// 同样看不见注释拆词 → 这是**既有边界**，不是 token 重建引入的回归。
#[test]
fn non_finding_function_call_split_by_comment_needs_deny_unparsable_false() {
    let lenient = WallConfig {
        deny_unparsable: false,
        ..Default::default()
    };
    for sql in [
        "SELECT SLEEP/**/(1)",
        "SELECT SLEEP /*x*/ (1)",
        "SELECT `SLEEP`/**/(1)",
        "SELECT SLEEP -- x\n(1)",
        "SELECT SLEEP # x\n(1)",
    ] {
        // 默认配置：语法树过不去 → fail-closed
        deny(WallConfig::default(), sql, "unparseable");
        // 边界固化：非默认配置放行（若已收窄请更新本用例）
        assert!(
            verdict(lenient.clone(), sql).is_ok(),
            "边界行为已变化（若已修复请更新本用例）：{sql}"
        );
    }
    // 空白（非注释）分隔时 compact 拼接仍命中
    for sql in ["SELECT SLEEP\n(1)", "SELECT SLEEP (1)", "SELECT SLEEP\t(1)"] {
        deny(WallConfig::default(), sql, "forbidden function");
    }
}

/// 已知边界（主负责人已确认不修）：`tokenize()` 不回传词法错误（`checker.rs` 忽略
/// 返回值），故 quick_check 看不到 `/*!` 嵌套超深；该错误只在 `parse_sql` 里体现。
///
/// 边界验证（本轮实测）：
///   默认配置：`parse_sql` 报错 → `deny_unparsable` → DENY（**不可达**）；
///   `deny_unparsable:false`：放行。但 MySQL 8.0.46 对同输入报 1064（`/*!` 不可嵌套，
///   本仓库文件头已记录 2 层即 1064），载荷不会执行；超深注释**之后**的载荷
///   （`... */ DROP TABLE users`）整条同样 1064 —— 实测 rc=1，DROP 未执行。
#[test]
fn non_finding_deep_exec_comment_requires_deny_unparsable_false() {
    let lenient = WallConfig {
        deny_unparsable: false,
        ..Default::default()
    };
    for sql in [
        "SELECT 1 /*! /*! /*! /*! /*! INTO OUTFILE '/tmp/x' */ */ */ */ */",
        "SELECT 1 /*! /*! /*! /*! /*! X */ */ */ */ */ DROP TABLE users",
    ] {
        deny(WallConfig::default(), sql, "unparseable");
        assert!(
            verdict(lenient.clone(), sql).is_ok(),
            "边界行为已变化（若已收窄请更新本用例）：{sql}"
        );
    }
}
