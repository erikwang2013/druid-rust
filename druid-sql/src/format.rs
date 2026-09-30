use crate::ast::*;
use std::fmt::Write;

/// 将 SQL AST 格式化输出为 SQL 字符串
pub fn format_statement(stmt: &SQLStatement) -> String {
    match stmt {
        SQLStatement::Select(s) => format_select(s),
        SQLStatement::Insert(s) => format_insert(s),
        SQLStatement::Update(s) => format_update(s),
        SQLStatement::Delete(s) => format_delete(s),
        SQLStatement::CreateTable(s) => format_create_table(s),
        SQLStatement::DropObject(s) => format_drop(s),
    }
}

fn format_select(stmt: &SelectStatement) -> String {
    let mut s = String::new();
    if !stmt.with_cte.is_empty() {
        s.push_str("WITH ");
        let ctes: Vec<String> = stmt
            .with_cte
            .iter()
            .map(|cte| {
                let mut def = format_ident(&cte.name);
                if !cte.columns.is_empty() {
                    let cols: Vec<String> = cte.columns.iter().map(|c| format_ident(c)).collect();
                    def.push_str(&format!(" ({})", cols.join(", ")));
                }
                def.push_str(&format!(" AS ({})", format_select(&cte.query)));
                def
            })
            .collect();
        s.push_str(&ctes.join(", "));
        s.push(' ');
    }
    s.push_str("SELECT ");
    if stmt.distinct {
        s.push_str("DISTINCT ");
    }

    let cols: Vec<String> = stmt
        .columns
        .iter()
        .map(|item| match item {
            SelectItem::Expr(e, alias) => {
                let mut es = format_expr(e);
                if let Some(a) = alias {
                    es.push_str(" AS ");
                    es.push_str(&format_ident(a));
                }
                es
            }
            SelectItem::Wildcard(Some(t)) => format!("{}.*", format_ident(t)),
            SelectItem::Wildcard(None) => "*".to_string(),
        })
        .collect();
    s.push_str(&cols.join(", "));

    if let Some(ref from) = stmt.from {
        s.push_str(" FROM ");
        s.push_str(&format_table_ref(from));
    }

    for join in &stmt.joins {
        write!(s, " {}", format_join_clause(join)).unwrap();
    }

    if let Some(ref where_clause) = stmt.where_clause {
        write!(s, " WHERE {}", format_expr(where_clause)).unwrap();
    }

    if !stmt.group_by.is_empty() {
        let gb: Vec<String> = stmt.group_by.iter().map(format_expr).collect();
        write!(s, " GROUP BY {}", gb.join(", ")).unwrap();
    }

    if let Some(ref having) = stmt.having {
        write!(s, " HAVING {}", format_expr(having)).unwrap();
    }

    if !stmt.order_by.is_empty() {
        let ob: Vec<String> = stmt
            .order_by
            .iter()
            .map(|o| {
                let mut es = format_expr(&o.expr);
                if !o.asc {
                    es.push_str(" DESC");
                }
                es
            })
            .collect();
        write!(s, " ORDER BY {}", ob.join(", ")).unwrap();
    }

    if let Some(ref limit) = stmt.limit {
        write!(s, " LIMIT {}", format_expr(limit)).unwrap();
    }

    if let Some(ref offset) = stmt.offset {
        write!(s, " OFFSET {}", format_expr(offset)).unwrap();
    }

    s
}

fn format_insert(stmt: &InsertStatement) -> String {
    let mut s = String::new();
    let kw = if stmt.is_replace { "REPLACE" } else { "INSERT" };
    s.push_str(kw);
    write!(s, " INTO {}", format_ident(&stmt.table)).unwrap();

    if !stmt.columns.is_empty() {
        let cols: Vec<String> = stmt.columns.iter().map(|c| format_ident(c)).collect();
        write!(s, " ({})", cols.join(", ")).unwrap();
    }

    s.push_str(" VALUES ");
    let rows: Vec<String> = stmt
        .values
        .iter()
        .map(|row| {
            let vals: Vec<String> = row.iter().map(format_expr).collect();
            format!("({})", vals.join(", "))
        })
        .collect();
    s.push_str(&rows.join(", "));
    s
}

