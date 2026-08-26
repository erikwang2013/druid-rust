use serde::{Deserialize, Serialize};

/// 数据库类型枚举 — 对应 Druid 支持的 30 种 SQL 方言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DbType {
    MySQL,
    PostgreSQL,
    Oracle,
    #[serde(rename = "sqlserver")]
    SqlServer,
    DB2,
    H2,
    Informix,
    #[serde(rename = "dm")]
    DM,
    Oscar,
    GaussDB,
    ClickHouse,
    Doris,
    StarRocks,
    Teradata,
    Redshift,
    BigQuery,
    Snowflake,
    Synapse,
    Hologres,
    #[serde(rename = "odps")]
    ODPS,
    Hive,
    Spark,
    Presto,
    Impala,
    Athena,
    Blink,
    Databricks,
    Phoenix,
    SuperSQL,
    #[serde(rename = "transact-sql")]
    TransactSql,
    Other,
}

impl DbType {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mysql" => DbType::MySQL,
            "postgresql" | "postgres" | "pgsql" => DbType::PostgreSQL,
            "oracle" => DbType::Oracle,
            "sqlserver" | "mssql" | "sql server" => DbType::SqlServer,
            "db2" => DbType::DB2,
            "h2" => DbType::H2,
            "informix" => DbType::Informix,
            "dm" | "dameng" => DbType::DM,
            "oscar" => DbType::Oscar,
            "gaussdb" | "gauss" => DbType::GaussDB,
            "clickhouse" => DbType::ClickHouse,
            "doris" => DbType::Doris,
            "starrocks" => DbType::StarRocks,
            "teradata" => DbType::Teradata,
            "redshift" => DbType::Redshift,
            "bigquery" => DbType::BigQuery,
            "snowflake" => DbType::Snowflake,
            "synapse" => DbType::Synapse,
            "hologres" => DbType::Hologres,
            "odps" | "maxcompute" => DbType::ODPS,
            "hive" => DbType::Hive,
            "spark" => DbType::Spark,
            "presto" => DbType::Presto,
            "impala" => DbType::Impala,
            "athena" => DbType::Athena,
            "blink" => DbType::Blink,
            "databricks" => DbType::Databricks,
            "phoenix" => DbType::Phoenix,
            "supersql" => DbType::SuperSQL,
            "transact-sql" | "tsql" => DbType::TransactSql,
            _ => DbType::Other,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            DbType::MySQL => "MySQL",
            DbType::PostgreSQL => "PostgreSQL",
            DbType::Oracle => "Oracle",
            DbType::SqlServer => "SQL Server",
            DbType::DB2 => "DB2",
            DbType::H2 => "H2",
            DbType::Informix => "Informix",
            DbType::DM => "达梦",
            DbType::Oscar => "Oscar",
            DbType::GaussDB => "GaussDB",
            DbType::ClickHouse => "ClickHouse",
            DbType::Doris => "Doris",
            DbType::StarRocks => "StarRocks",
            DbType::Teradata => "Teradata",
            DbType::Redshift => "Redshift",
            DbType::BigQuery => "BigQuery",
            DbType::Snowflake => "Snowflake",
            DbType::Synapse => "Synapse",
            DbType::Hologres => "Hologres",
            DbType::ODPS => "ODPS(MaxCompute)",
            DbType::Hive => "Hive",
            DbType::Spark => "Spark",
            DbType::Presto => "Presto",
            DbType::Impala => "Impala",
            DbType::Athena => "Athena",
            DbType::Blink => "Blink",
            DbType::Databricks => "Databricks",
            DbType::Phoenix => "Phoenix",
            DbType::SuperSQL => "SuperSQL",
            DbType::TransactSql => "Transact-SQL",
            DbType::Other => "Other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mysql() {
        assert_eq!(DbType::parse("mysql"), DbType::MySQL);
        assert_eq!(DbType::parse("MySQL"), DbType::MySQL);
    }

    #[test]
    fn test_parse_postgres_aliases() {
        assert_eq!(DbType::parse("postgresql"), DbType::PostgreSQL);
        assert_eq!(DbType::parse("postgres"), DbType::PostgreSQL);
        assert_eq!(DbType::parse("pgsql"), DbType::PostgreSQL);
    }

    #[test]
    fn test_parse_sqlserver_aliases() {
        assert_eq!(DbType::parse("sqlserver"), DbType::SqlServer);
        assert_eq!(DbType::parse("mssql"), DbType::SqlServer);
        assert_eq!(DbType::parse("sql server"), DbType::SqlServer);
    }

    #[test]
    fn test_parse_dm_aliases() {
        assert_eq!(DbType::parse("dm"), DbType::DM);
        assert_eq!(DbType::parse("dameng"), DbType::DM);
    }

    #[test]
    fn test_parse_oracle() {
        assert_eq!(DbType::parse("oracle"), DbType::Oracle);
    }

    #[test]
    fn test_parse_unknown_fallback() {
        assert_eq!(DbType::parse("unknown_db"), DbType::Other);
        assert_eq!(DbType::parse(""), DbType::Other);
        assert_eq!(DbType::parse("  mysql  "), DbType::Other); // 不去空白
    }

    #[test]
    fn test_parse_case_insensitive() {
        assert_eq!(DbType::parse("MYsql"), DbType::MySQL);
        assert_eq!(DbType::parse("PostgreSQL"), DbType::PostgreSQL);
        assert_eq!(DbType::parse("CLICKHOUSE"), DbType::ClickHouse);
    }

    #[test]
    fn test_parse_all_canonical_names() {
        for (s, t) in [
            ("mysql", DbType::MySQL),
            ("postgresql", DbType::PostgreSQL),
            ("oracle", DbType::Oracle),
            ("sqlserver", DbType::SqlServer),
            ("db2", DbType::DB2),
            ("h2", DbType::H2),
            ("informix", DbType::Informix),
            ("dm", DbType::DM),
            ("oscar", DbType::Oscar),
            ("gaussdb", DbType::GaussDB),
            ("clickhouse", DbType::ClickHouse),
            ("doris", DbType::Doris),
            ("starrocks", DbType::StarRocks),
            ("teradata", DbType::Teradata),
            ("redshift", DbType::Redshift),
            ("bigquery", DbType::BigQuery),
            ("snowflake", DbType::Snowflake),
            ("synapse", DbType::Synapse),
            ("hologres", DbType::Hologres),
            ("odps", DbType::ODPS),
            ("hive", DbType::Hive),
            ("spark", DbType::Spark),
            ("presto", DbType::Presto),
            ("impala", DbType::Impala),
            ("athena", DbType::Athena),
            ("blink", DbType::Blink),
            ("databricks", DbType::Databricks),
            ("phoenix", DbType::Phoenix),
            ("supersql", DbType::SuperSQL),
            ("transact-sql", DbType::TransactSql),
        ] {
            assert_eq!(DbType::parse(s), t, "parse {s}");
        }
    }

    #[test]
    fn test_parse_aliases() {
        assert_eq!(DbType::parse("postgres"), DbType::PostgreSQL);
        assert_eq!(DbType::parse("pgsql"), DbType::PostgreSQL);
        assert_eq!(DbType::parse("mssql"), DbType::SqlServer);
        assert_eq!(DbType::parse("sql server"), DbType::SqlServer);
        assert_eq!(DbType::parse("dameng"), DbType::DM);
        assert_eq!(DbType::parse("gauss"), DbType::GaussDB);
        assert_eq!(DbType::parse("maxcompute"), DbType::ODPS);
        assert_eq!(DbType::parse("tsql"), DbType::TransactSql);
    }

    #[test]
    fn test_db_type_names() {
        assert_eq!(DbType::MySQL.name(), "MySQL");
        assert_eq!(DbType::PostgreSQL.name(), "PostgreSQL");
        assert_eq!(DbType::Oracle.name(), "Oracle");
    }

    #[test]
    fn test_all_names_are_non_empty() {
        for t in [
            DbType::MySQL,
            DbType::PostgreSQL,
            DbType::Oracle,
            DbType::SqlServer,
            DbType::DB2,
            DbType::H2,
            DbType::Informix,
            DbType::DM,
            DbType::Oscar,
            DbType::GaussDB,
            DbType::ClickHouse,
            DbType::Doris,
            DbType::StarRocks,
            DbType::Teradata,
            DbType::Redshift,
            DbType::BigQuery,
            DbType::Snowflake,
            DbType::Synapse,
            DbType::Hologres,
            DbType::ODPS,
            DbType::Hive,
            DbType::Spark,
            DbType::Presto,
            DbType::Impala,
            DbType::Athena,
            DbType::Blink,
            DbType::Databricks,
            DbType::Phoenix,
            DbType::SuperSQL,
            DbType::TransactSql,
            DbType::Other,
        ] {
            assert!(!t.name().is_empty(), "name for {:?}", t);
        }
        assert_eq!(DbType::DM.name(), "达梦");
        assert_eq!(DbType::ODPS.name(), "ODPS(MaxCompute)");
        assert_eq!(DbType::SqlServer.name(), "SQL Server");
        assert_eq!(DbType::TransactSql.name(), "Transact-SQL");
        assert_eq!(DbType::Other.name(), "Other");
    }

    #[test]
    fn test_db_type_equality_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(DbType::MySQL);
        set.insert(DbType::MySQL);
        set.insert(DbType::PostgreSQL);
        assert_eq!(set.len(), 2);
        assert!(DbType::MySQL == DbType::parse("mysql"));
        assert!(DbType::MySQL != DbType::PostgreSQL);
    }
}
