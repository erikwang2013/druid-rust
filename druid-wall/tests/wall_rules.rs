//! druid-wall 规则测试。
//! 绕过用例一律走 WallProvider::check（真实路径），不再直接调 checker.check 配手工 stmt。
use druid_sql::parse_sql;
use druid_wall::{DenyOperation, WallChecker, WallConfig, WallProvider};

/// 用给定配置检查一条 SQL：放行返回 Ok，拒绝返回违规信息
fn check_sql(cfg: WallConfig, sql: &str) -> Result<(), String> {
    let mut p = WallProvider::new(WallChecker::new(cfg), 64);
    let r = p.check(sql);
    if r.allowed {
        Ok(())
    } else {
        Err(r.violations[0].message.clone())
    }
}

fn default_cfg() -> WallConfig {
    WallConfig::default()
}

// ── 历史绕过输入：默认配置下必须全部拒绝 ──

#[test]
fn test_bypass_inputs_denied_by_default() {
    let cases: &[(&str, &str)] = &[
        // INTO OUTFILE 及其空白/注释/DUMPFILE 变体
        ("SELECT * FROM users INTO OUTFILE '/tmp/x'", "INTO OUTFILE"),
        ("SELECT * FROM users INTO  OUTFILE '/tmp/x'", "INTO OUTFILE"),
        (
            "SELECT * FROM users INTO/**/OUTFILE '/tmp/x'",
            "INTO OUTFILE",
        ),
        ("SELECT * FROM users INTO\nOUTFILE '/tmp/x'", "INTO OUTFILE"),
        ("SELECT * FROM users INTO DUMPFILE '/tmp/x'", "INTO OUTFILE"),
        // parser 未建模的语句类型（含前导注释/注释分隔/小写）
        ("TRUNCATE TABLE users", "TRUNCATE denied"),
        ("ALTER TABLE t ADD c INT", "ALTER TABLE denied"),
        ("/* c */ TRUNCATE TABLE users", "TRUNCATE denied"),
        ("truncate/**/table users", "TRUNCATE denied"),
        // 嵌套函数与注释绕过
        ("SELECT COALESCE(SLEEP(5),0)", "forbidden"),
        ("SELECT id FROM users WHERE id = ABS(SLEEP(5))", "forbidden"),
        ("SELECT COUNT(SLEEP(1)) FROM users", "forbidden"),
        ("SELECT SLEEP/**/(5)", "unparseable"),
        ("SELECT 1--SLEEP(5)", "forbidden"),
        ("SELECT /*!50000 SLEEP(5) */ 1", "forbidden"),
    ];
    for (sql, expect) in cases {
        match check_sql(default_cfg(), sql) {
            Err(msg) => assert!(
                msg.contains(expect),
                "SQL {:?} 被拒但信息 {:?} 不含 {:?}",
                sql,
                msg,
                expect
            ),
            Ok(()) => panic!("SQL {:?} 不应放行", sql),
        }
    }
}

/// fail-closed 的对称面：parser 支持的合法 MySQL 语法必须放行，不能被误伤。
/// `LIMIT offset, count` 曾因 Comma 无人消费而解析失败，被 deny_unparsable 连带拒绝；
/// parser 补上该语法后应恢复放行。
#[test]
fn test_valid_mysql_syntax_is_not_false_positive() {
    for sql in [
        "SELECT 1 LIMIT 0,10",
        "SELECT 1 LIMIT 10 OFFSET 5",
        "SELECT * FROM t WHERE name = @uid",
        "SELECT @@version",
    ] {
        assert!(
            check_sql(default_cfg(), sql).is_ok(),
            "合法 SQL {:?} 被误伤",
            sql
        );
    }
}

#[test]
fn test_wall_filter_denies_truncate_end_to_end() {
    use druid_filter::{Filter, FilterContext};
    let f = druid_wall::WallFilter::new(WallConfig::default());
    let ctx = FilterContext::new("t").with_sql("TRUNCATE TABLE users");
    assert!(f.statement_execute_before(&ctx).is_err());
}

// ── 操作拦截（含 parser 未建模的语句类型按首关键字归类） ──

