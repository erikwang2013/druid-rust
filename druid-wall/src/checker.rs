use crate::config::{DenyOperation, WallConfig};
use druid_sql::ast::SQLStatement;

#[derive(Debug, Clone)]
pub struct WallCheckResult {
    pub allowed: bool,
    pub violations: Vec<Violation>,
}
impl WallCheckResult {
    pub fn pass() -> Self {
        WallCheckResult {
            allowed: true,
            violations: vec![],
        }
    }
    pub fn deny(msg: String) -> Self {
        WallCheckResult {
            allowed: false,
            violations: vec![Violation::new(&msg)],
        }
    }
}
#[derive(Debug, Clone)]
pub struct Violation {
    pub message: String,
}
impl Violation {
    pub fn new(msg: &str) -> Self {
        Violation {
            message: msg.into(),
        }
    }
}

pub struct WallChecker {
    config: WallConfig,
}

impl WallChecker {
    pub fn new(config: WallConfig) -> Self {
        WallChecker { config }
    }

    pub fn check(&self, sql: &str, stmt: &SQLStatement) -> WallCheckResult {
        if !self.config.enabled {
            return WallCheckResult::pass();
        }
        if sql.len() > self.config.max_sql_length {
            return WallCheckResult::deny(format!(
                "SQL len {} > max {}",
                sql.len(),
                self.config.max_sql_length
            ));
        }
        if let Some(v) = self.check_functions(stmt) {
            return v;
        }
        match stmt {
            SQLStatement::Select(s) => self.check_select(sql, s),
            SQLStatement::Insert(_) => self.check_op(&DenyOperation::Insert),
            SQLStatement::Update(s) => self.check_update(s),
            SQLStatement::Delete(s) => self.check_delete(s),
            SQLStatement::CreateTable(_) => self.check_op(&DenyOperation::CreateTable),
            SQLStatement::DropObject(_) => self.check_op(&DenyOperation::DropTable),
        }
    }

    fn check_functions(&self, stmt: &SQLStatement) -> Option<WallCheckResult> {
        use druid_sql::ast::SQLExpr;
        fn visit(expr: &SQLExpr, deny: &[String]) -> Option<WallCheckResult> {
            match expr {
                SQLExpr::Function { name, .. } | SQLExpr::Aggregate { name, .. } => {
                    let upper = name.to_uppercase();
                    if deny.iter().any(|f| f.to_uppercase() == upper) {
                        return Some(WallCheckResult::deny(format!(
                            "forbidden function: {}",
                            name
                        )));
                    }
                }
                SQLExpr::BinaryOp { left, right, .. } => {
                    if let Some(r) = visit(left, deny) {
                        return Some(r);
                    }
                    if let Some(r) = visit(right, deny) {
                        return Some(r);
                    }
                }
                SQLExpr::UnaryOp { expr, .. }
                | SQLExpr::Nested(expr)
                | SQLExpr::IsNull { expr, .. }
                | SQLExpr::Cast { expr, .. } => {
                    if let Some(r) = visit(expr, deny) {
                        return Some(r);
                    }
                }
                SQLExpr::InList { expr, list, .. } => {
                    if let Some(r) = visit(expr, deny) {
                        return Some(r);
                    }
                    for item in list {
                        if let Some(r) = visit(item, deny) {
                            return Some(r);
                        }
                    }
                }
                SQLExpr::Between {
                    expr, low, high, ..
                } => {
                    if let Some(r) = visit(expr, deny) {
                        return Some(r);
                    }
                    if let Some(r) = visit(low, deny) {
                        return Some(r);
                    }
                    if let Some(r) = visit(high, deny) {
                        return Some(r);
                    }
                }
                SQLExpr::Like { expr, pattern, .. } => {
                    if let Some(r) = visit(expr, deny) {
                        return Some(r);
                    }
                    if let Some(r) = visit(pattern, deny) {
                        return Some(r);
                    }
                }
                SQLExpr::Case {
                    expr: case_expr,
                    whens,
                    else_expr,
                } => {
                    if let Some(e) = case_expr {
                        if let Some(r) = visit(e, deny) {
                            return Some(r);
                        }
                    }
                    for (cond, result) in whens {
                        if let Some(r) = visit(cond, deny) {
                            return Some(r);
                        }
                        if let Some(r) = visit(result, deny) {
                            return Some(r);
                        }
                    }
                    if let Some(e) = else_expr {
                        if let Some(r) = visit(e, deny) {
                            return Some(r);
                        }
                    }
                }
                SQLExpr::SubQuery(s)
                | SQLExpr::Exists(s, _)
                | SQLExpr::InSubQuery { query: s, .. } => {
                    return visit_stmt(s, deny);
                }
                SQLExpr::WindowFunction {
                    function,
                    partition_by,
                    order_by,
                } => {
                    if let Some(r) = visit(function, deny) {
                        return Some(r);
                    }
                    for p in partition_by {
                        if let Some(r) = visit(p, deny) {
                            return Some(r);
                        }
                    }
                    for o in order_by {
                        if let Some(r) = visit(&o.expr, deny) {
                            return Some(r);
                        }
                    }
                }
                _ => {}
            }
            None
        }
        fn visit_stmt(stmt: &SQLStatement, deny: &[String]) -> Option<WallCheckResult> {
            match stmt {
                SQLStatement::Select(s) => {
                    for item in &s.columns {
                        if let druid_sql::ast::SelectItem::Expr(e, _) = item {
                            if let Some(r) = visit(e, deny) {
                                return Some(r);
                            }
                        }
                    }
                    if let Some(ref w) = s.where_clause {
                        if let Some(r) = visit(w, deny) {
                            return Some(r);
                        }
                    }
                    for join in &s.joins {
                        if let Some(r) = visit(&join.on, deny) {
                            return Some(r);
                        }
                    }
                }
                SQLStatement::Insert(s) => {
                    for row in &s.values {
                        for val in row {
                            if let Some(r) = visit(val, deny) {
                                return Some(r);
                            }
                        }
                    }
                }
                SQLStatement::Update(s) => {
                    for (_, val) in &s.sets {
                        if let Some(r) = visit(val, deny) {
                            return Some(r);
                        }
                    }
                    if let Some(ref w) = s.where_clause {
                        if let Some(r) = visit(w, deny) {
                            return Some(r);
                        }
                    }
                }
                SQLStatement::Delete(s) => {
                    if let Some(ref w) = s.where_clause {
                        if let Some(r) = visit(w, deny) {
                            return Some(r);
                        }
                    }
                }
                _ => {}
            }
            None
        }
        visit_stmt(stmt, &self.config.deny_functions)
    }

