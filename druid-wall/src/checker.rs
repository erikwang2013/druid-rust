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
        match stmt {
            SQLStatement::Select(s) => self.check_select(sql, s),
            SQLStatement::Insert(_) => self.check_op(&DenyOperation::Insert),
            SQLStatement::Update(s) => self.check_update(s),
            SQLStatement::Delete(s) => self.check_delete(s),
            SQLStatement::CreateTable(_) => self.check_op(&DenyOperation::CreateTable),
            SQLStatement::DropTable(_) => self.check_op(&DenyOperation::DropTable),
        }
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