#[test]
fn test_deny_operations_all_reachable() {
    let cases: &[(DenyOperation, &str, &str)] = &[
        (DenyOperation::Select, "SELECT 1", "SELECT denied"),
        (
            DenyOperation::Insert,
            "INSERT INTO users (id) VALUES (1)",
            "INSERT denied",
        ),
        (
            DenyOperation::Update,
            "UPDATE users SET name='x' WHERE id=1",
            "UPDATE denied",
        ),
        (
            DenyOperation::Delete,
            "DELETE FROM users WHERE id=1",
            "DELETE denied",
        ),
        (
            DenyOperation::Truncate,
            "TRUNCATE TABLE users",
            "TRUNCATE denied",
        ),
        (
            DenyOperation::DropTable,
            "DROP TABLE users",
            "DROP TABLE denied",
        ),
        (
            DenyOperation::AlterTable,
            "ALTER TABLE t ADD c INT",
            "ALTER TABLE denied",
        ),
        (
            DenyOperation::CreateTable,
            "CREATE TABLE t (id INT)",
            "CREATE TABLE denied",
        ),
        (
            DenyOperation::CreateIndex,
            "CREATE INDEX idx ON t(c)",
            "CREATE INDEX denied",
        ),
        (
            DenyOperation::Grant,
            "GRANT ALL ON *.* TO u",
            "GRANT denied",
        ),
        (
            DenyOperation::Revoke,
            "REVOKE ALL ON *.* FROM u",
            "REVOKE denied",
        ),
        (DenyOperation::Call, "CALL proc(1)", "CALL denied"),
        (DenyOperation::Execute, "EXECUTE stmt", "EXECUTE denied"),
    ];
    for (op, sql, expect) in cases {
        let cfg = WallConfig {
            deny_operations: vec![op.clone()],
            ..Default::default()
        };
        assert_eq!(
            check_sql(cfg, sql).unwrap_err(),
            *expect,
            "DenyOperation::{:?} 未生效",
            op
        );
    }
}

#[test]
fn test_default_policy_allow_and_require_where() {
    let d = default_cfg;
    assert!(check_sql(d(), "SELECT id FROM users WHERE id=1").is_ok());
    assert!(check_sql(d(), "CREATE TABLE t (id INT)").is_ok());
    assert!(check_sql(d(), "INSERT INTO users (id) VALUES (1)").is_ok());
    assert!(check_sql(d(), "UPDATE users SET name='x' WHERE id=1").is_ok());
    assert!(check_sql(d(), "DELETE FROM users WHERE id=1").is_ok());
    // 默认 update_delete_require_where = true
    assert_eq!(
        check_sql(d(), "UPDATE users SET name='x'").unwrap_err(),
        "UPDATE without WHERE"
    );
    assert_eq!(
        check_sql(d(), "DELETE FROM users").unwrap_err(),
        "DELETE without WHERE"
    );
    // 关掉后放行
    let cfg = WallConfig {
        update_delete_require_where: false,
        ..Default::default()
    };
    assert!(check_sql(cfg.clone(), "UPDATE users SET name='x'").is_ok());
    assert!(check_sql(cfg, "DELETE FROM users").is_ok());
}

// ── 多语句（allow_multi_statements，默认 false） ──

#[test]
fn test_multi_statement_policy() {
    assert!(check_sql(default_cfg(), "SELECT 1; SELECT 2")
        .unwrap_err()
        .contains("multiple statements"));
    // 单条 + 结尾分号不受影响
    assert!(check_sql(default_cfg(), "SELECT 1;").is_ok());
    let cfg = WallConfig {
        allow_multi_statements: true,
        ..Default::default()
    };
    assert!(check_sql(cfg.clone(), "SELECT 1; SELECT 2").is_ok());
    // 多语句放行后，其中任一条违规仍要拦
    assert!(check_sql(cfg, "SELECT 1; DROP TABLE users").is_err());
}

// ── schema 黑名单（deny_schemas） ──

