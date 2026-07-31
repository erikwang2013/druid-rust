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
