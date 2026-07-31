use druid_core::DbType;

pub fn detect_db_type_from_url(url: &str) -> Option<DbType> {
    let u = url.to_lowercase();
    if scheme_contains(&u, "mysql") {
        Some(DbType::MySQL)
    } else if scheme_contains(&u, "postgresql") || scheme_contains(&u, "postgres") {
        Some(DbType::PostgreSQL)
    } else if scheme_contains(&u, "oracle") {
        Some(DbType::Oracle)
    } else if scheme_contains(&u, "sqlserver") || scheme_contains(&u, "mssql") {
        Some(DbType::SqlServer)
    } else if scheme_contains(&u, "db2") && !u.contains("not-db2") {
        Some(DbType::DB2)
    } else if u.starts_with("jdbc:h2") {
        Some(DbType::H2)
    } else if scheme_contains(&u, "clickhouse") {
        Some(DbType::ClickHouse)
    } else if scheme_contains(&u, "doris") {
        Some(DbType::Doris)
    } else if scheme_contains(&u, "starrocks") {
        Some(DbType::StarRocks)
    } else if scheme_contains(&u, "hive") {
        Some(DbType::Hive)
    } else if scheme_contains(&u, "presto") {
        Some(DbType::Presto)
    } else if scheme_contains(&u, "impala") {
        Some(DbType::Impala)
    } else if scheme_contains(&u, "snowflake") {
        Some(DbType::Snowflake)
    } else if scheme_contains(&u, "bigquery") {
        Some(DbType::BigQuery)
    } else if scheme_contains(&u, "redshift") {
        Some(DbType::Redshift)
    } else if scheme_contains(&u, "spark") {
        Some(DbType::Spark)
    } else if scheme_contains(&u, "phoenix") {
        Some(DbType::Phoenix)
    } else if scheme_contains(&u, "teradata") {
        Some(DbType::Teradata)
    } else if scheme_contains(&u, "informix") {
        Some(DbType::Informix)
    } else if scheme_contains(&u, "athena") {
        Some(DbType::Athena)
    } else if scheme_contains(&u, "gauss") {
        Some(DbType::GaussDB)
    } else if scheme_contains(&u, "dameng") {
        Some(DbType::DM)
    } else if scheme_contains(&u, "odps") || scheme_contains(&u, "maxcompute") {
        Some(DbType::ODPS)
    } else if scheme_contains(&u, "hologres") {
        Some(DbType::Hologres)
    } else {
        None
    }
}

fn scheme_contains(url: &str, pat: &str) -> bool {
    if let Some(colon_idx) = url.find("://") {
        url[..colon_idx].contains(pat)
    } else {
        false
    }
}

pub fn is_select_sql(sql: &str) -> bool {
    let t = sql.trim_start();
    t.len() >= 6 && t[..6].eq_ignore_ascii_case("select")
        || t.len() >= 4 && t[..4].eq_ignore_ascii_case("with")
}

pub fn is_write_sql(sql: &str) -> bool {
    let t = sql.trim_start();
    starts_with_any_ignore_case(t, &["insert", "update", "delete", "replace", "merge"])
}

pub fn is_ddl_sql(sql: &str) -> bool {
    let t = sql.trim_start();
    starts_with_any_ignore_case(t, &["create", "alter", "drop", "truncate", "rename"])
}

fn starts_with_any_ignore_case(s: &str, prefixes: &[&str]) -> bool {
    let s_lower = s.to_ascii_lowercase();
    prefixes.iter().any(|p| s_lower.starts_with(p))
}

pub fn get_sql_type(sql: &str) -> &'static str {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return "EMPTY";
    }
    let first = match trimmed.split_whitespace().next() {
        Some(w) => w,
        None => return "OTHER",
    };
    match first.to_ascii_lowercase().as_str() {
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
        "begin" => "TRANSACTION",
        "start" => {
            if trimmed.len() > 6
                && trimmed[6..]
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("transaction")
            {
                "TRANSACTION"
            } else {
                "OTHER"
            }
        }
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
        assert_eq!(
            detect_db_type_from_url("jdbc:mysql://localhost/test"),
            Some(DbType::MySQL)
        );
        assert_eq!(
            detect_db_type_from_url("jdbc:postgresql://localhost/test"),
            Some(DbType::PostgreSQL)
        );
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
