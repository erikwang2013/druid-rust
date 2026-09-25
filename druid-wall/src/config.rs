use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallConfig {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub deny_operations: Vec<DenyOperation>,
    #[serde(default)]
    pub deny_functions: Vec<String>,
    #[serde(default)]
    pub deny_schemas: Vec<String>,
    #[serde(default = "default_max")]
    pub max_sql_length: usize,
    #[serde(default)]
    pub allow_multi_statements: bool,
    #[serde(default)]
    pub deny_keywords: Vec<String>,
    #[serde(default)]
    pub update_delete_require_where: bool,
    #[serde(default)]
    pub select_into_outfile_allow: bool,
}
fn default_name() -> String {
    "wall".into()
}
fn default_true() -> bool {
    true
}
fn default_max() -> usize {
    8192
}
impl Default for WallConfig {
    fn default() -> Self {
        WallConfig {
            name: default_name(),
            enabled: true,
            deny_operations: vec![
                DenyOperation::Truncate,
                DenyOperation::DropTable,
                DenyOperation::AlterTable,
            ],
            deny_functions: vec!["SLEEP".into(), "BENCHMARK".into(), "LOAD_FILE".into()],
            deny_schemas: vec![],
            max_sql_length: default_max(),
            allow_multi_statements: false,
            deny_keywords: vec![],
            update_delete_require_where: true,
            select_into_outfile_allow: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyOperation {
    Select,
    Insert,
    Update,
    Delete,
    Truncate,
    DropTable,
    AlterTable,
    CreateTable,
    CreateIndex,
    Grant,
    Revoke,
    Call,
    Execute,
}
impl std::fmt::Display for DenyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenyOperation::Select => write!(f, "SELECT"),
            DenyOperation::Insert => write!(f, "INSERT"),
            DenyOperation::Update => write!(f, "UPDATE"),
            DenyOperation::Delete => write!(f, "DELETE"),
            DenyOperation::Truncate => write!(f, "TRUNCATE"),
            DenyOperation::DropTable => write!(f, "DROP TABLE"),
            DenyOperation::AlterTable => write!(f, "ALTER TABLE"),
            DenyOperation::CreateTable => write!(f, "CREATE TABLE"),
            DenyOperation::CreateIndex => write!(f, "CREATE INDEX"),
            DenyOperation::Grant => write!(f, "GRANT"),
            DenyOperation::Revoke => write!(f, "REVOKE"),
            DenyOperation::Call => write!(f, "CALL"),
            DenyOperation::Execute => write!(f, "EXECUTE"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let c = WallConfig::default();
        assert_eq!(c.name, "wall");
        assert!(c.enabled);
        assert_eq!(c.max_sql_length, 8192);
        assert!(!c.allow_multi_statements);
        assert!(c.update_delete_require_where);
        assert!(!c.select_into_outfile_allow);
        // 默认拒绝 DDL 三类操作
        assert_eq!(
            c.deny_operations,
            vec![
                DenyOperation::Truncate,
                DenyOperation::DropTable,
                DenyOperation::AlterTable,
            ]
        );
        assert_eq!(
            c.deny_functions,
            vec![
                "SLEEP".to_string(),
                "BENCHMARK".to_string(),
                "LOAD_FILE".to_string()
            ]
        );
        assert!(c.deny_schemas.is_empty());
        assert!(c.deny_keywords.is_empty());
    }

    #[test]
    fn test_deny_operation_display() {
        assert_eq!(DenyOperation::Select.to_string(), "SELECT");
        assert_eq!(DenyOperation::DropTable.to_string(), "DROP TABLE");
        assert_eq!(DenyOperation::CreateIndex.to_string(), "CREATE INDEX");
        assert_eq!(DenyOperation::Execute.to_string(), "EXECUTE");
    }
}
