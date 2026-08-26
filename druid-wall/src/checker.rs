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
                    for cte in &s.with_cte {
                        if let Some(r) = visit_stmt(&SQLStatement::Select(cte.query.clone()), deny)
                        {
                            return Some(r);
                        }
                    }
                    for item in &s.columns {
                        if let druid_sql::ast::SelectItem::Expr(e, _) = item {
                            if let Some(r) = visit(e, deny) {
                                return Some(r);
                            }
                        }
                    }
                    if let Some(druid_sql::ast::TableReference::SubQuery(stmt, _)) = &s.from {
                        if let Some(r) = visit_stmt(stmt, deny) {
                            return Some(r);
                        }
                    }
                    for join in &s.joins {
                        if let Some(r) = visit(&join.on, deny) {
                            return Some(r);
                        }
                        if let druid_sql::ast::TableReference::SubQuery(stmt, _) = &join.table {
                            if let Some(r) = visit_stmt(stmt, deny) {
                                return Some(r);
                            }
                        }
                    }
                    if let Some(ref w) = s.where_clause {
                        if let Some(r) = visit(w, deny) {
                            return Some(r);
                        }
                    }
                    for e in &s.group_by {
                        if let Some(r) = visit(e, deny) {
                            return Some(r);
                        }
                    }
                    if let Some(ref h) = s.having {
                        if let Some(r) = visit(h, deny) {
                            return Some(r);
                        }
                    }
                    for o in &s.order_by {
                        if let Some(r) = visit(&o.expr, deny) {
                            return Some(r);
                        }
                    }
                    if let Some(ref l) = s.limit {
                        if let Some(r) = visit(l, deny) {
                            return Some(r);
                        }
                    }
                    if let Some(ref off) = s.offset {
                        if let Some(r) = visit(off, deny) {
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
        // 剔除单引号字符串字面量：字符串/注释内的单词不应命中关键字
        let mut bare = String::new();
        let mut in_str = false;
        for c in s.chars() {
            if c == '\'' {
                in_str = !in_str;
            } else if !in_str {
                bare.push(c);
            }
        }
        // 函数名匹配忽略空白：SLEEP (1) 与 SLEEP(1) 同样拦截
        let compact: String = bare.chars().filter(|c| !c.is_whitespace()).collect();
        for func in &self.config.deny_functions {
            if compact.contains(&format!("{}(", func.to_lowercase())) {
                return WallCheckResult::deny(format!("forbidden: {}", func));
            }
        }
        for kw in &self.config.deny_keywords {
            let kw_lower = kw.to_lowercase();
            if let Some(pos) = bare.find(&kw_lower) {
                let before = pos == 0 || {
                    let c = bare.as_bytes()[pos - 1];
                    !c.is_ascii_alphanumeric() && c != b'_'
                };
                let after = {
                    let end = pos + kw_lower.len();
                    end >= bare.len() || {
                        let c = bare.as_bytes()[end];
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

    /// 解析单条语句并执行完整 AST 检查
    fn check_one(c: &WallChecker, sql: &str) -> WallCheckResult {
        let stmts = parse_sql(sql).expect("sql should parse");
        c.check(sql, &stmts[0])
    }

    // ── 基本放行/拦截 ──

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

    // ── 操作拦截 ──

    #[test]
    fn test_deny_insert_when_configured() {
        let cfg = WallConfig {
            deny_operations: vec![DenyOperation::Insert],
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        let r = check_one(&c, "INSERT INTO users (id) VALUES (1)");
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "INSERT denied");
    }

    #[test]
    fn test_deny_update_and_delete_default_require_where() {
        let c = WallChecker::new(WallConfig::default());
        // 默认 update_delete_require_where = true
        let r = check_one(&c, "UPDATE users SET name='x'");
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "UPDATE without WHERE");
        let r = check_one(&c, "DELETE FROM users");
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "DELETE without WHERE");
        // 带 WHERE 则放行
        assert!(check_one(&c, "UPDATE users SET name='x' WHERE id=1").allowed);
        assert!(check_one(&c, "DELETE FROM users WHERE id=1").allowed);
    }

    #[test]
    fn test_require_where_disabled() {
        let cfg = WallConfig {
            update_delete_require_where: false,
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        assert!(check_one(&c, "UPDATE users SET name='x'").allowed);
        assert!(check_one(&c, "DELETE FROM users").allowed);
    }

    #[test]
    fn test_deny_update_operation() {
        let cfg = WallConfig {
            deny_operations: vec![DenyOperation::Update],
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        assert!(!check_one(&c, "UPDATE users SET name='x' WHERE id=1").allowed);
    }

    #[test]
    fn test_create_table_deny_when_configured() {
        let cfg = WallConfig {
            deny_operations: vec![DenyOperation::CreateTable],
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        assert!(!check_one(&c, "CREATE TABLE t (id INT)").allowed);
        // 默认配置下 CREATE TABLE 放行（默认只拒绝 TRUNCATE/DROP/ALTER）
        let c = WallChecker::new(WallConfig::default());
        assert!(check_one(&c, "CREATE TABLE t (id INT)").allowed);
    }

    // ── 函数拦截（AST 递归） ──

    #[test]
    fn test_sleep_in_where_denied() {
        let c = WallChecker::new(WallConfig::default());
        let r = check_one(&c, "SELECT id FROM users WHERE id=SLEEP(1)");
        assert!(!r.allowed);
        assert!(r.violations[0].message.contains("forbidden function"));
    }

    #[test]
    fn test_deny_function_case_insensitive() {
        let c = WallChecker::new(WallConfig::default());
        assert!(!check_one(&c, "SELECT sleep(5)").allowed);
        assert!(!check_one(&c, "SELECT benchMark(1000000, md5('x'))").allowed);
    }

    #[test]
    fn test_deny_function_in_subquery_and_case_when() {
        let c = WallChecker::new(WallConfig::default());
        assert!(!check_one(
            &c,
            "SELECT id FROM users WHERE id IN (SELECT id FROM t WHERE x=SLEEP(1))"
        )
        .allowed);
        assert!(!check_one(
            &c,
            "SELECT CASE WHEN SLEEP(1)=1 THEN 1 ELSE 0 END FROM users"
        )
        .allowed);
    }

    #[test]
    fn test_similar_function_name_allowed() {
        // 只拦截完全匹配的函数名
        let c = WallChecker::new(WallConfig::default());
        assert!(check_one(&c, "SELECT SLEEPLESS(1)").allowed);
        assert!(check_one(&c, "SELECT my_sleep(1)").allowed);
    }

    // ── quick_check（纯文本） ──

    #[test]
    fn test_quick_check_keyword_boundary() {
        let cfg = WallConfig {
            deny_keywords: vec!["drop".into()],
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        // 词边界：drop 在单词中间不应命中
        assert!(c.quick_check("SELECT * FROM dropdown").allowed);
        assert!(c.quick_check("SELECT * FROM x WHERE y='not drop here'").allowed);
        assert!(!c.quick_check("SELECT * FROM users DROP").allowed);
    }

    #[test]
    fn test_quick_check_function_case_insensitive() {
        let c = WallChecker::new(WallConfig::default());
        assert!(!c.quick_check("select sleep (1)").allowed); // 括号前带空格同样拦截
        assert!(!c.quick_check("SELECT SLEEP(10)").allowed);
        assert!(c.quick_check("SELECT * FROM sleeping_table").allowed);
        // 字符串字面量里的函数名不拦截
        assert!(c.quick_check("SELECT 'sleep(1)'").allowed);
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

    #[test]
    fn test_ast_check_sql_too_long() {
        let cfg = WallConfig {
            max_sql_length: 10,
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        let long = "SELECT 123456789012345";
        let s = parse_sql(long).unwrap();
        let r = c.check(long, &s[0]);
        assert!(!r.allowed);
        assert!(r.violations[0].message.contains("max"));
    }

    #[test]
    fn test_into_outfile_denied_by_default() {
        let c = WallChecker::new(WallConfig::default());
        // parser 不支持 INTO 子句，checker 通过原始 SQL 文本拦截
        let s = parse_sql("SELECT * FROM users").unwrap();
        let r = c.check("SELECT * FROM users INTO OUTFILE '/tmp/x'", &s[0]);
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "INTO OUTFILE denied");
        let cfg = WallConfig {
            select_into_outfile_allow: true,
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        let r = c.check("SELECT * FROM users INTO OUTFILE '/tmp/x'", &s[0]);
        assert!(r.allowed);
    }

    #[test]
    fn test_select_deny_when_configured() {
        let cfg = WallConfig {
            deny_operations: vec![DenyOperation::Select],
            ..Default::default()
        };
        let c = WallChecker::new(cfg);
        let r = check_one(&c, "SELECT 1");
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "SELECT denied");
    }

    #[test]
    fn test_violation_and_result_helpers() {
        let r = WallCheckResult::pass();
        assert!(r.allowed);
        assert!(r.violations.is_empty());
        let r = WallCheckResult::deny("nope".into());
        assert!(!r.allowed);
        assert_eq!(r.violations[0].message, "nope");
        assert_eq!(Violation::new("m").message, "m");
    }
}
