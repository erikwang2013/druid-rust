//! 对抗验证（druid-util / druid-core 侧）
//!
//! 攻击目标：
//!   1. `substitute_params_mysql` 的转义是否真能防注入（含跨参数逃逸）
//!   2. `DruidConfig` 的 Debug 脱敏是否可绕过
//!   3. `DruidConfig::validate()` 的边界与误拒
//!
//! 本文件只验证与取证，**不修改任何 src/**。
//!
//! 首轮发现的两个缺陷（Debug 脱敏 fail-open、字面量内的 `?` 被当占位符）已修复，
//! 原 `FAIL_*` 用例转为正断言留作回归；下面的清单记录“攻击未命中”。

use druid_core::{DruidConfig, DruidError};
use druid_util::string::substitute_params_mysql;

/// 哨兵口令：只要它出现在 Debug 输出里，就是明文泄漏
const SECRET: &str = "S3CRET-D0-NOT-LEAK-7f21";

// ---------------------------------------------------------------------------
// 独立判定工具（不接受被测代码的自证）
// ---------------------------------------------------------------------------

/// 按 MySQL 手册的默认字符串规则解析第一个字面量：
/// `\` 转义后随的任意字符（`\\`→`\`，`\'`→`'`，`\x`→`x`）；`''` 表示一个引号。
/// 返回 (字面量值, 闭合引号之后的剩余文本)。
///
/// 与 src 里的解析器独立实现：这里刻意按“反斜杠吃掉下一个字符”的通用规则写，
/// 而不是只认生成器会产出的两种序列，避免共享同一套错误假设导致往返假绿。
fn mysql_literal_value(s: &str) -> Option<(String, String)> {
    let start = s.find('\'')? + 1;
    let cs: Vec<char> = s[start..].chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' => {
                out.push(*cs.get(i + 1)?);
                i += 2;
            }
            '\'' if cs.get(i + 1) == Some(&'\'') => {
                out.push('\'');
                i += 2;
            }
            '\'' => return Some((out, cs[i + 1..].iter().collect())),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    None // 未闭合
}

/// 依次解码整条 SQL 中的全部字面量，返回 (各字面量值, 字面量之间的原文片段)
fn decode_literals(sql: &str) -> (Vec<String>, Vec<String>) {
    let mut values = Vec::new();
    let mut outside = Vec::new();
    let mut rest = sql.to_string();
    loop {
        match rest.find('\'') {
            Some(pos) => {
                let (val, tail) = mysql_literal_value(&rest[pos..]).expect("字面量未闭合");
                outside.push(rest[..pos].to_string());
                values.push(val);
                rest = tail;
            }
            None => {
                outside.push(rest.clone());
                break;
            }
        }
    }
    (values, outside)
}

// ---------------------------------------------------------------------------
// 回归：首轮脱敏器的漏网形态（现已 fail-closed）
// ---------------------------------------------------------------------------

