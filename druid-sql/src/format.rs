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
                let mut def = cte.name.clone();
                if !cte.columns.is_empty() {
                    def.push_str(&format!(" ({})", cte.columns.join(", ")));
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
                    es.push_str(a);
                }
                es
            }
            SelectItem::Wildcard(Some(t)) => format!("{}.*", t),
            SelectItem::Wildcard(None) => "*".to_string(),
        })
        .collect();
    s.push_str(&cols.join(", "));

    if let Some(ref from) = stmt.from {
        s.push_str(" FROM ");
        s.push_str(&format_table_ref(from));
    }

    for join in &stmt.joins {
        write!(s, " {} {}", join.join_type, format_table_ref(&join.table)).unwrap();
        write!(s, " ON {}", format_expr(&join.on)).unwrap();
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
    write!(s, " INTO {}", stmt.table).unwrap();

    if !stmt.columns.is_empty() {
        write!(s, " ({})", stmt.columns.join(", ")).unwrap();
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
    let mut s = format!("UPDATE {}", stmt.table);
    let sets: Vec<String> = stmt
        .sets
        .iter()
        .map(|(col, val)| format!("{} = {}", col, format_expr(val)))
        .collect();
    write!(s, " SET {}", sets.join(", ")).unwrap();
    if let Some(ref w) = stmt.where_clause {
        write!(s, " WHERE {}", format_expr(w)).unwrap();
    }
    s
}

fn format_delete(stmt: &DeleteStatement) -> String {
    let mut s = format!("DELETE FROM {}", stmt.table);
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
    write!(s, "{} (", stmt.table).unwrap();
    let cols: Vec<String> = stmt
        .columns
        .iter()
        .map(|c| format!("{} {}", c.name, c.data_type))
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
    s.push_str(&stmt.name);
    s
}

fn format_table_ref(tr: &TableReference) -> String {
    match tr {
        TableReference::Table {
            name,
            alias,
            schema,
        } => {
            let mut s = if let Some(sch) = schema {
                format!("{}.{}", sch, name)
            } else {
                name.clone()
            };
            if let Some(a) = alias {
                write!(s, " {}", a).unwrap();
            }
            s
        }
        TableReference::SubQuery(stmt, alias) => {
            format!("({}) {}", format_statement(stmt), alias)
        }
        TableReference::Join(_) => "...".to_string(),
    }
}

pub fn format_expr(expr: &SQLExpr) -> String {
    match expr {
        SQLExpr::Identifier(parts) => parts.join("."),
        SQLExpr::StringLiteral(s) => format!("'{}'", s),
        SQLExpr::NumberLiteral(n) => n.clone(),
        SQLExpr::Null => "NULL".to_string(),
        SQLExpr::Placeholder => "?".to_string(),
        SQLExpr::Wildcard => "*".to_string(),
        SQLExpr::BinaryOp { left, op, right } => {
            format!("{} {} {}", format_expr(left), op, format_expr(right))
        }
        SQLExpr::UnaryOp { op, expr } => {
            let op_str = match op {
                UnaryOpType::Not => "NOT ",
                UnaryOpType::Neg => "-",
                UnaryOpType::Plus => "+",
            };
            format!("{}{}", op_str, format_expr(expr))
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
            format!("{} {}IN ({})", format_expr(expr), n, items.join(", "))
        }
        SQLExpr::Between {
            expr,
            low,
            high,
            not,
        } => {
            let n = if *not { "NOT " } else { "" };
            format!(
                "{} {}BETWEEN {} AND {}",
                format_expr(expr),
                n,
                format_expr(low),
                format_expr(high)
            )
        }
        SQLExpr::IsNull { expr, not } => {
            let n = if *not { "NOT " } else { "" };
            format!("{} IS {}NULL", format_expr(expr), n)
        }
        SQLExpr::Like { expr, pattern, not } => {
            let n = if *not { "NOT " } else { "" };
            format!("{} {}LIKE {}", format_expr(expr), n, format_expr(pattern))
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
                format_expr(expr),
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
        let out = fmt(
            "SELECT DISTINCT t.a, COUNT(*) AS cnt, b + 1 FROM db.t x \
             LEFT JOIN u ON x.id = u.id WHERE x.a > 1 AND x.b IS NOT NULL \
             GROUP BY t.a HAVING COUNT(*) > 2 ORDER BY t.a DESC LIMIT 10",
        );
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
        assert_eq!(
            fmt("CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, name VARCHAR(20) NOT NULL)"),
            "CREATE TABLE IF NOT EXISTS t (id INT, name VARCHAR(20))"
        );
        // 格式化后列定义不保留 nullable/primary key 细节 — 验证可重新解析
        assert!(parse_sql(&fmt("CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY)")).is_ok());
        assert_eq!(
            fmt("DROP TABLE IF EXISTS t"),
            "DROP TABLE IF EXISTS t"
        );
        assert_eq!(fmt("DROP VIEW v"), "DROP VIEW v");
        assert_eq!(fmt("DROP INDEX i"), "DROP INDEX i");
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
        assert_eq!(format_expr(&SQLExpr::StringLiteral("it's".into())), "'it's'");
        assert_eq!(
            format_expr(&SQLExpr::Identifier(vec!["a".into(), "b".into()])),
            "a.b"
        );
    }

    #[test]
    fn test_format_expr_from_parser() {
        // 解析器能产出的表达式走一遍完整管道
        let out = fmt(
            "SELECT a FROM t \
             WHERE a BETWEEN 1 AND 2 AND b NOT IN (1, 2) \
             AND c LIKE 'x%' AND d IS NULL \
             AND EXISTS (SELECT 1 FROM u WHERE u.a = t.a)",
        );
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
            assert!(reparsed.is_ok(), "roundtrip failed for: {sql} -> {formatted}");
        }
    }

    #[test]
    fn test_format_subquery_in_from() {
        let out = fmt("SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
        assert_eq!(out, "SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
    }
}
