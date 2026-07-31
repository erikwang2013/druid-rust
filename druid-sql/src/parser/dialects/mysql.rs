//! MySQL 方言解析器
//!
//! 继承核心 Parser，处理 MySQL 特有语法：
//! - LIMIT offset, count
//! - REPLACE INTO
//! - 反引号标识符

use crate::ast::SQLStatement;
use crate::parser::{ParseResult, Parser};

/// MySQL 方言解析器（当前复用核心 Parser，后续扩展方言特有语法）
pub struct MySQLParser {
    inner: Parser,
}

impl MySQLParser {
    pub fn new(sql: &str) -> Self {
        MySQLParser {
            inner: Parser::new(sql),
        }
    }

    pub fn parse_statement(&mut self) -> ParseResult<SQLStatement> {
        self.inner.parse_statement()
    }
}
