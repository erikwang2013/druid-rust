use crate::ast::*;
use std::fmt::Write;

/// 将 SQL AST 格式化输出为 SQL 字符串
pub fn format_statement(stmt: &SQLStatement) -> String {
    match stmt {
        SQLStatement::Select(s) => format_select(s, 0),
        SQLStatement::Insert(s) => format_insert(s),
        SQLStatement::Update(s) => format_update(s),
        SQLStatement::Delete(s) => format_delete(s),
        SQLStatement::CreateTable(s) => format_create_table(s),
        SQLStatement::DropTable(s) => format_drop_table(s),
    }
}

fn format_select(stmt: &SelectStatement, _indent: usize) -> String {
    let mut s = String::new();
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

fn format_drop_table(stmt: &DropTableStatement) -> String {
    let mut s = "DROP TABLE ".to_string();
    if stmt.if_exists {
        s.push_str("IF EXISTS ");
    }
    s.push_str(&stmt.table);
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
}
