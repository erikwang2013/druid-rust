//! 对抗验证（druid-sql 侧）：`format.rs` 的往返语义保真
//!
//! 判据（与任务一致）：`parse → format → parse` 必须语义不变 —— 以“再次格式化得到同一字符串”
//! 为不动点判据，并对任务点名的输入额外给出**手工硬编码**的期望输出。
//!
//! 本文件只验证与取证，**不修改任何 src/**。`FAIL_*` 前缀 = 真实缺陷证据（失败用例，置顶）。

// `FAIL_` 前缀是刻意保留的证据命名（失败用例置顶），不是笔误
#![allow(non_snake_case)]

use druid_sql::ast::*;
use druid_sql::{format_expr, format_statement, parse_sql};

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 往返探测：Ok(格式化结果) 表示成立；Err(说明) 表示不成立（含实测输出）
fn roundtrip(sql: &str) -> Result<String, String> {
    let first = parse_sql(sql).map_err(|e| format!("首次解析失败: {e}"))?;
    if first.len() != 1 {
        return Err(format!("语句条数 = {}", first.len()));
    }
    let out1 = format_statement(&first[0]);
    let second = match parse_sql(&out1) {
        Ok(s) => s,
        Err(e) => return Err(format!("格式化结果 {out1:?} 重解析失败: {e}")),
    };
    let out2 = format_statement(&second[0]);
    if out1 != out2 {
        return Err(format!("不是不动点: {out1:?} → {out2:?}"));
    }
    Ok(out1)
}

fn assert_roundtrip(sql: &str) {
    match roundtrip(sql) {
        Ok(_) => {}
        Err(why) => panic!("{sql:?} 往返失败: {why}"),
    }
}

/// 格式化表达式 → 重新解析回表达式（比对语义）
fn reparse_expr(formatted: &str) -> SQLExpr {
    let stmts = parse_sql(&format!("SELECT {formatted}"))
        .unwrap_or_else(|e| panic!("重解析 {formatted:?} 失败: {e}"));
    let SQLStatement::Select(s) = &stmts[0] else {
        panic!("不是 SELECT: {formatted:?}")
    };
    match &s.columns[0] {
        SelectItem::Expr(e, None) => e.clone(),
        other => panic!("列项形态意外: {other:?}（{formatted:?}）"),
    }
}

// ---------------------------------------------------------------------------
// FAIL：往返不成立的形态（真实缺陷证据）
// ---------------------------------------------------------------------------

/// 攻击尝试（未命中）：词法把 `N'…'` / `X'…'` 当字面量前缀，若格式化把函数名直接贴到引号上，
/// 输出会被重新词法化成本字面量。实测不会发生 —— 函数调用固定输出 `名(`，括号隔开了引号，
/// 且 format.rs 中所有 identifier 输出点后跟的都是空格 / `=` / `.` / `(`，不存在“标识符紧邻引号”的拼法。
#[test]
fn attack_function_name_adjacent_to_quote_not_relexable_as_literal() {
    assert_eq!(roundtrip("SELECT N ('a')").unwrap(), "SELECT N('a')");
    assert_eq!(roundtrip("SELECT X ('AB')").unwrap(), "SELECT X('AB')");
    // 反向确认词法前缀确实存在（若有一天输出能拼出 N'...' 就会命中该路径）
    assert_eq!(
        druid_sql::parser::lexer::tokenize("N'x'"),
        vec![
            druid_sql::token::Token::StringLit("x".into()),
            druid_sql::token::Token::Eof
        ]
    );
}

/// `@'a b'` 是词法明确支持的写法，但 `format_expr` 对 Variable 一律原样输出，
/// 带空格的变量名重新解析后被拆成「变量 + 别名」，且**不报错**（静默改变语义）。
#[test]
fn FAIL_quoted_variable_with_space_silently_changes_ast() {
    let sql = "SELECT @'a b' FROM t";
    let out = roundtrip(sql).expect("往返本身应当成立到重解析阶段");
    let first = parse_sql(sql).unwrap();
    let second = parse_sql(&out).unwrap();
    assert_eq!(
        first, second,
        "格式化 {out:?} 后 AST 改变：\n  原: {first:?}\n  新: {second:?}"
    );
}

