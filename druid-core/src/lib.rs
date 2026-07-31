//! Druid-Rust 核心类型和 trait 定义
//!
//! 提供整个 Druid 生态共享的基础类型、错误枚举和配置结构。

pub mod config;
pub mod error;
pub mod types;

pub use config::DruidConfig;
pub use error::DruidError;
pub use types::DbType;
