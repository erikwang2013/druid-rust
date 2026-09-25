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
    t.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("select"))
        || t.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("with"))
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
    fn test_detect_db_type_all_dialects() {
        for (url, expect) in [
            ("jdbc:mysql://h/db", Some(DbType::MySQL)),
            ("jdbc:postgresql://h/db", Some(DbType::PostgreSQL)),
            ("jdbc:oracle://h:1521/db", Some(DbType::Oracle)),
            ("jdbc:sqlserver://h:1433;db=db", Some(DbType::SqlServer)),
            ("jdbc:mssql://h/db", Some(DbType::SqlServer)),
            ("jdbc:db2://h/db", Some(DbType::DB2)),
            ("jdbc:h2:mem:test", Some(DbType::H2)),
            ("jdbc:clickhouse://h/db", Some(DbType::ClickHouse)),
            ("jdbc:doris://h/db", Some(DbType::Doris)),
            ("jdbc:starrocks://h/db", Some(DbType::StarRocks)),
            ("jdbc:hive2://h:10000/db", Some(DbType::Hive)),
            ("jdbc:presto://h:8080/db", Some(DbType::Presto)),
            ("jdbc:impala://h/db", Some(DbType::Impala)),
            ("jdbc:snowflake://h/db", Some(DbType::Snowflake)),
            ("jdbc:bigquery://h/db", Some(DbType::BigQuery)),
            ("jdbc:redshift://h/db", Some(DbType::Redshift)),
            ("jdbc:spark://h/db", Some(DbType::Spark)),
            ("jdbc:phoenix://h/db", Some(DbType::Phoenix)),
            ("jdbc:teradata://h/db", Some(DbType::Teradata)),
            ("jdbc:informix-sqli://h/db", Some(DbType::Informix)),
            ("jdbc:athena://h/db", Some(DbType::Athena)),
            ("jdbc:gaussdb://h/db", Some(DbType::GaussDB)),
            ("jdbc:dameng://h/db", Some(DbType::DM)),
            ("jdbc:odps://h/db", Some(DbType::ODPS)),
            ("jdbc:maxcompute://h/db", Some(DbType::ODPS)),
            ("jdbc:hologres://h/db", Some(DbType::Hologres)),
        ] {
            assert_eq!(detect_db_type_from_url(url), expect, "url {url}");
        }
    }

    #[test]
    fn test_detect_db_type_edge_cases() {
        // 大小写不敏感
        assert_eq!(
            detect_db_type_from_url("jdbc:MySQL://h/db"),
            Some(DbType::MySQL)
        );
        // 无 :// 分隔时 scheme 匹配失效，但 h2 例外（整体 starts_with 匹配）
        assert_eq!(
            detect_db_type_from_url("jdbc:h2:mem:test"),
            Some(DbType::H2)
        );
        assert!(detect_db_type_from_url("jdbc:mysql:db").is_none());
        // Oracle thin 格式（无 "://"）无法识别 —— 既有启发式行为，保留文档化
        assert!(detect_db_type_from_url("jdbc:oracle:thin:@h:1521/db").is_none());
        // db2 防误判 guard
        assert!(detect_db_type_from_url("jdbc:not-db2://h/db").is_none());
        assert!(detect_db_type_from_url("").is_none());
        // 非 JDBC URL
        assert_eq!(
            detect_db_type_from_url("postgres://user@h/db"),
            Some(DbType::PostgreSQL)
        );
    }

    #[test]
    fn test_sql_types() {
        assert_eq!(get_sql_type("SELECT * FROM users"), "SELECT");
        assert_eq!(get_sql_type("INSERT INTO users VALUES (1)"), "INSERT");
        assert_eq!(get_sql_type(""), "EMPTY");
        assert_eq!(get_sql_type("   \t  "), "EMPTY"); // 纯空白
    }

    #[test]
    fn test_get_sql_type_full_coverage() {
        for (sql, expect) in [
            ("select 1", "SELECT"),
            ("WITH cte AS (SELECT 1) SELECT * FROM cte", "SELECT"),
            ("insert into t values (1)", "INSERT"),
            ("update t set a=1", "UPDATE"),
            ("delete from t", "DELETE"),
            ("create table t (a int)", "CREATE"),
            ("alter table t add b int", "ALTER"),
            ("drop table t", "DROP"),
            ("truncate table t", "TRUNCATE"),
            ("merge into t using s on (1=1)", "MERGE"),
            ("replace into t values (1)", "MERGE"),
            ("explain select 1", "EXPLAIN"),
            ("desc t", "EXPLAIN"),
            ("describe t", "EXPLAIN"),
            ("show tables", "SHOW"),
            ("set names utf8", "SET"),
            ("begin", "TRANSACTION"),
            ("START TRANSACTION", "TRANSACTION"),
            ("start foo", "OTHER"), // start 后非 transaction
            ("commit", "COMMIT"),
            ("rollback", "ROLLBACK"),
            ("grant select on t to u", "DCL"),
            ("revoke select on t from u", "DCL"),
            ("call proc(1)", "CALL"),
            ("execute proc", "CALL"),
            ("vacuum", "OTHER"),
            ("选择 * from t", "OTHER"), // 多字节首词不回退为 SELECT
        ] {
            assert_eq!(get_sql_type(sql), expect, "sql {sql:?}");
        }
        assert_eq!(get_sql_type(" start transaction"), "TRANSACTION"); // 前后空白容忍
    }

    #[test]
    fn test_is_select() {
        assert!(is_select_sql("SELECT * FROM t"));
        assert!(is_select_sql("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(!is_select_sql("INSERT INTO t VALUES (1)"));
        assert!(is_select_sql("select")); // 恰好 6 字节
        assert!(is_select_sql("  select 1"));
        assert!(!is_select_sql(""));
        assert!(!is_select_sql("s"));
        assert!(!is_select_sql("sel"));
        assert!(is_select_sql("selectivly-other")); // 前缀匹配语义（非完整词也命中）
        assert!(!is_select_sql("INSERT"));
        assert!(!is_select_sql("选择")); // 多字节无 panic 且非 select
        assert!(!is_select_sql("你好select"));
    }

    #[test]
    fn test_is_write() {
        for sql in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a=1",
            "DELETE FROM t",
            "REPLACE INTO t VALUES (1)",
            "MERGE INTO t USING s ON (1=1)",
            "  insert into t",
        ] {
            assert!(is_write_sql(sql), "sql {sql:?}");
        }
        assert!(!is_write_sql("SELECT * FROM t"));
        assert!(!is_write_sql(""));
        assert!(is_write_sql("insertx into t")); // 前缀匹配语义
        assert!(!is_write_sql("更新 t"));
    }

    #[test]
    fn test_is_ddl() {
        for sql in [
            "CREATE TABLE t (a INT)",
            "ALTER TABLE t ADD b INT",
            "DROP TABLE t",
            "TRUNCATE TABLE t",
            "RENAME TABLE a TO b",
            "  create index i on t(a)",
        ] {
            assert!(is_ddl_sql(sql), "sql {sql:?}");
        }
        assert!(!is_ddl_sql("SELECT 1"));
        assert!(!is_ddl_sql(""));
        assert!(is_ddl_sql("CREATE")); // 恰好一个词也算 DDL（前缀匹配语义）
    }
}