/// `format.rs` 把 `data_type` 原样输出，而 parser 在无法识别的类型位会写入 Token 的
/// **Debug 字符串**（`parser/mod.rs:1051`）。Debug 不是 SQL，于是格式化输出不可解析。
#[test]
fn FAIL_data_type_carrying_debug_text_breaks_roundtrip() {
    let cases = ["CREATE TABLE t (id 'x')", "CREATE TABLE t (id 5)"];
    let broken: Vec<String> = cases
        .iter()
        .filter_map(|sql| {
            roundtrip(sql)
                .err()
                .map(|why| format!("  - {sql:?} → {why}"))
        })
        .collect();
    assert!(
        broken.is_empty(),
        "data_type 含 Debug 文本导致格式化结果无法重解析:\n{}",
        broken.join("\n")
    );
}

/// 外部构造的 AST：`format_operand` 只为 BinaryOp 补括号，Between/InList/Like/IsNull 的
/// **操作数**直接走 `format_expr`，没有同类处理 → 父运算符更松时括号丢失，语义改变。
#[test]
fn FAIL_handbuilt_ast_loses_parens_around_operand() {
    let or_ab = || SQLExpr::BinaryOp {
        left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
        op: BinaryOpType::Or,
        right: Box::new(SQLExpr::Identifier(vec!["b".into()])),
    };
    let cases: Vec<(&str, SQLExpr, &str)> = vec![
        (
            "Between 的左侧操作数",
            SQLExpr::Between {
                expr: Box::new(or_ab()),
                low: Box::new(SQLExpr::NumberLiteral("1".into())),
                high: Box::new(SQLExpr::NumberLiteral("2".into())),
                not: false,
            },
            "(a OR b) BETWEEN 1 AND 2",
        ),
        (
            "InList 的左侧操作数",
            SQLExpr::InList {
                expr: Box::new(or_ab()),
                list: vec![
                    SQLExpr::NumberLiteral("1".into()),
                    SQLExpr::NumberLiteral("2".into()),
                ],
                not: false,
            },
            "(a OR b) IN (1, 2)",
        ),
        (
            "Like 的左侧操作数",
            SQLExpr::Like {
                expr: Box::new(or_ab()),
                pattern: Box::new(SQLExpr::StringLiteral("x".into())),
                not: false,
            },
            "(a OR b) LIKE 'x'",
        ),
        (
            "IsNull 的操作数",
            SQLExpr::IsNull {
                expr: Box::new(or_ab()),
                not: false,
            },
            "(a OR b) IS NULL",
        ),
    ];

    let broken: Vec<String> = cases
        .iter()
        .filter_map(|(label, expr, expected)| {
            let got = format_expr(expr);
            (got != *expected).then(|| format!("  - {label}: 期望 {expected:?}，实测 {got:?}"))
        })
        .collect();
    assert!(
        broken.is_empty(),
        "无括号保护的父运算符节点（重解析后分组改变）:\n{}",
        broken.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 任务点名的输入：硬编码期望输出（语义由手推判定，不靠 AST 自证）
// ---------------------------------------------------------------------------

#[test]
fn required_cases_exact_output() {
    let cases: &[(&str, &str)] = &[
        ("SELECT 'O''Brien'", "SELECT 'O''Brien'"),
        (r"SELECT 'a\\b'", r"SELECT 'a\\b'"),
        ("SELECT 'x'' OR 1=1 -- '", "SELECT 'x'' OR 1=1 -- '"),
        (
            "SELECT `select` FROM `my table`",
            "SELECT `select` FROM `my table`",
        ),
        (
            "SELECT \"total amount\" FROM t",
            "SELECT `total amount` FROM t",
        ),
        (
            "CREATE TABLE t (id INT PRIMARY KEY, name VARCHAR(10) NOT NULL DEFAULT 'x')",
            "CREATE TABLE t (id INT PRIMARY KEY, name VARCHAR(10) NOT NULL DEFAULT 'x')",
        ),
        ("SELECT (a+b)*c", "SELECT (a + b) * c"),
        ("SELECT a-(b-c)", "SELECT a - (b - c)"),
        ("SELECT a AND b AND c", "SELECT a AND b AND c"),
        ("SELECT -(a+b)", "SELECT -(a + b)"),
    ];
    for (sql, expected) in cases {
        assert_eq!(
            &roundtrip(sql).unwrap_or_else(|e| panic!("{sql:?}: {e}")),
            expected,
            "输入 {sql:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 对抗输入：字面量 / 标识符
// ---------------------------------------------------------------------------

#[test]
fn literal_escaping_roundtrips() {
    let cases = [
        r"SELECT '\''",      // 值 = 一个单引号
        r"SELECT '\\'",      // 值 = 一个反斜杠
        r"SELECT 'a\'b'",    // 值含反斜杠转义的引号
        r"SELECT 'a''b'",    // 值含双写引号
        r"SELECT '\\''",     // 值 = 反斜杠 + 引号
        r"SELECT 'a\\b''c'", // 反斜杠与双写引号混合
        "SELECT ''",
        "SELECT '-- 不是注释'",
        "SELECT '#?%'",
    ];
    for sql in cases {
        assert_roundtrip(sql);
    }

    // 单引号/反斜杠必须按 MySQL 语义转义（手推期望）
    assert_eq!(
        format_expr(&SQLExpr::StringLiteral("it's".into())),
        "'it''s'"
    );
    assert_eq!(
        format_expr(&SQLExpr::StringLiteral(r"a\b".into())),
        r"'a\\b'"
    );
    assert_eq!(
        format_expr(&SQLExpr::StringLiteral(r"\'".into())),
        r"'\\'''"
    );
    assert_eq!(format_expr(&SQLExpr::StringLiteral(r"".into())), "''");
    // 结尾是反斜杠：不得吃掉闭合引号
    assert_eq!(
        format_expr(&SQLExpr::StringLiteral(r"tail\".into())),
        r"'tail\\'"
    );
    let back = reparse_expr(&format_expr(&SQLExpr::StringLiteral(r"tail\".into())));
    assert_eq!(back, SQLExpr::StringLiteral(r"tail\".into()));
}

#[test]
fn identifier_quoting_roundtrips() {
    let cases = [
        "SELECT `a``b` FROM t", // 标识符内含反引号（转义为双反引号）
        r"SELECT `a\b` FROM t", // 标识符内含反斜杠（引号内不转义）
        "SELECT `a\"b` FROM t", // 内含双引号
        "SELECT `1abc` FROM t", // 数字开头
        "SELECT `a b`.`c d` FROM `e f`",
        "SELECT `order` FROM `group`",
        "SELECT t.`total amount` FROM t",
        "SELECT t.* FROM t",
        "SELECT 1 AS `select`",
        "SELECT `中文列` FROM `中文表`",
        "UPDATE `my table` SET `order` = 'x' WHERE `select` = 1",
        "INSERT INTO `my table` (`odd col`) VALUES ('a''b')",
        "SELECT * FROM `t` `别名`",
        "DELETE FROM `a``b` WHERE `x y` = 1",
        "DROP TABLE `select`",
        "SELECT COUNT(*) AS `count of rows` FROM t",
    ];
    for sql in cases {
        assert_roundtrip(sql);
    }

    // 关键字 / 非普通标识符必须加反引号，否则重解析语义改变
    assert_eq!(
        format_expr(&SQLExpr::Identifier(vec!["select".into()])),
        "`select`"
    );
    assert_eq!(
        format_expr(&SQLExpr::Identifier(vec!["total amount".into()])),
        "`total amount`"
    );
    assert_eq!(
        format_expr(&SQLExpr::Identifier(vec!["a".into(), "b".into()])),
        "a.b"
    );
    assert_eq!(
        format_expr(&SQLExpr::Identifier(vec!["t".into(), "*".into()])),
        "t.*"
    );
}

// ---------------------------------------------------------------------------
// 对抗输入：运算符优先级与结合性
// ---------------------------------------------------------------------------

#[test]
fn operator_precedence_roundtrips() {
    let cases = [
        "SELECT ((a))",
        "SELECT a-b-c",
        "SELECT a/b/c",
        "SELECT a%b%c",
        "SELECT a - (b + c)",
        "SELECT a - (b - c)",
        "SELECT (a + b) * c",
        "SELECT a * (b + c)",
        "SELECT a / (b * c)",
        "SELECT (a AND b) OR c",
        "SELECT a OR (b AND c)",
        "SELECT NOT a = 1",
        "SELECT NOT NOT a",
        "SELECT NOT (a AND b)",
        "SELECT - -1",
        "SELECT -(-1)",
        "SELECT a || b || c",
        "SELECT a || (b || c)",
        "SELECT a BETWEEN 1 AND 2 AND b",
        "SELECT (a BETWEEN 1 AND 2) OR c",
        "SELECT a IN (1, 2) OR b",
        "SELECT a NOT IN (1, 2) AND b",
        "SELECT a NOT LIKE 'x%' OR b",
        "SELECT a IS NULL AND b IS NOT NULL",
        "SELECT 1--SLEEP(5)",
        "SELECT CASE WHEN a > 1 THEN 'x' ELSE 'y' END FROM t",
    ];
    for sql in cases {
        assert_roundtrip(sql);
    }
}

/// 外部构造的 AST 里，结合律/优先级的补括号必须成立（同类断言在修复前会失败的那批）。
///
/// 注意判据：括号在重解析后会变成 `Nested` 节点，所以“重解析结果”要按
/// “格式化文本所表达的分组”来写期望值，而不是照抄原 AST。
#[test]
fn handbuilt_ast_precedence_parens() {
    let ident = |n: &str| SQLExpr::Identifier(vec![n.into()]);
    let bin = |l: SQLExpr, op: BinaryOpType, r: SQLExpr| SQLExpr::BinaryOp {
        left: Box::new(l),
        op,
        right: Box::new(r),
    };
    let nested = |e: SQLExpr| SQLExpr::Nested(Box::new(e));

    // 断言：文本完全一致 + 重解析分组正确 + 再格式化是不动点
    let check = |e: &SQLExpr, expected: &str, reparsed: &SQLExpr| {
        assert_eq!(format_expr(e), expected, "格式化文本不符");
        let back = reparse_expr(expected);
        assert_eq!(&back, reparsed, "{expected:?} 重解析后的分组与期望不符");
        assert_eq!(
            format_expr(&back),
            expected,
            "{expected:?} 再格式化不是不动点"
        );
    };

    // (a + b) * c：子表达式更松必须补括号
    let mul = bin(
        bin(ident("a"), BinaryOpType::Plus, ident("b")),
        BinaryOpType::Mul,
        ident("c"),
    );
    check(
        &mul,
        "(a + b) * c",
        &bin(
            nested(bin(ident("a"), BinaryOpType::Plus, ident("b"))),
            BinaryOpType::Mul,
            ident("c"),
        ),
    );

    // a - (b - c)：同级右侧且不满足结合律，必须补括号
    let sub_r = bin(
        ident("a"),
        BinaryOpType::Minus,
        bin(ident("b"), BinaryOpType::Minus, ident("c")),
    );
    check(
        &sub_r,
        "a - (b - c)",
        &bin(
            ident("a"),
            BinaryOpType::Minus,
            nested(bin(ident("b"), BinaryOpType::Minus, ident("c"))),
        ),
    );

    // (a - b) - c：同级左侧天然左结合，不需要括号
    let sub_l = bin(
        bin(ident("a"), BinaryOpType::Minus, ident("b")),
        BinaryOpType::Minus,
        ident("c"),
    );
    check(&sub_l, "a - b - c", &sub_l);

    // a * (b + c)：右侧更松同样补括号
    let mul_r = bin(
        ident("a"),
        BinaryOpType::Mul,
        bin(ident("b"), BinaryOpType::Plus, ident("c")),
    );
    check(
        &mul_r,
        "a * (b + c)",
        &bin(
            ident("a"),
            BinaryOpType::Mul,
            nested(bin(ident("b"), BinaryOpType::Plus, ident("c"))),
        ),
    );

    // a AND (b AND c)：结合律成立，允许省略括号，重解析后分组变化但等价
    let and_r = bin(
        ident("a"),
        BinaryOpType::And,
        bin(ident("b"), BinaryOpType::And, ident("c")),
    );
    check(
        &and_r,
        "a AND b AND c",
        &bin(
            bin(ident("a"), BinaryOpType::And, ident("b")),
            BinaryOpType::And,
            ident("c"),
        ),
    );

    // a OR (b AND c)：右侧更紧，不需要括号
    let or_r = bin(
        ident("a"),
        BinaryOpType::Or,
        bin(ident("b"), BinaryOpType::And, ident("c")),
    );
    check(&or_r, "a OR b AND c", &or_r);

    // -(a + b)：一元负号作用在二元表达式上
    let neg = SQLExpr::UnaryOp {
        op: UnaryOpType::Neg,
        expr: Box::new(bin(ident("a"), BinaryOpType::Plus, ident("b"))),
    };
    check(
        &neg,
        "-(a + b)",
        &SQLExpr::UnaryOp {
            op: UnaryOpType::Neg,
            expr: Box::new(nested(bin(ident("a"), BinaryOpType::Plus, ident("b")))),
        },
    );
}

// ---------------------------------------------------------------------------
// 其它 DDL/DML 往返 + 已知观察项
// ---------------------------------------------------------------------------

#[test]
fn ddl_and_dml_roundtrips() {
    let cases = [
        "CREATE TABLE IF NOT EXISTS `t` (id INT PRIMARY KEY, name VARCHAR(10) NOT NULL DEFAULT 'x')",
        "CREATE TABLE t (a INT NOT NULL, b VARCHAR(20) DEFAULT 'it''s')",
        "CREATE TABLE t (a INT DEFAULT -1, b INT NULL)",
        "DROP TABLE IF EXISTS `my table`",
        "INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)",
        "REPLACE INTO t VALUES (1)",
        "UPDATE t SET a = 1, b = a + 1 WHERE `order` = 3",
        "DELETE FROM t WHERE x = 1",
        "SELECT * FROM t",
        "WITH x AS (SELECT 1) SELECT * FROM x",
        "SELECT DISTINCT a, COUNT(*) AS cnt FROM db.t x LEFT JOIN u ON x.id = u.id \
         WHERE x.a > 1 AND x.b IS NOT NULL GROUP BY a HAVING COUNT(*) > 2 ORDER BY a DESC LIMIT 10",
        "SELECT @@version, @x, @'q'",
        "SELECT a FROM t WHERE a IN (SELECT id FROM u)",
    ];
    for sql in cases {
        assert_roundtrip(sql);
    }
}

/// 原观察项：`` `my table`.* `` 曾不可解析（`parse_primary` 的 QuotedIdent 分支缺 `Mul` 特判）。
/// 现已与 Ident 分支对称补齐 —— 翻面为正向断言，并加往返校验守住回归：
/// 只断言 `is_ok()` 挡不住"能解析但格式化后语义变了"（如退化成普通标识符列项或丢失反引号）。
#[test]
fn quoted_identifier_wildcard_parses_and_roundtrips() {
    assert!(parse_sql("SELECT t.* FROM t").is_ok());

    let sql = "SELECT `my table`.* FROM `my table`";
    assert_eq!(roundtrip(sql).expect("应可解析且往返稳定"), sql);
    // 必须还原成带表限定的通配符，而不是 `my table`.`*` 这样的两段标识符
    let stmts = parse_sql(sql).unwrap();
    let SQLStatement::Select(s) = &stmts[0] else {
        panic!("not select")
    };
    assert_eq!(s.columns[0], SelectItem::Wildcard(Some("my table".into())));

    // 格式化器本身对被引号包裹的限定名处理正确（走 format_ident + "*" 直通）
    assert_eq!(
        format_expr(&SQLExpr::Identifier(vec!["my table".into(), "*".into()])),
        "`my table`.*"
    );
}

/// 观察项（parser 侧限制，非本次 format 修复引入）：表级约束（PRIMARY KEY (...)）不被支持，
/// 因此带表级主键的 DDL 无法进入格式化管道 —— 防火墙/审计看到的建表语句会缺主键信息。
#[test]
fn observe_table_level_constraints_are_unparseable() {
    assert!(
        parse_sql("CREATE TABLE t (id INT, PRIMARY KEY (id))").is_err(),
        "若此处成立，说明 parser 已支持表级约束，本观察项可删"
    );
    assert!(parse_sql("CREATE TABLE t (id INT, UNIQUE (id))").is_err());
}