fn format_update(stmt: &UpdateStatement) -> String {
    let mut s = format!("UPDATE {}", format_ident(&stmt.table));
    let sets: Vec<String> = stmt
        .sets
        .iter()
        .map(|(col, val)| format!("{} = {}", format_ident(col), format_expr(val)))
        .collect();
    write!(s, " SET {}", sets.join(", ")).unwrap();
    if let Some(ref w) = stmt.where_clause {
        write!(s, " WHERE {}", format_expr(w)).unwrap();
    }
    s
}

fn format_delete(stmt: &DeleteStatement) -> String {
    let mut s = format!("DELETE FROM {}", format_ident(&stmt.table));
    if let Some(ref w) = stmt.where_clause {
        write!(s, " WHERE {}", format_expr(w)).unwrap();
    }
    s
}

fn format_create_table(stmt: &CreateTableStatement) -> String {
    let mut s = "CREATE TABLE ".to_string();
    if stmt.if_not_exists {
        s.push_str("IF NOT EXISTS ");
    }
    write!(s, "{} (", format_ident(&stmt.table)).unwrap();
    let cols: Vec<String> = stmt
        .columns
        .iter()
        .map(|c| {
            // 列约束必须还原，否则拿去执行会把 NOT NULL / PRIMARY KEY / DEFAULT 全丢掉
            let mut col = format!("{} {}", format_ident(&c.name), c.data_type);
            if c.is_primary_key {
                col.push_str(" PRIMARY KEY");
            }
            if !c.nullable {
                col.push_str(" NOT NULL");
            }
            if let Some(d) = &c.default_value {
                write!(col, " DEFAULT {}", format_expr(d)).unwrap();
            }
            col
        })
        .collect();
    s.push_str(&cols.join(", "));
    s.push(')');
    s
}

fn format_drop(stmt: &DropStatement) -> String {
    let obj = match stmt.object_type {
        DropObjectType::Table => "TABLE",
        DropObjectType::View => "VIEW",
        DropObjectType::Index => "INDEX",
        DropObjectType::Database => "DATABASE",
    };
    let mut s = format!("DROP {} ", obj);
    if stmt.if_exists {
        s.push_str("IF EXISTS ");
    }
    s.push_str(&format_ident(&stmt.name));
    s
}