#[test]
fn test_deny_schemas() {
    let cfg = WallConfig {
        deny_schemas: vec!["mysql".into()],
        ..Default::default()
    };
    assert_eq!(
        check_sql(cfg.clone(), "SELECT * FROM mysql.user").unwrap_err(),
        "forbidden schema: mysql"
    );
    // 大小写不敏感
    assert!(check_sql(cfg.clone(), "SELECT * FROM MySQL.user").is_err());
    // JOIN、FROM 子查询、WHERE 子查询里的 schema 同样拦
    assert!(check_sql(cfg.clone(), "SELECT * FROM t JOIN mysql.user ON t.id=1").is_err());
    assert!(check_sql(cfg.clone(), "SELECT * FROM (SELECT * FROM mysql.user) x").is_err());
    assert!(check_sql(
        cfg.clone(),
        "SELECT * FROM t WHERE x IN (SELECT * FROM mysql.user)"
    )
    .is_err());
    // 未配置时放行
    assert!(check_sql(default_cfg(), "SELECT * FROM mysql.user").is_ok());
}

// ── 解析失败策略（deny_unparsable） ──

#[test]
fn test_deny_unparsable_toggle() {
    // 默认：类型无法识别 → 拒绝。
    // 注意别拿 EXPLAIN 当样板——parser 补上前缀剥离后它会被放行（见
    // test_valid_mysql_syntax_is_not_false_positive）；这里用一条任何方言都不会合法的输入。
    assert!(check_sql(default_cfg(), "NOT VALID SQL !!!")
        .unwrap_err()
        .contains("unparseable"));
    // 全部放开才允许
    let lax = WallConfig {
        deny_unparsable: false,
        deny_operations: vec![],
        ..Default::default()
    };
    assert!(check_sql(lax.clone(), "NOT VALID SQL !!!").is_ok());
    // 只关 deny_unparsable、保留 deny_operations：无法归类的语句仍 fail-closed
    let cfg = WallConfig {
        deny_unparsable: false,
        ..Default::default()
    };
    assert!(check_sql(cfg.clone(), "NOT VALID SQL !!!").is_err());
    // 可归类为 SELECT 的不可解析 SQL（parser 尚不支持 LIMIT a,b）放行
    assert!(check_sql(cfg, "SELECT 1 LIMIT 0,10").is_ok());
}

#[test]
fn test_into_outfile_allow_opt_in() {
    // 默认拒绝
    assert!(
        check_sql(default_cfg(), "SELECT * FROM users INTO DUMPFILE '/tmp/x'")
            .unwrap_err()
            .contains("INTO OUTFILE")
    );
    // 放开该能力需要同时允许不可解析 SQL（否则仍按 unparseable 拒绝）
    let cfg = WallConfig {
        select_into_outfile_allow: true,
        deny_unparsable: false,
        ..Default::default()
    };
    assert!(check_sql(cfg, "SELECT * FROM users INTO OUTFILE '/tmp/x'").is_ok());
}

#[test]
fn test_max_sql_length_checked_before_parse() {
    let cfg = WallConfig {
        max_sql_length: 20,
        ..Default::default()
    };
    // 超长且无法解析：长度检查必须在解析之前生效
    let long = "SELECT 1 LIMIT 0,10 WITH GARBAGE trailing junk";
    assert!(check_sql(cfg, long).unwrap_err().contains("too long"));
}

#[test]
fn test_disabled_wall_passes_everything() {
    let cfg = WallConfig {
        enabled: false,
        ..Default::default()
    };
    for sql in [
        "DROP TABLE users",
        "TRUNCATE TABLE users",
        "SELECT SLEEP(5)",
        "NOT VALID SQL !!!",
    ] {
        assert!(check_sql(cfg.clone(), sql).is_ok(), "{} 不应被拦", sql);
    }
}

// ── AST 递归：函数参数 / 聚合参数（checker 层，验证是 AST 而非纯文本命中） ──

#[test]
fn test_ast_visitor_recurses_into_function_args() {
    let c = WallChecker::new(WallConfig::default());
    for sql in [
        "SELECT COALESCE(SLEEP(5),0)",
        "SELECT id FROM users WHERE id=ABS(SLEEP(5))",
        "SELECT COUNT(SLEEP(1)) FROM users",
    ] {
        let stmts = parse_sql(sql).expect("should parse");
        let r = c.check(sql, &stmts[0]);
        assert!(!r.allowed, "{} 不应放行", sql);
        assert_eq!(r.violations[0].message, "forbidden function: SLEEP");
    }
    // 相似函数名不误伤
    let stmts = parse_sql("SELECT SLEEPLESS(1)").unwrap();
    assert!(c.check("SELECT SLEEPLESS(1)", &stmts[0]).allowed);
}