/// 旧脱敏器只认「敏感键名 = 值」这一种形态（键名命中 pass/pwd/secret/token 才打码），
/// 任何口令出现在 **非敏感键的值** 里、或 userinfo 用的不是字面量 `:`，都会原样落进日志。
/// 现在：键名走白名单（未知键一律打码）、userinfo 在 `@` 前整体打码、URL 值递归脱敏。
#[test]
fn debug_masks_shapes_previous_sanitizer_missed() {
    let mut props_url = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    props_url.connection_properties = vec![format!("jdbcUrl=jdbc:mysql://u:{SECRET}@h2/db")];

    let mut props_uri = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    props_uri.connection_properties = vec![format!("uri=mongodb://u:{SECRET}@h2/db")];

    let cases: Vec<(&str, DruidConfig)> = vec![
        (
            "查询串里非敏感键(url=)的值是内嵌凭据的 URL",
            DruidConfig::new(
                &format!("jdbc:mysql://host/db?url=jdbc:mysql://u:{SECRET}@h2/db"),
                "root",
                SECRET,
            ),
        ),
        (
            "连接属性里非敏感键(jdbcUrl=)的值是内嵌凭据的 URL",
            props_url,
        ),
        ("连接属性里非敏感键(uri=)的值是内嵌凭据的 URL", props_uri),
        (
            "userinfo 分隔符被百分号编码（%3A），无字面量冒号",
            DruidConfig::new(
                &format!("jdbc:mysql://root%3A{SECRET}@host/db"),
                "root",
                SECRET,
            ),
        ),
        (
            "口令含 / —— authority 在第一个 / 处被截断，userinfo 里没有 @",
            DruidConfig::new(
                &format!("jdbc:mysql://root:Ab/{SECRET}@host/db"),
                "root",
                SECRET,
            ),
        ),
        (
            "Oracle thin 口令含 / —— rfind('/') 只切最后一段",
            DruidConfig::new(
                &format!("jdbc:oracle:thin:scott/{SECRET}/x@host:1521:ORCL"),
                "scott",
                SECRET,
            ),
        ),
        (
            "口令落在不在键名提示表里的驱动键上（如 credential）",
            DruidConfig::new(
                &format!("jdbc:mysql://host/db?credential={SECRET}"),
                "root",
                SECRET,
            ),
        ),
    ];

    let leaks: Vec<String> = cases
        .iter()
        .filter(|(_, cfg)| format!("{cfg:?}").contains(SECRET))
        .map(|(label, cfg)| format!("  - {label}\n      Debug = {cfg:?}"))
        .collect();

    assert!(
        leaks.is_empty(),
        "以下形态重新把明文口令写进了 Debug（共 {} 条）:\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 1. substitute_params_mysql：转义正确性
// ---------------------------------------------------------------------------

/// 手工推导的期望输出表 —— 不依赖任何解析器，杜绝“生成器与解析器共享错误假设”的假绿。
/// 转义规则：先把 `\` 翻倍，再把 `'` 翻倍，最后两侧加引号。
#[test]
fn mysql_escape_hardcoded_expectations() {
    let cases: &[(&str, &str)] = &[
        (r"", r"''"),
        (r"plain", r"'plain'"),
        (r"O'Reilly", r"'O''Reilly'"),
        (r"a\b", r"'a\\b'"),
        (r"\", r"'\\'"),
        (r"abc\", r"'abc\\'"), // 结尾反斜杠：必须翻倍，否则会吃掉闭合引号
        (r"\\", r"'\\\\'"),
        (r"'", r"''''"),
        (r"''", r"''''''"), // 两个单引号 → 4 个 + 两侧 = 6 个
        (r"\'", r"'\\'''"), // 反斜杠先翻倍，单引号再翻倍
        (r"\\'", r"'\\\\'''"),
        (r"\\\'", r"'\\\\\\'''"),
        (r"'\'", r"'''\\'''"), // 引号 反斜杠 引号
        (r"a'b\c'd", r"'a''b\\c''d'"),
        (r" -- comment", r"' -- comment'"),
    ];
    for (param, expected) in cases {
        assert_eq!(
            substitute_params_mysql("?", &[param]),
            *expected,
            "参数 {param:?} 的转义结果与手工推导不符"
        );
    }
}

/// 用独立解码器做结构校验：字面量必须逐个还原成原参数，且字面量之外没有任何残留 SQL。
#[test]
fn mysql_escape_survives_independent_decoder() {
    let params: &[&str] = &[r"\", r" OR 1=1 -- ", r"'", r"\\'", r"a'b\c", r"%", r"\n"];
    let template = "a = ? AND b = ? AND c = ? AND d = ? AND e = ? AND f = ? AND g = ?";
    let sql = substitute_params_mysql(template, params);

    let (values, outside) = decode_literals(&sql);
    assert_eq!(values, params, "字面量还原结果与传入参数不一致: {sql}");
    assert_eq!(
        outside,
        vec![
            "a = ",
            " AND b = ",
            " AND c = ",
            " AND d = ",
            " AND e = ",
            " AND f = ",
            " AND g = ",
            ""
        ],
        "字面量之外出现了残留 SQL: {sql}"
    );
}

/// 经典跨参数逃逸：参数 1 以 `\` 结尾、参数 2 以 `'` 开头，看能否拼出闭合引号。
/// 期望输出同样手工硬编码，不靠解码器。
#[test]
fn mysql_escape_blocks_cross_parameter_escape() {
    assert_eq!(
        substitute_params_mysql("a = ? AND b = ?", &[r"\", r" OR 1=1 -- "]),
        r"a = '\\' AND b = ' OR 1=1 -- '"
    );
    assert_eq!(
        substitute_params_mysql("a = ? AND b = ?", &[r"x\", r"' OR 1=1 -- "]),
        r"a = 'x\\' AND b = ''' OR 1=1 -- '"
    );
    assert_eq!(
        substitute_params_mysql("a = ? AND b = ?", &[r"\", r"'"]),
        r"a = '\\' AND b = ''''"
    );
    assert_eq!(
        substitute_params_mysql("a = ? AND b = ?", &[r"\\'", r"'"]),
        r"a = '\\\\''' AND b = ''''"
    );

    // 结构校验：任何组合都不能让两个字面量粘连
    for (p1, p2) in [
        (r"\", r"'"),
        (r"\\", r"\'"),
        (r"\\'", r"\\\'"),
        (r"'", r"\"),
        (r"''", r"\\"),
    ] {
        let sql = substitute_params_mysql("a = ? AND b = ?", &[p1, p2]);
        let (values, outside) = decode_literals(&sql);
        assert_eq!(
            values,
            vec![p1.to_string(), p2.to_string()],
            "跨参数逃逸: {sql}"
        );
        assert_eq!(outside, vec!["a = ", " AND b = ", ""], "残留 SQL: {sql}");
    }
}

/// 边界：空参数、纯反斜杠、超长参数、Unicode、参数内含 `?`
#[test]
fn mysql_escape_edge_inputs() {
    assert_eq!(substitute_params_mysql("?", &[""]), "''");
    assert_eq!(substitute_params_mysql("?", &[r"\\"]), r"'\\\\'");

    // 超长参数（1 万个字符，交替 \ 与 '）不得 panic，且必须能完整还原
    let long: String = std::iter::repeat_n([r"\", r"'", "a"], 3334)
        .flatten()
        .collect::<String>()
        + &"x".repeat(10_000);
    let sql = substitute_params_mysql("p = ?", &[&long]);
    let (values, _) = decode_literals(&sql);
    assert_eq!(values, vec![long.clone()]);

    // Unicode + 参数里带 ?（单次遍历必须不把参数内部的 ? 当占位符）
    assert_eq!(
        substitute_params_mysql("q = ?", &["?你好?a"]),
        "q = '?你好?a'"
    );
}

/// 废弃函数的注入前提必须成立 —— 否则“新增 mysql 版本”的必要性无从谈起。
#[test]
#[allow(deprecated)]
fn deprecated_substitute_params_is_injectable_as_documented() {
    let sql =
        druid_util::string::substitute_params("name = ? AND pass = ?", &[r"\", r" OR 1=1 -- "]);
    assert_eq!(sql, r"name = '\' AND pass = ' OR 1=1 -- '");
    // 按 MySQL 规则解析：`\'` 被吃成转义引号，字面量延续到 AND pass = 之后 → 参数值被改写
    let (v1, _) = mysql_literal_value(&sql).unwrap();
    assert_ne!(v1, r"\", "旧函数本应可注入，这里说明前提不成立");
    assert!(v1.contains("AND pass"), "逃逸后的字面量: {v1:?}");
}

/// `NO_BACKSLASH_ESCAPES` 语义（反斜杠是普通字符）下，`\` 翻倍会改变数据内容。
/// 这不是注入，但是数据损坏 —— 实测并留证。
#[test]
fn mysql_escape_over_escapes_under_no_backslash_escapes() {
    let param = r"C:\path\to";
    let sql = substitute_params_mysql("p = ?", &[param]);
    assert_eq!(sql, r"p = 'C:\\path\\to'");
    // 该模式下的字面量内容就是引号之间的原文（无转义）
    let inner = &sql[sql.find('\'').unwrap() + 1..sql.rfind('\'').unwrap()];
    assert_ne!(
        inner, param,
        "期望观察到数据内容被改写（记录为已知限制，而非注入）"
    );
    assert_eq!(inner, r"C:\\path\\to");

    // 对照：废弃版本在该模式下反而是正确的，但它在默认模式下可注入 —— 两者不可兼得
    #[allow(deprecated)]
    let old = druid_util::string::substitute_params("p = ?", &[param]);
    let old_inner = &old[old.find('\'').unwrap() + 1..old.rfind('\'').unwrap()];
    assert_eq!(old_inner, param);
}

/// 回归：SQL 文本里的 `?` 位于字符串字面量内部时，曾被当占位符替换，拼出语法错误的 SQL
/// （该缺陷为继承而来：废弃版本同样存在）。现在按词法状态机只在普通态识别占位符。
#[test]
fn placeholder_inside_literal_is_not_substituted() {
    // `?` 出现在字面量里是常见写法（JSON 片段、ILIKE 模式、文本常量）
    let sql = substitute_params_mysql("SELECT 'a?b', ? FROM t", &["x"]);
    assert_eq!(
        sql, "SELECT 'a?b', 'x' FROM t",
        "字面量内部的 ? 被当作占位符替换，生成的 SQL 无法解析"
    );
    // 注释里的 ? 同理；字面量外的 ? 仍按顺序填充
    assert_eq!(
        substitute_params_mysql("SELECT ? -- ?\nFROM t WHERE c = ?", &["a", "b"]),
        "SELECT 'a' -- ?\nFROM t WHERE c = 'b'"
    );
    assert_eq!(
        substitute_params_mysql("SELECT /* ? */ ? FROM t", &["a"]),
        "SELECT /* ? */ 'a' FROM t"
    );
    // MySQL 的 `#` 行注释同样到行尾为止（含其中的 ?）
    assert_eq!(
        substitute_params_mysql("SELECT ? # ?\nFROM t WHERE c = ?", &["a", "b"]),
        "SELECT 'a' # ?\nFROM t WHERE c = 'b'"
    );
}

// ---------------------------------------------------------------------------
// 2. Debug 脱敏：应当被挡住的形态（攻击未命中清单）
// ---------------------------------------------------------------------------

fn assert_no_secret(cfg: &DruidConfig, label: &str) {
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains(SECRET), "{label} 泄漏明文口令: {dbg}");
}

#[test]
fn debug_masks_userinfo_and_query_params() {
    let urls = [
        format!("jdbc:mysql://root:{SECRET}@host/db"),
        format!("jdbc:mysql://root:{SECRET}@host:3306/db?useSSL=false"),
        format!("jdbc:mysql://host/db?password={SECRET}"),
        format!("jdbc:mysql://host/db?pwd={SECRET}"),
        format!("jdbc:mysql://host/db?PASSWORD={SECRET}"),
        format!("jdbc:mysql://host/db?PaSsWoRd={SECRET}"),
        format!("jdbc:mysql://host/db?secret={SECRET}"),
        format!("jdbc:mysql://host/db?token={SECRET}"),
        format!("jdbc:mysql://host/db?accessToken={SECRET}"),
        format!("jdbc:mysql://host/db?user=root&password={SECRET}&ssl=true"),
        format!("jdbc:mysql://host/db? password = {SECRET}"),
        format!("jdbc:mysql://host/db?password ={SECRET}"),
        format!("jdbc:mysql://host/db?password={SECRET}%2F"),
        format!("jdbc:mysql://host/db?password={SECRET}&x=1"),
        format!("jdbc:mysql://root:{SECRET}@host/db?password={SECRET}"),
        format!("jdbc:sqlserver://host:1433;user=sa;password={SECRET}"),
        format!("jdbc:sqlserver://host:1433;databaseName=db;password={SECRET}"),
        format!("jdbc:mysql://host/db;password={SECRET}"),
        format!("jdbc:oracle:thin:scott/{SECRET}@host:1521:ORCL"),
        format!("jdbc:mysql://host/db#password={SECRET}"),
        format!("jdbc:mysql:db?password={SECRET}"),
    ];
    for url in urls {
        assert_no_secret(&DruidConfig::new(&url, "root", SECRET), &url);
    }
}

#[test]
fn debug_masks_connection_properties() {
    let mut cfg = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    cfg.connection_properties = vec![
        format!("password={SECRET}"),
        format!("pwd={SECRET}"),
        format!("secret={SECRET}"),
        format!("accessToken={SECRET}"),
        format!("PASSWORD={SECRET}"),
        format!("pass word={SECRET}"),
        "user=keepme".into(),
        "autoReconnect=true".into(),
    ];
    assert_no_secret(&cfg, "connection_properties");
    // 非敏感属性必须保留，避免“脱敏=全打码”
    let dbg = format!("{cfg:?}");
    assert!(dbg.contains("user=keepme"), "{dbg}");
    assert!(dbg.contains("autoReconnect=true"), "{dbg}");
}

#[test]
fn debug_masks_secret_that_looks_like_a_key_name() {
    // 口令本身含被脱敏的键名 / 含 = & ; 等分隔符
    for url in [
        "jdbc:mysql://host/db?password=secret".to_string(),
        "jdbc:mysql://host/db?password=token=secret".to_string(),
        "jdbc:mysql://host/db?password=pwd&x=1".to_string(),
        format!("jdbc:mysql://host/db?password={SECRET}"),
    ] {
        assert_no_secret(&DruidConfig::new(&url, "root", SECRET), &url);
    }
}

// ---------------------------------------------------------------------------
// 3. validate() 边界
// ---------------------------------------------------------------------------

#[test]
fn validate_rejects_inconsistent_bounds() {
    let base = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    assert!(base.validate().is_ok(), "默认配置不应被拒");

    let mut c = base.clone();
    c.max_active = 0;
    assert!(
        matches!(c.validate(), Err(DruidError::Config(_))),
        "max_active=0"
    );

    let mut c = base.clone();
    c.min_idle = c.max_active + 1;
    assert!(
        matches!(c.validate(), Err(DruidError::Config(_))),
        "min_idle>max_active"
    );

    let mut c = base.clone();
    c.initial_size = c.max_active + 1;
    assert!(
        matches!(c.validate(), Err(DruidError::Config(_))),
        "initial_size>max_active"
    );

    let mut c = base.clone();
    c.connect_timeout_secs = 0;
    assert!(
        matches!(c.validate(), Err(DruidError::Config(_))),
        "connect_timeout_secs=0"
    );
}

#[test]
fn validate_accepts_legal_boundaries() {
    let mut cfg = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    cfg.max_active = 4;
    cfg.min_idle = 4;
    cfg.initial_size = 4;
    assert!(
        cfg.validate().is_ok(),
        "min_idle==max_active==initial_size 必须通过"
    );

    cfg.min_idle = 0;
    assert!(cfg.validate().is_ok(), "min_idle=0 必须通过");

    let mut big = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    big.max_active = usize::MAX;
    big.initial_size = usize::MAX;
    big.min_idle = usize::MAX;
    assert!(big.validate().is_ok(), "极大值不应因溢出/比较被误拒");
}

/// 间隔参数取 0 的两种语义，validate() 现在必须区分开：
///   - 驱逐间隔 0 = 禁用循环（`datasource.rs` 里有 `> 0` 守卫），合法，不该误拒；
///   - KeepAlive 打开且间隔 0：`background.rs:81` 是 `loop { sleep(interval) }` 且每轮
///     都要打一遍数据库 —— sleep(0) 立即返回，等于空转热循环轰库，必须拒绝。
///
/// 对照 Java 版：`period <= 0 → 1000ms`，非正值不会变成 0 间隔。
#[test]
fn validate_rejects_zero_keepalive_interval_but_allows_disabled_eviction() {
    let mut cfg = DruidConfig::new("jdbc:mysql://host/db", "root", SECRET);
    cfg.time_between_eviction_runs_ms = 0;
    assert!(cfg.validate().is_ok(), "驱逐间隔 0 表示禁用循环，合法");
    assert_eq!(cfg.eviction_interval(), std::time::Duration::ZERO);

    cfg.keep_alive_between_time_ms = 0;
    cfg.keep_alive = true;
    assert!(
        matches!(cfg.validate(), Err(DruidError::Config(_))),
        "keep_alive=true 且间隔为 0 会让后台循环空转并每轮打库，必须被拒"
    );
    assert_eq!(cfg.keep_alive_interval(), std::time::Duration::ZERO);

    // 给出正间隔即合法
    cfg.keep_alive_between_time_ms = 120_000;
    assert!(cfg.validate().is_ok());
}
