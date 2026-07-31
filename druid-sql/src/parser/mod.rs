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
                } else if matches!(&self.current, Token::Ident(_)) {
                    Some(self.parse_ident()?)
                } else {
                    None
                };
                SelectItem::Expr(expr, alias)
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
            let alias = if matches!(&self.current, Token::Ident(_)) {
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
            let alias = if matches!(&self.current, Token::Ident(_)) && self.current != Token::As {
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
                    left = SQLExpr::InList {
                        expr: Box::new(left),
                        list: items,
                        not: true,
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
            left = SQLExpr::InList {
                expr: Box::new(left),
                list: items,
                not: false,
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
                    let col = self.parse_ident()?;
                    Ok(SQLExpr::Identifier(vec![name, col]))
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
            _ => DropObjectType::Table,
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
