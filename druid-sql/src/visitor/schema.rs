use std::collections::{HashMap, HashSet};
use crate::ast::*;

/// Schema 统计访问器 — 提取 SQL 中引用的表名和列名
#[derive(Debug, Default)]
pub struct SchemaVisitor {
    /// 表名集合
    pub tables: HashSet<String>,
    /// 表名 -> 列名集合
    pub columns: HashMap<String, HashSet<String>>,
}

impl SchemaVisitor {
    pub fn new() -> Self {
        SchemaVisitor { tables: HashSet::new(), columns: HashMap::new() }
    }

    /// 访问 SELECT 语句
    pub fn visit_select(&mut self, stmt: &SelectStatement) {
        if let Some(ref from) = stmt.from {
            self.visit_table_ref(from);
        }
        for join in &stmt.joins {
            self.visit_table_ref(&join.table);
        }
        for col in &stmt.columns {
            self.visit_select_item(col);
        }
        if let Some(ref where_clause) = stmt.where_clause {
            self.visit_expr(where_clause, "");
        }
        for expr in &stmt.group_by {
            self.visit_expr(expr, "");
        }
        for item in &stmt.order_by {
            self.visit_expr(&item.expr, "");
        }
    }

    /// 访问 INSERT 语句
    pub fn visit_insert(&mut self, stmt: &InsertStatement) {
        self.tables.insert(stmt.table.clone());
        for col in &stmt.columns {
            self.add_column(&stmt.table, col);
        }
    }

    /// 访问 UPDATE 语句
    pub fn visit_update(&mut self, stmt: &UpdateStatement) {
        self.tables.insert(stmt.table.clone());
        for (col, val) in &stmt.sets {
            self.add_column(&stmt.table, col);
            self.visit_expr(val, &stmt.table);
        }
        if let Some(ref where_clause) = stmt.where_clause {
            self.visit_expr(where_clause, &stmt.table);
        }
    }

    /// 访问 DELETE 语句
    pub fn visit_delete(&mut self, stmt: &DeleteStatement) {
        self.tables.insert(stmt.table.clone());
        if let Some(ref where_clause) = stmt.where_clause {
            self.visit_expr(where_clause, &stmt.table);
        }
    }

    /// 访问任意语句
    pub fn visit_statement(&mut self, stmt: &SQLStatement) {
        match stmt {
            SQLStatement::Select(s) => self.visit_select(s),
            SQLStatement::Insert(s) => self.visit_insert(s),
            SQLStatement::Update(s) => self.visit_update(s),
            SQLStatement::Delete(s) => self.visit_delete(s),
            SQLStatement::CreateTable(s) => {
                self.tables.insert(s.table.clone());
                for col in &s.columns {
                    self.add_column(&s.table, &col.name);
                }
            }
            SQLStatement::DropTable(s) => {
                self.tables.insert(s.table.clone());
            }
        }
    }

    fn visit_table_ref(&mut self, tr: &TableReference) {
        match tr {
            TableReference::Table { name, schema, .. } => {
                let full = if let Some(s) = schema {
                    format!("{}.{}", s, name)
                } else {
                    name.clone()
                };
                self.tables.insert(full);
            }
            TableReference::SubQuery(stmt, _alias) => {
                self.visit_statement(stmt);
            }
            TableReference::Join(_) => {}
        }
    }

    fn visit_select_item(&mut self, item: &SelectItem) {
        match item {
            SelectItem::Expr(expr, _) => { self.visit_expr(expr, ""); }
            SelectItem::Wildcard(_) => {}
        }
    }

    fn visit_expr(&mut self, expr: &SQLExpr, table: &str) {
        match expr {
            SQLExpr::Identifier(parts) => {
                if parts.len() == 2 {
                    self.add_column(&parts[0], &parts[1]);
                } else if parts.len() == 1 && !table.is_empty() {
                    self.add_column(table, &parts[0]);
                }
            }
            SQLExpr::BinaryOp { left, right, .. } => {
                self.visit_expr(left, table);
                self.visit_expr(right, table);
            }
            SQLExpr::Function { args, .. } => {
                for arg in args { self.visit_expr(arg, table); }
            }
            SQLExpr::InList { expr, list, .. } => {
                self.visit_expr(expr, table);
                for item in list { self.visit_expr(item, table); }
            }
            SQLExpr::Nested(inner) => { self.visit_expr(inner, table); }
            SQLExpr::Case { whens, else_expr, .. } => {
                for (cond, result) in whens {
                    self.visit_expr(cond, table);
                    self.visit_expr(result, table);
                }
                if let Some(e) = else_expr { self.visit_expr(e, table); }
            }
            SQLExpr::Aggregate { expr: inner, .. } => { self.visit_expr(inner, table); }
            SQLExpr::SubQuery(stmt) => { self.visit_statement(stmt); }
            SQLExpr::Exists(stmt, _) => { self.visit_statement(stmt); }
            _ => {}
        }
    }

    fn add_column(&mut self, table: &str, column: &str) {
        self.tables.insert(table.to_string());
        self.columns
            .entry(table.to_string())
            .or_default()
            .insert(column.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_sql;

    #[test]
    fn test_schema_visitor_select() {
        let sql = "SELECT u.id, u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id WHERE u.age > 18";
        let stmts = parse_sql(sql).unwrap();
        let mut visitor = SchemaVisitor::new();
        for stmt in &stmts {
            visitor.visit_statement(stmt);
        }
        assert!(visitor.tables.contains("users"));
        assert!(visitor.tables.contains("orders"));
        assert!(visitor.columns.get("u").unwrap().contains("id"));
        assert!(visitor.columns.get("u").unwrap().contains("name"));
        assert!(visitor.columns.get("o").unwrap().contains("total"));
    }

    #[test]
    fn test_schema_visitor_insert() {
        let sql = "INSERT INTO products (name, price) VALUES ('a', 10)";
        let stmts = parse_sql(sql).unwrap();
        let mut visitor = SchemaVisitor::new();
        for stmt in &stmts {
            visitor.visit_statement(stmt);
        }
        assert!(visitor.tables.contains("products"));
        assert!(visitor.columns.get("products").unwrap().contains("name"));
        assert!(visitor.columns.get("products").unwrap().contains("price"));
    }
}