    fn check_op(&self, op: &DenyOperation) -> WallCheckResult {
        if self.config.deny_operations.contains(op) {
            WallCheckResult::deny(format!("{} denied", op))
        } else {
            WallCheckResult::pass()
        }
    }

    fn check_select(&self, sql: &str, _: &druid_sql::ast::SelectStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Select) {
            return WallCheckResult::deny("SELECT denied".into());
        }
        if !self.config.select_into_outfile_allow && sql.to_lowercase().contains("into outfile") {
            return WallCheckResult::deny("INTO OUTFILE denied".into());
        }
        WallCheckResult::pass()
    }

    fn check_update(&self, s: &druid_sql::ast::UpdateStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Update) {
            return WallCheckResult::deny("UPDATE denied".into());
        }
        if self.config.update_delete_require_where && s.where_clause.is_none() {
            return WallCheckResult::deny("UPDATE without WHERE".into());
        }
        WallCheckResult::pass()
    }

    fn check_delete(&self, s: &druid_sql::ast::DeleteStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Delete) {
            return WallCheckResult::deny("DELETE denied".into());
        }
        if self.config.update_delete_require_where && s.where_clause.is_none() {
            return WallCheckResult::deny("DELETE without WHERE".into());
        }
        WallCheckResult::pass()
    }

    pub fn quick_check(&self, sql: &str) -> WallCheckResult {
        if !self.config.enabled {
            return WallCheckResult::pass();
        }
        if sql.len() > self.config.max_sql_length {
            return WallCheckResult::deny("SQL too long".into());
        }
        let s = sql.trim().to_lowercase();
        for func in &self.config.deny_functions {
            if s.contains(&format!("{}(", func.to_lowercase())) {
                return WallCheckResult::deny(format!("forbidden: {}", func));
            }
        }
        for kw in &self.config.deny_keywords {
            let kw_lower = kw.to_lowercase();
            if let Some(pos) = s.find(&kw_lower) {
                let before = pos == 0 || {
                    let c = s.as_bytes()[pos - 1];
                    !c.is_ascii_alphanumeric() && c != b'_'
                };
                let after = {
                    let end = pos + kw_lower.len();
                    end >= s.len() || {
                        let c = s.as_bytes()[end];
                        !c.is_ascii_alphanumeric() && c != b'_'
                    }
                };
                if before && after {
                    return WallCheckResult::deny(format!("forbidden: {}", kw));
                }
            }
        }
        WallCheckResult::pass()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use druid_sql::parse_sql;
    #[test]
    fn test_allow() {
        let c = WallChecker::new(WallConfig::default());
        let s = parse_sql("SELECT id FROM users WHERE id=1").unwrap();
        assert!(c.check("SELECT id FROM users WHERE id=1", &s[0]).allowed);
    }
    #[test]
    fn test_deny_drop() {
        let c = WallChecker::new(WallConfig::default());
        let s = parse_sql("DROP TABLE users").unwrap();
        assert!(!c.check("DROP TABLE users", &s[0]).allowed);
    }
    #[test]
    fn test_forbid() {
        let c = WallChecker::new(WallConfig::default());
        assert!(!c.quick_check("SELECT SLEEP(10)").allowed);
    }
    #[test]
    fn test_disabled() {
        let cfg = WallConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(WallChecker::new(cfg).quick_check("DROP TABLE x").allowed);
    }
}