// ── quick_check（纯文本预检） ──

#[test]
fn test_quick_check_boundaries() {
    let c = WallChecker::new(WallConfig::default());
    assert!(!c.quick_check("select sleep (1)").allowed); // 括号前带空格同样拦截
    assert!(!c.quick_check("SELECT SLEEP(10)").allowed);
    assert!(c.quick_check("SELECT * FROM sleeping_table").allowed);
    assert!(c.quick_check("SELECT 'sleep(1)'").allowed); // 字符串字面量不拦截

    let cfg = WallConfig {
        deny_keywords: vec!["drop".into()],
        ..Default::default()
    };
    let c = WallChecker::new(cfg);
    assert!(c.quick_check("SELECT * FROM dropdown").allowed); // 词边界
    assert!(
        c.quick_check("SELECT * FROM x WHERE y='not drop here'")
            .allowed
    );
    assert!(!c.quick_check("SELECT * FROM users DROP").allowed);
}

#[test]
fn test_quick_check_sql_too_long() {
    let cfg = WallConfig {
        max_sql_length: 10,
        ..Default::default()
    };
    let c = WallChecker::new(cfg);
    let r = c.quick_check("SELECT 12345");
    assert!(!r.allowed);
    assert_eq!(r.violations[0].message, "SQL too long");
}

// ── 转义引号致盲 / CREATE TABLE 默认值 / 事务控制安全通道 ──

/// `\'` 不能致盲 deny_keywords：旧实现只数 `'` 奇偶，单个 `\'` 即可把其后文本
/// 全当字符串跳过（deny_keywords 只在 quick_check 检查，没有 AST 兜底）。
#[test]
fn test_escaped_quote_cannot_blind_deny_keywords() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    for sql in [
        "SELECT 'a\\'', secret FROM t",
        "SELECT 'a\\'', (SELECT secret FROM t)",
    ] {
        assert_eq!(
            check_sql(cfg.clone(), sql).unwrap_err(),
            "forbidden: secret",
            "{} 应命中 deny_keywords",
            sql
        );
    }
    // 合法转义不误伤：默认 sql_mode 下 `'it\'s'` 是完整字符串
    assert!(check_sql(cfg.clone(), "SELECT 'it\\'s' FROM t").is_ok());
    assert!(check_sql(cfg, "SELECT 'it''s' FROM t").is_ok());
}

/// Round 3：`"..."`（默认 sql_mode 下的字符串字面量）与 `` `...` ``（标识符）里的
/// 奇数个 `'` 不得再让其后文本对 deny_keywords 失明 —— 真机实测 `"O'Brien"` 版本
/// 可执行并改数据。根因是 checker 自建引号扫描；现在匹配文本由词法 token 重建
/// （见 `checker.rs::bare_text`），引号形态的演进只改词法器一处。
#[test]
fn test_double_quote_and_backtick_cannot_blind_deny_keywords() {
    let cfg = WallConfig {
        deny_keywords: vec!["secret".into()],
        ..Default::default()
    };
    for sql in [
        "SELECT \"O'Brien\" , secret FROM t",
        "SELECT `a'b` , secret FROM t",
        "SELECT \"O'Brien\" , `a'b` , secret FROM t",
        "UPDATE t SET name = \"O'Brien\" WHERE secret = 42",
        "DELETE FROM t WHERE name = \"O'Brien\" OR secret = 42",
    ] {
        assert_eq!(
            check_sql(cfg.clone(), sql).unwrap_err(),
            "forbidden: secret",
            "{} 应命中 deny_keywords",
            sql
        );
    }
    // 数据位置不误伤：单引号字符串里的单词不命中
    assert!(check_sql(cfg.clone(), "SELECT 'the secret' FROM t").is_ok());
    // 双引号内容按保守口径**仍参与**匹配：`` `SLEEP`(1) ``（及 ANSI_QUOTES 下的
    // `"SLEEP"(1)`）真机可执行，引号包着的标识符不能当数据跳过；
    // 代价是默认 sql_mode 下 `"..."` 字符串里的关键词也会命中（宁可误报）
    assert_eq!(
        check_sql(cfg, "SELECT \"the secret\" FROM t").unwrap_err(),
        "forbidden: secret"
    );
}

