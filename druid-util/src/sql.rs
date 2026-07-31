use druid_core::DbType;

pub fn detect_db_type_from_url(url: &str) -> Option<DbType> {
    let u = url.to_lowercase();
    if u.contains("mysql") { Some(DbType::MySQL) }
    else if u.contains("postgresql") || u.contains("postgres") { Some(DbType::PostgreSQL) }
    else if u.contains("oracle") { Some(DbType::Oracle) }
    else if u.contains("sqlserver") || u.contains("mssql") { Some(DbType::SqlServer) }
    else if u.contains("db2") { Some(DbType::DB2) }
    else if u.contains("h2") { Some(DbType::H2) }
    else if u.contains("clickhouse") { Some(DbType::ClickHouse) }
    else if u.contains("doris") { Some(DbType::Doris) }
    else if u.contains("starrocks") { Some(DbType::StarRocks) }
    else if u.contains("hive") { Some(DbType::Hive) }
    else if u.contains("presto") { Some(DbType::Presto) }
    else if u.contains("impala") { Some(DbType::Impala) }
    else if u.contains("snowflake") { Some(DbType::Snowflake) }
    else if u.contains("bigquery") { Some(DbType::BigQuery) }
    else if u.contains("redshift") { Some(DbType::Redshift) }
    else if u.contains("spark") { Some(DbType::Spark) }
    else if u.contains("phoenix") { Some(DbType::Phoenix) }
    else if u.contains("teradata") { Some(DbType::Teradata) }
    else if u.contains("informix") { Some(DbType::Informix) }
    else if u.contains("athena") { Some(DbType::Athena) }
    else if u.contains("gauss") { Some(DbType::GaussDB) }
    else if u.contains("dameng") { Some(DbType::DM) }
    else if u.contains("odps") || u.contains("maxcompute") { Some(DbType::ODPS) }
    else if u.contains("hologres") { Some(DbType::Hologres) }
    else { None }
}

pub fn is_select_sql(sql: &str) -> bool {
    let t = sql.trim().to_lowercase();
    t.starts_with("select") || t.starts_with("with")
}

pub fn is_write_sql(sql: &str) -> bool {
    let t = sql.trim().to_lowercase();
    t.starts_with("insert") || t.starts_with("update") || t.starts_with("delete")
        || t.starts_with("replace") || t.starts_with("merge")
}

pub fn is_ddl_sql(sql: &str) -> bool {
    let t = sql.trim().to_lowercase();
    t.starts_with("create") || t.starts_with("alter") || t.starts_with("drop")
        || t.starts_with("truncate") || t.starts_with("rename")
}

pub fn get_sql_type(sql: &str) -> &'static str {
    let trimmed = sql.trim().to_lowercase();
    if trimmed.is_empty() { return "EMPTY"; }
    match trimmed.split_whitespace().next().unwrap_or("") {
        "select" | "with" => "SELECT",
        "insert" => "INSERT",
        "update" => "UPDATE",
        "delete" => "DELETE",
        "create" => "CREATE",
        "alter" => "ALTER",
        "drop" => "DROP",
        "truncate" => "TRUNCATE",
        "merge" | "replace" => "MERGE",
        "explain" | "desc" | "describe" => "EXPLAIN",
        "show" => "SHOW",
        "set" => "SET",
        "begin" | "start" => "TRANSACTION",
        "commit" => "COMMIT",
        "rollback" => "ROLLBACK",
        "grant" | "revoke" => "DCL",
        "call" | "execute" => "CALL",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_db_type() {
        assert_eq!(detect_db_type_from_url("jdbc:mysql://localhost/test"), Some(DbType::MySQL));
        assert_eq!(detect_db_type_from_url("jdbc:postgresql://localhost/test"), Some(DbType::PostgreSQL));
        assert!(detect_db_type_from_url("jdbc:unknown://localhost/test").is_none());
    }

    #[test]
    fn test_sql_types() {
        assert_eq!(get_sql_type("SELECT * FROM users"), "SELECT");
        assert_eq!(get_sql_type("INSERT INTO users VALUES (1)"), "INSERT");
        assert_eq!(get_sql_type(""), "EMPTY");
    }

    #[test]
    fn test_is_select() {
        assert!(is_select_sql("SELECT * FROM t"));
        assert!(is_select_sql("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(!is_select_sql("INSERT INTO t VALUES (1)"));
    }

    #[test]
    fn test_is_write() {
        assert!(is_write_sql("INSERT INTO t VALUES (1)"));
        assert!(!is_write_sql("SELECT * FROM t"));
    }
}
