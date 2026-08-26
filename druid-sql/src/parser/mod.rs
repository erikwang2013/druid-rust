pub mod dialects;
pub mod lexer;

use crate::ast::*;
use crate::token::Token;
use lexer::Lexer;

/// SQL 解析结果
pub type ParseResult<T> = Result<T, String>;

/// SQL 解析器 trait — 各方言实现此接口
pub trait SQLParser {
    fn parse_statement(&mut self) -> ParseResult<SQLStatement>;
    fn parse_select(&mut self) -> ParseResult<SelectStatement>;
    fn parse_expr(&mut self) -> ParseResult<SQLExpr>;
}

/// 核心解析器（不特定于方言）
pub struct Parser {
    lexer: Lexer,
    current: Token,
}

impl Parser {
    pub fn new(sql: &str) -> Self {
        let mut lexer = Lexer::new(sql);
        let current = lexer.next_token();
        Parser { lexer, current }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    fn expect(&mut self, expected: Token) -> ParseResult<()> {
        if self.current == expected {
            self.advance();
            Ok(())
        } else {
            Err(format!("expected {:?}, got {:?}", expected, self.current))
        }
    }

    fn skip_comments(&mut self) {
        while matches!(self.current, Token::Comment(_) | Token::BlockComment(_)) {
            self.advance();
        }
    }

    /// 解析单个 SQL 语句
    pub fn parse_statement(&mut self) -> ParseResult<SQLStatement> {
        self.skip_comments();
        match &self.current {
            Token::Select | Token::With => Ok(SQLStatement::Select(Box::new(self.parse_select()?))),
            Token::Insert | Token::Replace => Ok(SQLStatement::Insert(self.parse_insert()?)),
            Token::Update => Ok(SQLStatement::Update(self.parse_update()?)),
            Token::Delete => Ok(SQLStatement::Delete(self.parse_delete()?)),
            Token::Create => Ok(SQLStatement::CreateTable(self.parse_create_table()?)),
            Token::Drop => Ok(SQLStatement::DropObject(self.parse_drop()?)),
            _ => Err(format!("unexpected token: {:?}", self.current)),
        }
    }

    fn parse_select(&mut self) -> ParseResult<SelectStatement> {
        let mut distinct = false;
        let mut with_cte = Vec::new();
        if self.current == Token::With {
            self.advance();
            if self.current == Token::Recursive {
                self.advance();
            }
            loop {
                let cte_name = self.parse_ident()?;
                let mut cte_cols = Vec::new();
                if self.current == Token::LParen {
                    self.advance();
                    loop {
                        cte_cols.push(self.parse_ident()?);
                        if self.current == Token::Comma {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(Token::RParen)?;
                }
                self.expect(Token::As)?;
                self.expect(Token::LParen)?;
                let cte_query = self.parse_select()?;
                self.expect(Token::RParen)?;
                with_cte.push(CteDef {
                    name: cte_name,
                    columns: cte_cols,
                    query: Box::new(cte_query),
                });
                if self.current == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.advance(); // skip SELECT

        if self.current == Token::Distinct {
            distinct = true;
            self.advance();
        }

        let columns = self.parse_select_items()?;
        let from = if self.current == Token::From {
            self.advance();
            Some(self.parse_table_ref()?)
        } else {
            None
        };

        let joins = self.parse_joins()?;

        let where_clause = if self.current == Token::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        let group_by = if self.current == Token::Group {
            self.advance(); // skip GROUP
            self.expect(Token::By)?;
            let mut cols = Vec::new();
            loop {
                cols.push(self.parse_expr()?);
                if self.current == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            cols
        } else {
            Vec::new()
        };

        let having = if self.current == Token::Having {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        let order_by = if self.current == Token::Order {
            self.advance();
            self.expect(Token::By)?;
            let mut items = Vec::new();
            loop {
                let expr = self.parse_expr()?;
                let asc = match &self.current {
                    Token::Asc => {
                        self.advance();
                        true
                    }
                    Token::Desc => {
                        self.advance();
                        false
                    }
                    _ => true,
                };
                items.push(OrderByExpr { expr, asc });
                if self.current == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            items
        } else {
            Vec::new()
        };

        let limit = if self.current == Token::Limit {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        let offset = if self.current == Token::Offset {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(SelectStatement {
            with_cte,
            distinct,
            columns,
            from,
            joins,
            where_clause,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    fn parse_select_items(&mut self) -> ParseResult<Vec<SelectItem>> {
        let mut items = Vec::new();
        loop {
            let item = if self.current == Token::Mul {
                self.advance();
                SelectItem::Wildcard(None)
            } else {
                let expr = self.parse_expr()?;
                let alias = if self.current == Token::As {
                    self.advance();
                    Some(self.parse_ident()?)
                } else if matches!(&self.current, Token::Ident(_) | Token::QuotedIdent(_)) {
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                SelectItem::Expr(expr, alias)
            };
            // 还原 t.* 为带表限定的通配符
            let item = match item {
                SelectItem::Expr(SQLExpr::Identifier(parts), None)
                    if parts.len() == 2 && parts[1] == "*" =>
                {
                    SelectItem::Wildcard(Some(parts[0].clone()))
                }
                other => other,
            };
            items.push(item);
            if self.current == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(items)
    }

    fn parse_table_ref(&mut self) -> ParseResult<TableReference> {
        if self.current == Token::LParen {
            self.advance();
            let sub = self.parse_select()?;
            self.expect(Token::RParen)?;
            let alias = if matches!(&self.current, Token::Ident(_)) {
                self.parse_ident()?
            } else {
                "sub".to_string()
            };
            return Ok(TableReference::SubQuery(
                Box::new(SQLStatement::Select(Box::new(sub))),
                alias,
            ));
        }

        let mut name = self.parse_ident()?;
        if self.current == Token::Dot {
            self.advance();
            let schema = Some(name);
            name = self.parse_ident()?;
            let alias = if matches!(&self.current, Token::Ident(_) | Token::QuotedIdent(_)) {
                Some(self.parse_ident()?)
            } else {
                None
            };
            Ok(TableReference::Table {
                name,
                alias,
                schema,
            })
        } else {
            let alias = if matches!(&self.current, Token::Ident(_) | Token::QuotedIdent(_))
                && self.current != Token::As
            {
                Some(self.parse_ident()?)
            } else if self.current == Token::As {
                self.advance();
                Some(self.parse_ident()?)
            } else {
                None
            };
            Ok(TableReference::Table {
                name,
                alias,
                schema: None,
            })
        }
    }

    fn parse_joins(&mut self) -> ParseResult<Vec<JoinClause>> {
        let mut joins = Vec::new();
        while let Token::Join
        | Token::Inner
        | Token::Left
        | Token::Right
        | Token::Full
        | Token::Cross = &self.current
        {
            let join_type = match &self.current {
                Token::Inner => {
                    self.advance();
                    if self.current == Token::Join {
                        self.advance();
                    }
                    JoinType::Inner
                }
                Token::Left => {
                    self.advance();
                    if self.current == Token::Outer {
                        self.advance();
                    }
                    if self.current == Token::Join {
                        self.advance();
                    }
                    JoinType::Left
                }
                Token::Right => {
                    self.advance();
                    if self.current == Token::Outer {
                        self.advance();
                    }
                    if self.current == Token::Join {
                        self.advance();
                    }
                    JoinType::Right
                }
                Token::Full => {
                    self.advance();
                    if self.current == Token::Outer {
                        self.advance();
                    }
                    if self.current == Token::Join {
                        self.advance();
                    }
                    JoinType::Full
                }
                Token::Cross => {
                    self.advance();
                    if self.current == Token::Join {
                        self.advance();
                    }
                    JoinType::Cross
                }
                Token::Join => {
                    self.advance();
                    JoinType::Inner
                }
                _ => unreachable!(),
            };
            let table = self.parse_table_ref()?;
            self.expect(Token::On)?;
            let on = self.parse_expr()?;
            joins.push(JoinClause {
                join_type,
                table,
                on,
            });
        }
        Ok(joins)
    }

    pub fn parse_expr(&mut self) -> ParseResult<SQLExpr> {
        self.parse_or_expr()
    }

    fn parse_or_expr(&mut self) -> ParseResult<SQLExpr> {
        let mut left = self.parse_and_expr()?;
        while self.current == Token::Or {
            self.advance();
            let right = self.parse_and_expr()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op: BinaryOpType::Or,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> ParseResult<SQLExpr> {
        let mut left = self.parse_comparison()?;
        while self.current == Token::And {
            self.advance();
            let right = self.parse_comparison()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op: BinaryOpType::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> ParseResult<SQLExpr> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match &self.current {
                Token::Eq => BinaryOpType::Eq,
                Token::Neq => BinaryOpType::Neq,
                Token::Lt => BinaryOpType::Lt,
                Token::Gt => BinaryOpType::Gt,
                Token::Leq => BinaryOpType::Leq,
                Token::Geq => BinaryOpType::Geq,
                Token::Like => BinaryOpType::Like,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        // IS NULL / IS NOT NULL
        if self.current == Token::Is {
            self.advance();
            let not = self.current == Token::Not;
            if not {
                self.advance();
            }
            if self.current == Token::Null {
                self.advance();
                left = SQLExpr::IsNull {
                    expr: Box::new(left),
                    not,
                };
            }
        }
        // NOT BETWEEN / NOT IN / NOT LIKE
        if self.current == Token::Not {
            self.advance();
            match &self.current {
                Token::Between => {
                    self.advance();
                    let low = self.parse_additive()?;
                    self.expect(Token::And)?;
                    let high = self.parse_additive()?;
                    left = SQLExpr::Between {
                        expr: Box::new(left),
                        low: Box::new(low),
                        high: Box::new(high),
                        not: true,
                    };
                }
                Token::In => {
                    self.advance();
                    self.expect(Token::LParen)?;
                    left = if self.current == Token::Select {
                        let sub = self.parse_select()?;
                        self.expect(Token::RParen)?;
                        SQLExpr::InSubQuery {
                            expr: Box::new(left),
                            query: Box::new(SQLStatement::Select(Box::new(sub))),
                            not: true,
                        }
                    } else {
                        let mut items = Vec::new();
                        loop {
                            items.push(self.parse_expr()?);
                            if self.current == Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        self.expect(Token::RParen)?;
                        SQLExpr::InList {
                            expr: Box::new(left),
                            list: items,
                            not: true,
                        }
                    };
                }
                Token::Like => {
                    self.advance();
                    let pattern = self.parse_additive()?;
                    left = SQLExpr::Like {
                        expr: Box::new(left),
                        pattern: Box::new(pattern),
                        not: true,
                    };
                }
                Token::Exists => {
                    self.advance();
                    self.expect(Token::LParen)?;
                    let sub = self.parse_select()?;
                    self.expect(Token::RParen)?;
                    left = SQLExpr::Exists(Box::new(SQLStatement::Select(Box::new(sub))), true);
                }
                _ => {
                    tracing::warn!("unrecognized NOT combination: {:?}", self.current);
                }
            }
        }
        // BETWEEN
        if self.current == Token::Between {
            self.advance();
            let low = self.parse_additive()?;
            self.expect(Token::And)?;
            let high = self.parse_additive()?;
            left = SQLExpr::Between {
                expr: Box::new(left),
                low: Box::new(low),
                high: Box::new(high),
                not: false,
            };
        }
        // IN
        if self.current == Token::In {
            self.advance();
            self.expect(Token::LParen)?;
            left = if self.current == Token::Select {
                let sub = self.parse_select()?;
                self.expect(Token::RParen)?;
                SQLExpr::InSubQuery {
                    expr: Box::new(left),
                    query: Box::new(SQLStatement::Select(Box::new(sub))),
                    not: false,
                }
            } else {
                let mut items = Vec::new();
                loop {
                    items.push(self.parse_expr()?);
                    if self.current == Token::Comma {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(Token::RParen)?;
                SQLExpr::InList {
                    expr: Box::new(left),
                    list: items,
                    not: false,
                }
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> ParseResult<SQLExpr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match &self.current {
                Token::Plus => BinaryOpType::Plus,
                Token::Minus => BinaryOpType::Minus,
                Token::Concat => BinaryOpType::Concat,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> ParseResult<SQLExpr> {
        let mut left = self.parse_primary()?;
        loop {
            let op = match &self.current {
                Token::Mul => BinaryOpType::Mul,
                Token::Div => BinaryOpType::Div,
                Token::Mod => BinaryOpType::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_primary()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> ParseResult<SQLExpr> {
        match &self.current {
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();
                // 可能是 table.column 或函数调用
                if self.current == Token::Dot {
                    self.advance();
                    if self.current == Token::Mul {
                        // t.* — parse_select_items 会还原为 SelectItem::Wildcard(Some)
                        self.advance();
                        Ok(SQLExpr::Identifier(vec![name, "*".to_string()]))
                    } else {
                        let col = self.parse_ident()?;
                        Ok(SQLExpr::Identifier(vec![name, col]))
                    }
                } else if self.current == Token::LParen {
                    // 函数调用
                    self.advance();
                    let mut distinct = false;
                    let mut args = Vec::new();
                    if self.current == Token::Distinct {
                        distinct = true;
                        self.advance();
                    }
                    if self.current != Token::RParen {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.current == Token::Comma {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Token::RParen)?;
                    Ok(SQLExpr::Function {
                        name,
                        args,
                        distinct,
                    })
                } else {
                    Ok(SQLExpr::Identifier(vec![name]))
                }
            }
            Token::QuotedIdent(name) => {
                let name = name.clone();
                self.advance();
                if self.current == Token::Dot {
                    self.advance();
                    let col = self.parse_ident()?;
                    Ok(SQLExpr::Identifier(vec![name, col]))
                } else {
                    Ok(SQLExpr::Identifier(vec![name]))
                }
            }
            Token::Number(n) => {
                let n = n.clone();
                self.advance();
                Ok(SQLExpr::NumberLiteral(n))
            }
            Token::StringLit(s) => {
                let s = s.clone();
                self.advance();
                Ok(SQLExpr::StringLiteral(s))
            }
            Token::Null => {
                self.advance();
                Ok(SQLExpr::Null)
            }
            Token::Placeholder => {
                self.advance();
                Ok(SQLExpr::Placeholder)
            }
            Token::Mul => {
                self.advance();
                Ok(SQLExpr::Wildcard)
            }
            Token::LParen => {
                self.advance();
                // 检查是否是子查询
                if self.current == Token::Select {
                    let sub = self.parse_select()?;
                    self.expect(Token::RParen)?;
                    Ok(SQLExpr::SubQuery(Box::new(SQLStatement::Select(Box::new(
                        sub,
                    )))))
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(Token::RParen)?;
                    Ok(SQLExpr::Nested(Box::new(expr)))
                }
            }
            Token::Not => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(SQLExpr::UnaryOp {
                    op: UnaryOpType::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(SQLExpr::UnaryOp {
                    op: UnaryOpType::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::Case => self.parse_case(),
            Token::Exists => {
                self.advance();
                self.expect(Token::LParen)?;
                let sub = self.parse_select()?;
                self.expect(Token::RParen)?;
                Ok(SQLExpr::Exists(
                    Box::new(SQLStatement::Select(Box::new(sub))),
                    false,
                ))
            }
            Token::Count => {
                self.advance();
                self.expect(Token::LParen)?;
                let expr = if self.current == Token::Mul {
                    self.advance();
                    SQLExpr::Wildcard
                } else {
                    self.parse_expr()?
                };
                self.expect(Token::RParen)?;
                Ok(SQLExpr::Aggregate {
                    name: "COUNT".to_string(),
                    expr: Box::new(expr),
                })
            }
            _ => Err(format!(
                "unexpected token in expression: {:?}",
                self.current
            )),
        }
    }

    fn parse_case(&mut self) -> ParseResult<SQLExpr> {
        self.advance(); // skip CASE
        let expr = if self.current != Token::When {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let mut whens = Vec::new();
        while self.current == Token::When {
            self.advance();
            let condition = self.parse_expr()?;
            self.expect(Token::Then)?;
            let result = self.parse_expr()?;
            whens.push((condition, result));
        }
        let else_expr = if self.current == Token::Else {
            self.advance();
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(Token::End)?;
        Ok(SQLExpr::Case {
            expr,
            whens,
            else_expr,
        })
    }

    fn parse_insert(&mut self) -> ParseResult<InsertStatement> {
        let is_replace = self.current == Token::Replace;
        self.advance(); // skip INSERT/REPLACE
        self.expect(Token::Into)?;
        let table = self.parse_ident()?;

        let columns = if self.current == Token::LParen {
            self.advance();
            let mut cols = Vec::new();
            loop {
                cols.push(self.parse_ident()?);
                if self.current == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            cols
        } else {
            Vec::new()
        };

        self.expect(Token::Values)?;
        let mut values = Vec::new();
        loop {
            self.expect(Token::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_expr()?);
                if self.current == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            values.push(row);
            if self.current == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(InsertStatement {
            table,
            columns,
            values,
            is_replace,
        })
    }

    fn parse_update(&mut self) -> ParseResult<UpdateStatement> {
        self.advance(); // skip UPDATE
        let table = self.parse_ident()?;
        self.expect(Token::Set)?;
        let mut sets = Vec::new();
        loop {
            let col = self.parse_ident()?;
            self.expect(Token::Eq)?;
            let val = self.parse_expr()?;
            sets.push((col, val));
            if self.current == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        let where_clause = if self.current == Token::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(UpdateStatement {
            table,
            sets,
            where_clause,
        })
    }

    fn parse_delete(&mut self) -> ParseResult<DeleteStatement> {
        self.advance(); // skip DELETE
        self.expect(Token::From)?;
        let table = self.parse_ident()?;
        let where_clause = if self.current == Token::Where {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(DeleteStatement {
            table,
            where_clause,
        })
    }

    fn parse_create_table(&mut self) -> ParseResult<CreateTableStatement> {
        self.advance(); // skip CREATE
        self.expect(Token::Table)?;
        let if_not_exists = if self.current == Token::If {
            self.advance();
            self.expect(Token::Not)?;
            self.expect(Token::Exists)?;
            true
        } else {
            false
        };
        let table = self.parse_ident()?;
        self.expect(Token::LParen)?;
        let mut columns = Vec::new();
        loop {
            if self.current == Token::Primary
                || self.current == Token::Constraint
                || self.current == Token::Foreign
                || self.current == Token::Unique
                || self.current == Token::Check
                || self.current == Token::Index
            {
                while self.current != Token::Comma
                    && self.current != Token::RParen
                    && self.current != Token::Eof
                {
                    self.advance();
                }
                if self.current == Token::Comma {
                    self.advance();
                }
                continue;
            }
            let name = self.parse_ident()?;
            let data_type = self.parse_data_type()?;
            let mut nullable = true;
            let mut default_value = None;
            let mut is_primary_key = false;
            loop {
                match &self.current {
                    Token::Not => {
                        self.advance();
                        if self.current == Token::Null {
                            self.advance();
                            nullable = false;
                        }
                    }
                    Token::Null => {
                        self.advance();
                        nullable = true;
                    }
                    Token::Primary => {
                        self.advance();
                        if self.current == Token::Key {
                            self.advance();
                        }
                        is_primary_key = true;
                    }
                    Token::Default => {
                        self.advance();
                        default_value = Some(self.parse_expr()?);
                    }
                    _ => break,
                }
            }
            columns.push(ColumnDef {
                name,
                data_type,
                nullable,
                default_value,
                is_primary_key,
            });
            if self.current == Token::Comma {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;
        Ok(CreateTableStatement {
            if_not_exists,
            table,
            columns,
        })
    }

    fn parse_data_type(&mut self) -> ParseResult<String> {
        let mut dt = String::new();
        match &self.current {
            Token::Ident(s) | Token::QuotedIdent(s) => {
                dt.push_str(s);
                self.advance();
            }
            Token::Int
            | Token::BigInt
            | Token::SmallInt
            | Token::TinyInt
            | Token::VarChar
            | Token::Char
            | Token::Text
            | Token::Boolean
            | Token::Float
            | Token::Double
            | Token::Decimal
            | Token::Real
            | Token::Date
            | Token::Time
            | Token::Timestamp
            | Token::Blob
            | Token::Clob
            | Token::Json
            | Token::Jsonb
            | Token::Xml
            | Token::Uuid
            | Token::Bytea => {
                dt.push_str(self.current.as_type_name());
                self.advance();
            }
            _ => {
                dt.push_str(&format!("{:?}", self.current));
                self.advance();
            }
        }
        if self.current == Token::LParen {
            self.advance();
            dt.push('(');
            while let Token::Number(n) | Token::Ident(n) = &self.current {
                dt.push_str(n);
                self.advance();
                if self.current == Token::Comma {
                    dt.push_str(", ");
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            dt.push(')');
        }
        Ok(dt)
    }

    fn parse_drop(&mut self) -> ParseResult<DropStatement> {
        self.advance(); // skip DROP
        let obj_type = match &self.current {
            Token::Table => {
                self.advance();
                DropObjectType::Table
            }
            Token::View => {
                self.advance();
                DropObjectType::View
            }
            Token::Index => {
                self.advance();
                DropObjectType::Index
            }
            _ => {
                self.advance();
                DropObjectType::Table
            }
        };
        let if_exists = if self.current == Token::If {
            self.advance();
            self.expect(Token::Exists)?;
            true
        } else {
            false
        };
        let name = self.parse_ident()?;
        Ok(DropStatement {
            object_type: obj_type,
            if_exists,
            name,
        })
    }

    fn parse_ident(&mut self) -> ParseResult<String> {
        match &self.current {
            Token::Ident(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            Token::QuotedIdent(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(format!("expected identifier, got {:?}", self.current)),
        }
    }
}

impl SQLParser for Parser {
    fn parse_statement(&mut self) -> ParseResult<SQLStatement> {
        Parser::parse_statement(self)
    }
    fn parse_select(&mut self) -> ParseResult<SelectStatement> {
        Parser::parse_select(self)
    }
    fn parse_expr(&mut self) -> ParseResult<SQLExpr> {
        Parser::parse_expr(self)
    }
}

/// 解析 SQL 文本为语句列表（最多 10,000 条语句）
pub fn parse_sql(sql: &str) -> ParseResult<Vec<SQLStatement>> {
    const MAX_ITERATIONS: usize = 10_000;
    let mut parser = Parser::new(sql);
    let mut stmts = Vec::new();
    for i in 0..MAX_ITERATIONS {
        parser.skip_comments();
        if parser.current == Token::Eof || parser.current == Token::Semicolon {
            if parser.current == Token::Semicolon {
                parser.advance();
            }
            if parser.current == Token::Eof {
                break;
            }
            if i == MAX_ITERATIONS - 1 {
                tracing::warn!(
                    "parse_sql reached MAX_ITERATIONS ({}), remaining input truncated",
                    MAX_ITERATIONS
                );
            }
            continue;
        }
        stmts.push(parser.parse_statement()?);
        if parser.current == Token::Semicolon {
            parser.advance();
        }
        if parser.current == Token::Eof {
            break;
        }
        if i == MAX_ITERATIONS - 1 {
            tracing::warn!(
                "parse_sql reached MAX_ITERATIONS ({}), remaining input truncated",
                MAX_ITERATIONS
            );
        }
    }
    Ok(stmts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(sql: &str) -> SQLStatement {
        let stmts = parse_sql(sql).unwrap();
        assert_eq!(stmts.len(), 1, "expected 1 statement for: {sql}");
        stmts.into_iter().next().unwrap()
    }

    #[test]
    fn test_empty_and_blank_sql() {
        assert_eq!(parse_sql("").unwrap(), vec![]);
        assert_eq!(parse_sql("  \t\n").unwrap(), vec![]);
        assert_eq!(parse_sql(";").unwrap(), vec![]);
        assert_eq!(parse_sql("; ; -- x\n").unwrap(), vec![]);
        assert_eq!(parse_sql("-- only comment").unwrap(), vec![]);
    }

    #[test]
    fn test_select_basic_structure() {
        let stmt = first("SELECT id, name AS n FROM users WHERE age > 18");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert!(!s.distinct);
        assert_eq!(s.columns.len(), 2);
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["id".into()]), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(
                SQLExpr::Identifier(vec!["name".into()]),
                Some("n".into())
            )
        );
        let TableReference::Table { name, schema, .. } = s.from.as_ref().unwrap() else {
            panic!("not table")
        };
        assert_eq!(name, "users");
        assert!(schema.is_none());
        let Some(SQLExpr::BinaryOp { op, .. }) = &s.where_clause else {
            panic!("no where")
        };
        assert_eq!(*op, BinaryOpType::Gt);
    }

    #[test]
    fn test_select_all_clauses() {
        let stmt = first(
            "SELECT DISTINCT t.a, COUNT(*) FROM t \
             WHERE x > 1 GROUP BY t.a HAVING COUNT(*) > 1 \
             ORDER BY t.a DESC, b LIMIT 10 OFFSET 5",
        );
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert!(s.distinct);
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(
                SQLExpr::Aggregate {
                    name: "COUNT".into(),
                    expr: Box::new(SQLExpr::Wildcard),
                },
                None
            )
        );
        assert_eq!(s.group_by.len(), 1);
        assert!(s.having.is_some());
        assert_eq!(s.order_by.len(), 2);
        assert!(!s.order_by[0].asc);
        assert!(s.order_by[1].asc);
        assert_eq!(s.limit, Some(SQLExpr::NumberLiteral("10".into())));
        assert_eq!(s.offset, Some(SQLExpr::NumberLiteral("5".into())));
    }

    #[test]
    fn test_select_joins() {
        let stmt = first(
            "SELECT * FROM a LEFT JOIN b ON a.id = b.id \
             INNER JOIN c ON c.aid = a.id RIGHT OUTER JOIN d ON d.x = c.x \
             CROSS JOIN e ON e.x = a.x JOIN f ON f.a = a.id",
        );
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert_eq!(s.columns[0], SelectItem::Wildcard(None));
        assert_eq!(s.joins.len(), 5);
        assert_eq!(s.joins[0].join_type, JoinType::Left);
        assert_eq!(s.joins[1].join_type, JoinType::Inner);
        assert_eq!(s.joins[2].join_type, JoinType::Right);
        assert_eq!(s.joins[3].join_type, JoinType::Cross);
        assert_eq!(s.joins[4].join_type, JoinType::Inner);
        // 所有 join（含 CROSS）都要求 ON 子句
        let TableReference::Table { name, .. } = &s.joins[4].table else {
            panic!("not table")
        };
        assert_eq!(name, "f");
    }

    #[test]
    fn test_select_table_alias_and_schema() {
        let stmt = first("SELECT u.id FROM db.users u WHERE u.id = 1");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        let TableReference::Table { name, alias, schema } = s.from.as_ref().unwrap() else {
            panic!("not table")
        };
        assert_eq!(name, "users");
        assert_eq!(alias.as_deref(), Some("u"));
        assert_eq!(schema.as_deref(), Some("db"));
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["u".into(), "id".into()]), None)
        );
    }

    #[test]
    fn test_insert_and_replace() {
        let SQLStatement::Insert(ins) =
            first("INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)")
        else {
            panic!("not insert")
        };
        assert!(!ins.is_replace);
        assert_eq!(ins.table, "t");
        assert_eq!(ins.columns, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(ins.values.len(), 2);
        assert_eq!(ins.values[1][1], SQLExpr::Null);

        let SQLStatement::Insert(rep) = first("REPLACE INTO t VALUES (1)") else {
            panic!("not replace")
        };
        assert!(rep.is_replace);
        assert!(rep.columns.is_empty());
        assert_eq!(rep.values.len(), 1);
    }

    #[test]
    fn test_update_delete() {
        let SQLStatement::Update(up) =
            first("UPDATE t SET a = 1, b = a + 1 WHERE id = 3")
        else {
            panic!("not update")
        };
        assert_eq!(up.table, "t");
        assert_eq!(up.sets.len(), 2);
        assert_eq!(up.sets[0].0, "a");
        assert_eq!(
            up.sets[1].1,
            SQLExpr::BinaryOp {
                left: Box::new(SQLExpr::Identifier(vec!["a".into()])),
                op: BinaryOpType::Plus,
                right: Box::new(SQLExpr::NumberLiteral("1".into())),
            }
        );
        assert!(up.where_clause.is_some());

        let SQLStatement::Delete(del) = first("DELETE FROM t WHERE x = 1") else {
            panic!("not delete")
        };
        assert_eq!(del.table, "t");
        assert!(del.where_clause.is_some());

        let SQLStatement::Delete(del2) = first("DELETE FROM t") else {
            panic!("not delete")
        };
        assert!(del2.where_clause.is_none());
    }

    #[test]
    fn test_create_table() {
        let SQLStatement::CreateTable(ct) = first(
            "CREATE TABLE IF NOT EXISTS t (\
             id INT PRIMARY KEY, \
             name VARCHAR(20) NOT NULL DEFAULT 'x', \
             age INT NULL, \
             price DECIMAL(10, 2))",
        ) else {
            panic!("not create")
        };
        assert!(ct.if_not_exists);
        assert_eq!(ct.table, "t");
        assert_eq!(ct.columns.len(), 4);
        assert_eq!(ct.columns[0].name, "id");
        assert_eq!(ct.columns[0].data_type, "INT");
        assert!(ct.columns[0].is_primary_key);
        assert_eq!(ct.columns[1].data_type, "VARCHAR(20)");
        assert!(!ct.columns[1].nullable);
        assert_eq!(
            ct.columns[1].default_value,
            Some(SQLExpr::StringLiteral("x".into()))
        );
        assert!(ct.columns[2].nullable);
        assert_eq!(ct.columns[3].data_type, "DECIMAL(10, 2)");
    }

    #[test]
    fn test_drop_variants() {
        let stmts = parse_sql("DROP TABLE IF EXISTS t; DROP VIEW v; DROP INDEX i").unwrap();
        assert_eq!(stmts.len(), 3);
        let SQLStatement::DropObject(d1) = &stmts[0] else { panic!() };
        assert_eq!(d1.object_type, DropObjectType::Table);
        assert!(d1.if_exists);
        assert_eq!(d1.name, "t");
        let SQLStatement::DropObject(d2) = &stmts[1] else { panic!() };
        assert_eq!(d2.object_type, DropObjectType::View);
        let SQLStatement::DropObject(d3) = &stmts[2] else { panic!() };
        assert_eq!(d3.object_type, DropObjectType::Index);
        assert!(!d3.if_exists);
    }

    #[test]
    fn test_with_cte() {
        let stmt = first("WITH x AS (SELECT 1), y (a, b) AS (SELECT 1, 2) SELECT * FROM x");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert_eq!(s.with_cte.len(), 2);
        assert_eq!(s.with_cte[0].name, "x");
        assert!(s.with_cte[0].columns.is_empty());
        assert_eq!(s.with_cte[1].name, "y");
        assert_eq!(s.with_cte[1].columns, vec!["a".to_string(), "b".to_string()]);
        let TableReference::Table { name, .. } = s.from.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(name, "x");
    }

    #[test]
    fn test_case_expression() {
        let stmt = first(
            "SELECT CASE WHEN a > 1 THEN 'big' WHEN a < 0 THEN 'neg' ELSE 'small' END \
             AS size FROM t",
        );
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        let SelectItem::Expr(SQLExpr::Case { expr, whens, else_expr }, alias) = &s.columns[0]
        else {
            panic!("not case")
        };
        assert!(expr.is_none()); // 无 base expr 的 searched CASE
        assert_eq!(whens.len(), 2);
        assert!(else_expr.is_some());
        assert_eq!(alias.as_deref(), Some("size"));
    }

    #[test]
    fn test_between_in_exists_and_not() {
        let stmt = first(
            "SELECT * FROM t \
             WHERE a BETWEEN 1 AND 2 \
             AND b NOT IN (1, 2, 3) \
             AND c NOT LIKE 'x%' \
             AND d IS NOT NULL \
             AND EXISTS (SELECT 1 FROM u WHERE u.id = t.id)",
        );
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        // 顶层是 AND 链，逐层解包
        let mut found_between = false;
        let mut found_in = false;
        let mut found_not_like = false;
        let mut found_is_null = false;
        let mut found_exists = false;
        fn walk(e: &SQLExpr, f: &mut impl FnMut(&SQLExpr)) {
            f(e);
            if let SQLExpr::BinaryOp { left, right, .. } = e {
                walk(left, f);
                walk(right, f);
            }
        }
        let Some(wc) = &s.where_clause else { panic!("no where") };
        walk(wc, &mut |e| {
            match e {
                SQLExpr::Between { not, .. } => found_between = !*not,
                SQLExpr::InList { not, .. } => found_in = *not,
                SQLExpr::Like { not, .. } => found_not_like = *not,
                SQLExpr::IsNull { not, .. } => found_is_null = *not,
                SQLExpr::Exists(..) => found_exists = true,
                _ => {}
            }
        });
        assert!(found_between && found_in && found_not_like && found_is_null && found_exists);
    }

    #[test]
    fn test_subquery_and_nested() {
        let stmt = first("SELECT (a + b) * c FROM t WHERE id IN (SELECT uid FROM u)");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        let SelectItem::Expr(e, _) = &s.columns[0] else { panic!() };
        let SQLExpr::BinaryOp { op, .. } = e else { panic!("not binary") };
        assert_eq!(*op, BinaryOpType::Mul);
        let Some(SQLExpr::InSubQuery { expr, query, not }) = &s.where_clause else {
            panic!("not in-subquery")
        };
        assert!(!not);
        assert_eq!(
            **expr,
            SQLExpr::Identifier(vec!["id".into()])
        );
        let SQLStatement::Select(q) = query.as_ref() else { panic!() };
        assert_eq!(q.columns.len(), 1);
    }

    #[test]
    fn test_subquery_in_from() {
        let stmt = first("SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
        let SQLStatement::Select(outer) = stmt else { panic!("not select") };
        let TableReference::SubQuery(inner, alias) = outer.from.as_ref().unwrap() else {
            panic!("not subquery")
        };
        assert_eq!(alias, "s");
        let SQLStatement::Select(q) = inner.as_ref() else { panic!() };
        assert_eq!(q.columns.len(), 1);
        assert_eq!(
            q.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["a".into()]), None)
        );
    }

    #[test]
    fn test_function_and_unary() {
        let stmt = first("SELECT COALESCE(a, 0), -x, NOT y FROM t WHERE NOT EXISTS (SELECT 1)");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(
                SQLExpr::Function {
                    name: "COALESCE".into(),
                    args: vec![
                        SQLExpr::Identifier(vec!["a".into()]),
                        SQLExpr::NumberLiteral("0".into()),
                    ],
                    distinct: false,
                },
                None
            )
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(
                SQLExpr::UnaryOp {
                    op: UnaryOpType::Neg,
                    expr: Box::new(SQLExpr::Identifier(vec!["x".into()])),
                },
                None
            )
        );
    }

    #[test]
    fn test_quoted_identifiers_in_expr() {
        let stmt = first(r#"SELECT "col", t."x", 1 AS "one" FROM "t" "al""#);
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["col".into()]), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(SQLExpr::Identifier(vec!["t".into(), "x".into()]), None)
        );
        assert_eq!(s.columns[2], SelectItem::Expr(SQLExpr::NumberLiteral("1".into()), Some("one".into())));
        let TableReference::Table { name, alias, .. } = s.from.as_ref().unwrap() else {
            panic!("not table")
        };
        assert_eq!(name, "t");
        assert_eq!(alias.as_deref(), Some("al"));
    }

    #[test]
    fn test_string_literal_escapes_in_sql() {
        let stmt = first(r#"SELECT 'it''s', 'a\'b'"#);
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::StringLiteral("it's".into()), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(SQLExpr::StringLiteral("a'b".into()), None)
        );
    }

    #[test]
    fn test_multi_statement_with_comments() {
        let stmts = parse_sql("SELECT 1; -- comment\nUPDATE t SET a = 2; DELETE FROM t").unwrap();
        assert_eq!(stmts.len(), 3);
        assert!(matches!(stmts[0], SQLStatement::Select(_)));
        assert!(matches!(stmts[1], SQLStatement::Update(_)));
        assert!(matches!(stmts[2], SQLStatement::Delete(_)));
    }

    #[test]
    fn test_placeholder() {
        let stmt = first("SELECT * FROM t WHERE a = ? AND b IN (?, ?)");
        let SQLStatement::Select(s) = stmt else { panic!("not select") };
        let Some(SQLExpr::BinaryOp { .. }) = &s.where_clause else { panic!() };
        let sql = crate::format::format_statement(&SQLStatement::Select(s));
        assert!(sql.contains("?"));
    }

    #[test]
    fn test_parse_errors() {
        assert!(parse_sql("").is_ok());
        // 空 SELECT 无列
        assert!(parse_sql("SELECT").is_err());
        // 缺表名
        assert!(parse_sql("SELECT * FROM").is_err());
        // 缺 INTO
        assert!(parse_sql("INSERT t VALUES (1)").is_err());
        // 缺 SET
        assert!(parse_sql("UPDATE t a = 1").is_err());
        // 未知关键字
        assert!(parse_sql("FOO BAR").is_err());
        // 未闭合括号
        assert!(parse_sql("SELECT (1").is_err());
        // 括号后多余 token
        assert!(parse_sql("SELECT 1)").is_err());
        // CTE 缺 AS
        assert!(parse_sql("WITH x (SELECT 1) SELECT * FROM x").is_err());
    }
}