/// 两种 sql_mode 下同一段文本的字符串边界不同、执行语义也不同，必须双向验证 ——
/// 这不是冗余扫描（见 checker.rs `strip_string_literals` 的 ⚠️ 注释）。
/// 判定表：
/// - 默认未闭合 + NBE 闭合 → NBE 下分歧区间是**代码**（`'a\', SLEEP(1), 'x'`）→ fail-closed
/// - 默认闭合 + NBE 未闭合 → 默认模式下是合法完整字符串（`'it\'s'`）→ 放行
/// - 两模式都未闭合 → 真实后端均报 1064（见 adversarial 的 pathological 用例）→ 保持放行
#[test]
fn test_sql_mode_divergence_on_escaped_quote_fails_closed() {
    assert!(check_sql(default_cfg(), "SELECT 'a\\', SLEEP(1), 'x'")
        .unwrap_err()
        .contains("ambiguous string literal"));
    assert!(check_sql(default_cfg(), "SELECT 'it\\'s' FROM t").is_ok());
    assert!(check_sql(default_cfg(), "SELECT '").is_ok());
}

/// CREATE TABLE 的列默认值表达式必须进函数黑名单。
/// MySQL/MariaDB 实测允许 `DEFAULT (RAND())`/`DEFAULT (USER())` 并在 INSERT 时求值，
/// 只在 quick_check 里扫文本会漏（叠加 `\'` 致盲时更是完全失明）。
#[test]
fn test_create_table_default_value_checked() {
    let cfg = WallConfig {
        deny_functions: vec!["RAND".into(), "SLEEP".into()],
        ..Default::default()
    };
    // checker 层单独验证 AST 分支（该路径不经过 quick_check）
    let c = WallChecker::new(cfg.clone());
    let sql = "CREATE TABLE t (b INT DEFAULT (RAND()))";
    let stmts = parse_sql(sql).expect("should parse");
    let r = c.check(sql, &stmts[0]);
    assert!(!r.allowed, "CREATE TABLE DEFAULT 未被 AST 遍历");
    assert_eq!(r.violations[0].message, "forbidden function: RAND");
    // 真实路径（含 `\'` 致盲变体：quick_check 与 AST 两层都必须正确）
    assert!(check_sql(
        cfg,
        "CREATE TABLE t (a VARCHAR(5) DEFAULT 'x\\'', b INT DEFAULT (SLEEP(1)))"
    )
    .unwrap_err()
    .contains("forbidden function: SLEEP"));
}

/// 事务控制安全通道：parser 未建模且不承载可拒绝语义 → 放行（否则真实应用无法开事务）；
/// 但精确形状匹配，不得成为夹带载荷的载体。
#[test]
fn test_transaction_control_allowed_without_smuggling() {
    for sql in [
        "BEGIN",
        "BEGIN WORK",
        "COMMIT",
        "COMMIT;",
        "ROLLBACK",
        "START TRANSACTION",
        "START TRANSACTION READ ONLY",
        "START TRANSACTION WITH CONSISTENT SNAPSHOT",
        "SAVEPOINT sp1",
        "ROLLBACK TO SAVEPOINT sp1",
        "RELEASE SAVEPOINT sp1",
    ] {
        assert!(check_sql(default_cfg(), sql).is_ok(), "{} 应放行", sql);
    }
    for sql in [
        "BEGIN; DROP TABLE users",
        "SAVEPOINT a; DROP TABLE users",
        "ROLLBACK TO SAVEPOINT sp1; DROP TABLE users",
        "START TRANSACTION; TRUNCATE TABLE users",
    ] {
        assert!(check_sql(default_cfg(), sql).is_err(), "{} 不得放行", sql);
    }
    // 明确不放行（待评估风险后再定）：USE 可绕过 deny_schemas，SET 能改会话安全状态
    assert!(check_sql(default_cfg(), "USE mysql").is_err());
    assert!(check_sql(default_cfg(), "SET SESSION sql_mode=''").is_err());
}