/// 标识符按需加引号：不匹配 [A-Za-z_][A-Za-z0-9_]* 或是 SQL 关键字时用反引号包裹。
/// 裸输出会让 `SELECT "total amount"` 变成列 + 别名、`` `select` `` 重新解析失败。
fn format_ident(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && crate::token::lookup_keyword(name).is_none();
    if plain {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

/// 变量按原样输出，但名字不是普通标识符形态时必须用 @'…' 引号回写：
/// `@'a b'` 裸写成 `@a b` 会被重解析成「变量 @a + 别名 b」，凭空多出别名且不报错。
fn format_variable(v: &str) -> String {
    let prefix_len = if v.starts_with("@@") {
        2
    } else if v.starts_with('@') {
        1
    } else {
        0
    };
    let (prefix, name) = v.split_at(prefix_len);
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$');
    if plain || name.is_empty() {
        v.to_string()
    } else {
        // 引号内只做单引号加倍（read_quoted_ident 不处理反斜杠）
        format!("{}'{}'", prefix, name.replace('\'', "''"))
    }
}

/// 二元运算符优先级，数值越大结合越紧（与 parse_expr 的层次对应）
fn op_precedence(op: BinaryOpType) -> u8 {
    match op {
        BinaryOpType::Or => 1,
        BinaryOpType::And => 2,
        BinaryOpType::Eq
        | BinaryOpType::Neq
        | BinaryOpType::Lt
        | BinaryOpType::Gt
        | BinaryOpType::Leq
        | BinaryOpType::Geq
        | BinaryOpType::Like
        | BinaryOpType::Regex => 3,
        BinaryOpType::Concat => 4,
        BinaryOpType::Plus | BinaryOpType::Minus => 5,
        BinaryOpType::Mul | BinaryOpType::Div | BinaryOpType::Mod => 6,
    }
}

/// 表达式自身优先级；非二元表达式视为原子（不会为优先级补括号）
fn expr_precedence(expr: &SQLExpr) -> u8 {
    match expr {
        SQLExpr::BinaryOp { op, .. } => op_precedence(*op),
        // 这几个节点与比较运算符同级（见 COMPARISON_PREC）。
        // 当原子看待会让 a LIKE (b LIKE 'c') 这类嵌套的括号丢失，重解析成分组不同的左结合
        SQLExpr::Between { .. }
        | SQLExpr::InList { .. }
        | SQLExpr::InSubQuery { .. }
        | SQLExpr::Like { .. }
        | SQLExpr::IsNull { .. } => op_precedence(COMPARISON_PREC),
        SQLExpr::UnaryOp { .. } => 7,
        _ => 8,
    }
}

/// BETWEEN / IN / LIKE / IS NULL 的操作数所在层级（parse_comparison 一层）：
/// 这些节点只有操作数与比较运算符同级，借用 Like 代表该优先级
const COMPARISON_PREC: BinaryOpType = BinaryOpType::Like;

/// 格式化二元运算的操作数：子表达式结合更松、或右侧同级且父运算符不满足结合律时补括号，
/// 否则外部构造的 AST 经格式化后语义会改变（(a+b)*c 变成 a+b*c）。
fn format_operand(expr: &SQLExpr, parent: BinaryOpType, is_right: bool) -> String {
    let parent_prec = op_precedence(parent);
    let child_prec = expr_precedence(expr);
    let need_parens = child_prec < parent_prec
        || (child_prec == parent_prec
            && is_right
            && !matches!(parent, BinaryOpType::And | BinaryOpType::Or));
    let s = format_expr(expr);
    if need_parens {
        format!("({})", s)
    } else {
        s
    }
}

/// 格式化 JOIN 子句（TableReference::Join 变体没有左侧表，只能渲染自身）
fn format_join_clause(join: &JoinClause) -> String {
    format!(
        "{} {} ON {}",
        join.join_type,
        format_table_ref(&join.table),
        format_expr(&join.on)
    )
}

fn format_table_ref(tr: &TableReference) -> String {
    match tr {
        TableReference::Table {
            name,
            alias,
            schema,
        } => {
            let mut s = if let Some(sch) = schema {
                format!("{}.{}", format_ident(sch), format_ident(name))
            } else {
                format_ident(name)
            };
            if let Some(a) = alias {
                write!(s, " {}", format_ident(a)).unwrap();
            }
            s
        }
        TableReference::SubQuery(stmt, alias) => {
            format!("({}) {}", format_statement(stmt), format_ident(alias))
        }
        TableReference::Join(join) => format_join_clause(join),
    }
}

pub fn format_expr(expr: &SQLExpr) -> String {
    match expr {
        // "*" 是通配符而非标识符，不能加引号
        SQLExpr::Identifier(parts) => parts
            .iter()
            .map(|p| if p == "*" { p.clone() } else { format_ident(p) })
            .collect::<Vec<_>>()
            .join("."),
        // 单引号加倍转义，否则 ' 会提前结束字面量（注入 / 语法错误）；
        // 本 crate 词法把反斜杠当转义符（MySQL 语义），同样需要加倍
        SQLExpr::StringLiteral(s) => {
            format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
        }
        SQLExpr::NumberLiteral(n) => n.clone(),
        // 变量不能整体加引号（会从 @x 变成同名列），只在名字不是普通标识符形态时用 @'…'
        SQLExpr::Variable(v) => format_variable(v),
        SQLExpr::Null => "NULL".to_string(),
        SQLExpr::Placeholder => "?".to_string(),
        SQLExpr::Wildcard => "*".to_string(),
        SQLExpr::BinaryOp { left, op, right } => format!(
            "{} {} {}",
            format_operand(left, *op, false),
            op,
            format_operand(right, *op, true)
        ),
        SQLExpr::UnaryOp { op, expr } => {
            let op_str = match op {
                UnaryOpType::Not => "NOT ",
                UnaryOpType::Neg => "-",
                UnaryOpType::Plus => "+",
            };
            let inner = format_expr(expr);
            // 一元运算作用在二元表达式上必须补括号：-(a + b)
            let inner = if matches!(expr.as_ref(), SQLExpr::BinaryOp { .. }) {
                format!("({})", inner)
            } else {
                inner
            };
            format!("{}{}", op_str, inner)
        }
        SQLExpr::Function {
            name,
            args,
            distinct,
        } => {
            let d = if *distinct { "DISTINCT " } else { "" };
            let a: Vec<String> = args.iter().map(format_expr).collect();
            format!("{}({}{})", name, d, a.join(", "))
        }
        SQLExpr::Nested(e) => format!("({})", format_expr(e)),
        SQLExpr::InList { expr, list, not } => {
            let items: Vec<String> = list.iter().map(format_expr).collect();
            let n = if *not { "NOT " } else { "" };
            format!(
                "{} {}IN ({})",
                format_operand(expr, COMPARISON_PREC, false),
                n,
                items.join(", ")
            )
        }
        SQLExpr::Between {
            expr,
            low,
            high,
            not,
        } => {
            let n = if *not { "NOT " } else { "" };
            // low/high 被 AND 包夹，按右结合侧处理（宁多一对括号）
            format!(
                "{} {}BETWEEN {} AND {}",
                format_operand(expr, COMPARISON_PREC, false),
                n,
                format_operand(low, COMPARISON_PREC, true),
                format_operand(high, COMPARISON_PREC, true)
            )
        }
        SQLExpr::IsNull { expr, not } => {
            let n = if *not { "NOT " } else { "" };
            format!(
                "{} IS {}NULL",
                format_operand(expr, COMPARISON_PREC, false),
                n
            )
        }
        SQLExpr::Like { expr, pattern, not } => {
            let n = if *not { "NOT " } else { "" };
            format!(
                "{} {}LIKE {}",
                format_operand(expr, COMPARISON_PREC, false),
                n,
                format_operand(pattern, COMPARISON_PREC, true)
            )
        }
        SQLExpr::Exists(stmt, not) => {
            let n = if *not { "NOT " } else { "" };
            format!("{}EXISTS ({})", n, format_statement(stmt))
        }
        SQLExpr::SubQuery(stmt) => {
            format!("({})", format_statement(stmt))
        }
        SQLExpr::Case {
            expr,
            whens,
            else_expr,
        } => {
            let mut s = "CASE".to_string();
            if let Some(e) = expr {
                write!(s, " {}", format_expr(e)).unwrap();
            }
            for (cond, result) in whens {
                write!(
                    s,
                    " WHEN {} THEN {}",
                    format_expr(cond),
                    format_expr(result)
                )
                .unwrap();
            }
            if let Some(e) = else_expr {
                write!(s, " ELSE {}", format_expr(e)).unwrap();
            }
            s.push_str(" END");
            s
        }
        SQLExpr::Aggregate { name, expr } => {
            format!("{}({})", name, format_expr(expr))
        }
        SQLExpr::InSubQuery { expr, query, not } => {
            let n = if *not { "NOT " } else { "" };
            format!(
                "{} {}IN ({})",
                format_operand(expr, COMPARISON_PREC, false),
                n,
                format_statement(query)
            )
        }
        SQLExpr::Cast { expr, data_type } => {
            format!("CAST({} AS {})", format_expr(expr), data_type)
        }
        SQLExpr::WindowFunction {
            function,
            partition_by,
            order_by,
        } => {
            let mut s = format_expr(function);
            let mut over_parts = Vec::new();
            if !partition_by.is_empty() {
                let parts: Vec<String> = partition_by.iter().map(format_expr).collect();
                over_parts.push(format!("PARTITION BY {}", parts.join(", ")));
            }
            if !order_by.is_empty() {
                let ob: Vec<String> = order_by
                    .iter()
                    .map(|o| {
                        let mut es = format_expr(&o.expr);
                        if !o.asc {
                            es.push_str(" DESC");
                        }
                        es
                    })
                    .collect();
                over_parts.push(format!("ORDER BY {}", ob.join(", ")));
            }
            if !over_parts.is_empty() {
                write!(s, " OVER ({})", over_parts.join(" ")).unwrap();
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_sql;

    #[test]
    fn test_format_select() {
        let sql = "SELECT id, name FROM users WHERE age > 18";
        let stmts = parse_sql(sql).unwrap();
        let formatted = format_statement(&stmts[0]);
        assert!(formatted.contains("SELECT"));
        assert!(formatted.contains("FROM users"));
        assert!(formatted.contains("WHERE"));
    }

    #[test]
    fn test_format_roundtrip_simple() {
        let sql = "SELECT id, name FROM users";
        let stmts = parse_sql(sql).unwrap();
        let formatted = format_statement(&stmts[0]);
        // 重新解析格式化后的 SQL
        let reparsed = parse_sql(&formatted);
        assert!(reparsed.is_ok());
    }

    fn fmt(sql: &str) -> String {
        let stmts = parse_sql(sql).unwrap();
        assert_eq!(stmts.len(), 1, "expected 1 statement: {sql}");
        format_statement(&stmts[0])
    }

    #[test]
    fn test_format_select_full() {
        let out = fmt("SELECT DISTINCT t.a, COUNT(*) AS cnt, b + 1 FROM db.t x \
             LEFT JOIN u ON x.id = u.id WHERE x.a > 1 AND x.b IS NOT NULL \
             GROUP BY t.a HAVING COUNT(*) > 2 ORDER BY t.a DESC LIMIT 10");
        assert_eq!(
            out,
            "SELECT DISTINCT t.a, COUNT(*) AS cnt, b + 1 FROM db.t x \
             LEFT JOIN u ON x.id = u.id WHERE x.a > 1 AND x.b IS NOT NULL \
             GROUP BY t.a HAVING COUNT(*) > 2 ORDER BY t.a DESC LIMIT 10"
        );
    }

    #[test]
    fn test_format_wildcard_and_with() {
        assert_eq!(fmt("SELECT * FROM t"), "SELECT * FROM t");
        assert_eq!(fmt("SELECT t.* FROM t"), "SELECT t.* FROM t");
        let out = fmt("WITH x AS (SELECT 1) SELECT * FROM x");
        assert_eq!(out, "WITH x AS (SELECT 1) SELECT * FROM x");
    }

    #[test]
    fn test_format_dml() {
        assert_eq!(
            fmt("INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)"),
            "INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)"
        );
        assert_eq!(
            fmt("REPLACE INTO t VALUES (1)"),
            "REPLACE INTO t VALUES (1)"
        );
        assert_eq!(
            fmt("UPDATE t SET a = 1, b = a + 1 WHERE id = 3"),
            "UPDATE t SET a = 1, b = a + 1 WHERE id = 3"
        );
        assert_eq!(
            fmt("DELETE FROM t WHERE x = 1"),
            "DELETE FROM t WHERE x = 1"
        );
        assert_eq!(fmt("DELETE FROM t"), "DELETE FROM t");
    }

    #[test]
    fn test_format_ddl() {
        // 列约束必须保留：丢 NOT NULL / PRIMARY KEY / DEFAULT 等于悄悄改表结构
        assert_eq!(
            fmt("CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, name VARCHAR(20) NOT NULL)"),
            "CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, name VARCHAR(20) NOT NULL)"
        );
        assert_eq!(
            fmt("CREATE TABLE t (id INT PRIMARY KEY, name VARCHAR(10) NOT NULL DEFAULT 'x')"),
            "CREATE TABLE t (id INT PRIMARY KEY, name VARCHAR(10) NOT NULL DEFAULT 'x')"
        );
        assert!(parse_sql(&fmt("CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY)")).is_ok());
        assert_eq!(fmt("DROP TABLE IF EXISTS t"), "DROP TABLE IF EXISTS t");
        assert_eq!(fmt("DROP VIEW v"), "DROP VIEW v");
        assert_eq!(fmt("DROP INDEX i"), "DROP INDEX i");
    }

    #[test]
    fn test_format_data_type_reparses() {
        // 类型位拿到非类型 token 时，解析器写的是 token 的 SQL 文本（曾是 Debug 文本，
        // 格式化后无法重解析）；引号包裹的类型名要带引号回写
        for sql in [
            "CREATE TABLE t (id 'x')",
            "CREATE TABLE t (id 5)",
            "CREATE TABLE t (id \"a b\")",
            "CREATE TABLE t (id 'a''b')",
        ] {
            let first = parse_sql(sql).unwrap_or_else(|e| panic!("{sql:?} 首次解析失败: {e}"));
            let out = format_statement(&first[0]);
            let second = parse_sql(&out).unwrap_or_else(|e| panic!("{out:?} 重解析失败: {e}"));
            assert_eq!(first, second, "{sql:?} 格式化 {out:?} 后 AST 改变");
        }
    }

    #[test]
    fn test_format_expr_constructs() {
        use crate::ast::*;
        // 解析器无法产出的节点直接构造 AST 测试格式化
        let window = SQLExpr::WindowFunction {
            function: Box::new(SQLExpr::Function {
                name: "ROW_NUMBER".into(),
                args: vec![],
                distinct: false,
            }),
            partition_by: vec![SQLExpr::Identifier(vec!["a".into()])],
            order_by: vec![OrderByExpr {
                expr: SQLExpr::Identifier(vec!["b".into()]),
                asc: false,
            }],
        };
        assert_eq!(
            format_expr(&window),
            "ROW_NUMBER() OVER (PARTITION BY a ORDER BY b DESC)"
        );

        assert_eq!(
            format_expr(&SQLExpr::Cast {
                expr: Box::new(SQLExpr::Identifier(vec!["x".into()])),
                data_type: "INT".into(),
            }),
            "CAST(x AS INT)"
        );
        assert_eq!(
            format_expr(&SQLExpr::Case {
                expr: Some(Box::new(SQLExpr::Identifier(vec!["a".into()]))),
                whens: vec![(
                    SQLExpr::NumberLiteral("1".into()),
                    SQLExpr::StringLiteral("one".into())
                )],
                else_expr: Some(Box::new(SQLExpr::Null)),
            }),
            "CASE a WHEN 1 THEN 'one' ELSE NULL END"
        );
        assert_eq!(
            format_expr(&SQLExpr::UnaryOp {
                op: UnaryOpType::Not,
                expr: Box::new(SQLExpr::Identifier(vec!["x".into()])),
            }),
            "NOT x"
        );
        assert_eq!(format_expr(&SQLExpr::Null), "NULL");
        assert_eq!(format_expr(&SQLExpr::Placeholder), "?");
        assert_eq!(format_expr(&SQLExpr::Wildcard), "*");
        assert_eq!(format_expr(&SQLExpr::NumberLiteral("1.5".into())), "1.5");
        // 单引号必须加倍转义，否则注入 / 语法错误
        assert_eq!(
            format_expr(&SQLExpr::StringLiteral("it's".into())),
            "'it''s'"
        );
        assert_eq!(
            format_expr(&SQLExpr::StringLiteral(r"x' OR 1=1 -- ".into())),
            "'x'' OR 1=1 -- '"
        );
        // 反斜杠是本 crate 词法的转义符，同样加倍
        assert_eq!(
            format_expr(&SQLExpr::StringLiteral(r"a\b".into())),
            r"'a\\b'"
        );
        assert_eq!(
            format_expr(&SQLExpr::Identifier(vec!["a".into(), "b".into()])),
            "a.b"
        );
    }

    #[test]
    fn test_format_string_literal_reparses_identically() {
        for value in ["it's", r"a\b", "x' OR 1=1 -- ", "plain", r"tail\\"] {
            let formatted = format_expr(&SQLExpr::StringLiteral(value.into()));
            let stmts = parse_sql(&format!("SELECT {formatted}")).unwrap();
            let SQLStatement::Select(s) = &stmts[0] else {
                panic!()
            };
            assert_eq!(
                s.columns[0],
                SelectItem::Expr(SQLExpr::StringLiteral(value.into()), None),
                "roundtrip failed for {value:?}"
            );
        }
    }

    #[test]
    fn test_format_identifier_quoting() {
        // 非普通标识符需要加引号，否则重解析后语义改变
        assert_eq!(
            format_expr(&SQLExpr::Identifier(vec!["total amount".into()])),
            "`total amount`"
        );
        assert_eq!(
            format_expr(&SQLExpr::Identifier(vec!["select".into()])),
            "`select`"
        );
        assert_eq!(
            format_expr(&SQLExpr::Identifier(vec!["t".into(), "*".into()])),
            "t.*"
        );
        // 别名同理
        assert_eq!(
            fmt("SELECT 1 AS \"total amount\" FROM t"),
            "SELECT 1 AS `total amount` FROM t"
        );
        // 命名对象同样处理
        assert_eq!(
            fmt("SELECT \"total amount\" FROM t"),
            "SELECT `total amount` FROM t"
        );
        // 重新解析后仍是同一个标识符
        let out = fmt("SELECT \"total amount\" AS \"the sum\" FROM t");
        let SQLStatement::Select(s) = &parse_sql(&out).unwrap()[0] else {
            panic!()
        };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(
                SQLExpr::Identifier(vec!["total amount".into()]),
                Some("the sum".into())
            )
        );
    }

    #[test]
    fn test_format_binary_precedence_parens() {
        use crate::ast::*;
        // 外部构造的 AST：(a + b) * c 不能输出成 a + b * c
        let mul = SQLExpr::BinaryOp {
            left: Box::new(SQLExpr::BinaryOp {
                left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
                op: BinaryOpType::Plus,
                right: Box::new(SQLExpr::Identifier(vec!["b".into()])),
            }),
            op: BinaryOpType::Mul,
            right: Box::new(SQLExpr::Identifier(vec!["c".into()])),
        };
        assert_eq!(format_expr(&mul), "(a + b) * c");
        // 右侧同级且非结合运算：a - (b - c)
        let sub = SQLExpr::BinaryOp {
            left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
            op: BinaryOpType::Minus,
            right: Box::new(SQLExpr::BinaryOp {
                left: Box::new(SQLExpr::Identifier(vec!["b".into()])),
                op: BinaryOpType::Minus,
                right: Box::new(SQLExpr::Identifier(vec!["c".into()])),
            }),
        };
        assert_eq!(format_expr(&sub), "a - (b - c)");
        // 结合律成立的 AND/OR 不必加括号
        let and = SQLExpr::BinaryOp {
            left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
            op: BinaryOpType::And,
            right: Box::new(SQLExpr::BinaryOp {
                left: Box::new(SQLExpr::Identifier(vec!["b".into()])),
                op: BinaryOpType::And,
                right: Box::new(SQLExpr::Identifier(vec!["c".into()])),
            }),
        };
        assert_eq!(format_expr(&and), "a AND b AND c");
        // 一元负号作用在二元表达式上
        assert_eq!(
            format_expr(&SQLExpr::UnaryOp {
                op: UnaryOpType::Neg,
                expr: Box::new(SQLExpr::BinaryOp {
                    left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
                    op: BinaryOpType::Plus,
                    right: Box::new(SQLExpr::Identifier(vec!["b".into()])),
                }),
            }),
            "-(a + b)"
        );
    }

    #[test]
    fn test_format_operand_parens_around_parent_ops() {
        use crate::ast::*;
        let ident = |n: &str| SQLExpr::Identifier(vec![n.into()]);
        let or_ab = || SQLExpr::BinaryOp {
            left: Box::new(ident("a")),
            op: BinaryOpType::Or,
            right: Box::new(ident("b")),
        };
        let num = |n: &str| SQLExpr::NumberLiteral(n.into());

        // BETWEEN/IN/LIKE/IS NULL 的操作数与比较运算符同级，更松的子表达式必须补括号，
        // 否则 a OR b BETWEEN 1 AND 2 会重解析成 a OR (b BETWEEN 1 AND 2)
        assert_eq!(
            format_expr(&SQLExpr::Between {
                expr: Box::new(or_ab()),
                low: Box::new(num("1")),
                high: Box::new(num("2")),
                not: false,
            }),
            "(a OR b) BETWEEN 1 AND 2"
        );
        assert_eq!(
            format_expr(&SQLExpr::InList {
                expr: Box::new(or_ab()),
                list: vec![num("1"), num("2")],
                not: false,
            }),
            "(a OR b) IN (1, 2)"
        );
        assert_eq!(
            format_expr(&SQLExpr::Like {
                expr: Box::new(or_ab()),
                pattern: Box::new(SQLExpr::StringLiteral("x".into())),
                not: false,
            }),
            "(a OR b) LIKE 'x'"
        );
        assert_eq!(
            format_expr(&SQLExpr::IsNull {
                expr: Box::new(or_ab()),
                not: false,
            }),
            "(a OR b) IS NULL"
        );
        // 右侧同级：a LIKE b LIKE c 是左结合，模式侧必须补括号
        assert_eq!(
            format_expr(&SQLExpr::Like {
                expr: Box::new(ident("a")),
                pattern: Box::new(SQLExpr::Like {
                    expr: Box::new(ident("b")),
                    pattern: Box::new(SQLExpr::StringLiteral("c".into())),
                    not: false,
                }),
                not: false,
            }),
            "a LIKE (b LIKE 'c')"
        );
        // 同级左侧：a IS NULL IS NULL 左结合，不补括号（多补只是冗余，不是错误）
        assert_eq!(
            format_expr(&SQLExpr::IsNull {
                expr: Box::new(SQLExpr::IsNull {
                    expr: Box::new(ident("a")),
                    not: false,
                }),
                not: false,
            }),
            "a IS NULL IS NULL"
        );
    }

    #[test]
    fn test_format_variable_quoting() {
        // 普通变量名原样输出；带空格/引号的必须回写成 @'…'，否则会变成「变量 + 别名」
        assert_eq!(format_expr(&SQLExpr::Variable("@x".into())), "@x");
        assert_eq!(
            format_expr(&SQLExpr::Variable("@@GLOBAL.sql_mode".into())),
            "@@GLOBAL.sql_mode"
        );
        assert_eq!(format_expr(&SQLExpr::Variable("@a b".into())), "@'a b'");
        assert_eq!(format_expr(&SQLExpr::Variable("@a'b".into())), "@'a''b'");
        assert_eq!(format_expr(&SQLExpr::Variable("@@a b".into())), "@@'a b'");
        assert_eq!(format_expr(&SQLExpr::Variable("@".into())), "@");
        // 回写结果必须能重新解析回同名变量
        let stmts = crate::parser::parse_sql("SELECT @'a b'").unwrap();
        let SQLStatement::Select(s) = &stmts[0] else {
            panic!("not select")
        };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Variable("@a b".into()), None)
        );
    }

    #[test]
    fn test_format_join_table_reference() {
        use crate::ast::*;
        // TableReference::Join 不能格式化成字面量 "..."
        let tr = TableReference::Join(Box::new(JoinClause {
            join_type: JoinType::Left,
            table: TableReference::Table {
                name: "b".into(),
                alias: None,
                schema: None,
            },
            on: SQLExpr::BinaryOp {
                left: Box::new(SQLExpr::Identifier(vec!["a".into(), "id".into()])),
                op: BinaryOpType::Eq,
                right: Box::new(SQLExpr::Identifier(vec!["b".into(), "id".into()])),
            },
        }));
        let stmt = SQLStatement::Select(Box::new(SelectStatement {
            with_cte: vec![],
            distinct: false,
            columns: vec![SelectItem::Wildcard(None)],
            from: Some(tr),
            joins: vec![],
            where_clause: None,
            group_by: vec![],
            having: None,
            order_by: vec![],
            limit: None,
            offset: None,
        }));
        let out = format_statement(&stmt);
        assert_eq!(out, "SELECT * FROM LEFT JOIN b ON a.id = b.id");
        assert!(!out.contains("..."));
    }

    #[test]
    fn test_format_expr_from_parser() {
        // 解析器能产出的表达式走一遍完整管道
        let out = fmt("SELECT a FROM t \
             WHERE a BETWEEN 1 AND 2 AND b NOT IN (1, 2) \
             AND c LIKE 'x%' AND d IS NULL \
             AND EXISTS (SELECT 1 FROM u WHERE u.a = t.a)");
        assert_eq!(
            out,
            "SELECT a FROM t WHERE a BETWEEN 1 AND 2 \
             AND b NOT IN (1, 2) AND c LIKE 'x%' AND d IS NULL \
             AND EXISTS (SELECT 1 FROM u WHERE u.a = t.a)"
        );
        let out2 = fmt("SELECT a FROM t WHERE a IN (SELECT id FROM u)");
        assert_eq!(out2, "SELECT a FROM t WHERE a IN (SELECT id FROM u)");
    }

    #[test]
    fn test_format_roundtrip_all_statements() {
        let sqls = [
            "SELECT * FROM t",
            "SELECT DISTINCT a, b FROM t WHERE a > 1 GROUP BY a HAVING COUNT(*) > 1",
            "INSERT INTO t (a) VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "CREATE TABLE t (id INT)",
            "DROP TABLE t",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "SELECT (a + b) * c FROM t",
        ];
        for sql in sqls {
            let stmts = parse_sql(sql).unwrap();
            let formatted = format_statement(&stmts[0]);
            let reparsed = parse_sql(&formatted);
            assert!(
                reparsed.is_ok(),
                "roundtrip failed for: {sql} -> {formatted}"
            );
        }
    }

    #[test]
    fn test_format_subquery_in_from() {
        let out = fmt("SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
        assert_eq!(out, "SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
    }
}
