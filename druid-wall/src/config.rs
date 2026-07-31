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
