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

/// 递归深度上限 —— 递归下降每层嵌套消耗多个栈帧，
/// 不设上限时超深嵌套（如几万个括号）会栈溢出 SIGSEGV，进程不可恢复
const MAX_DEPTH: usize = 128;

/// 核心解析器（不特定于方言）
pub struct Parser {
    lexer: Lexer,
    current: Token,
    /// 当前递归深度，由 parse_select/parse_expr/parse_primary 等递归入口维护
    depth: usize,
}

impl Parser {
    pub fn new(sql: &str) -> Self {
        let mut lexer = Lexer::new(sql);
        let current = lexer.next_token();
        Parser {
            lexer,
            current,
            depth: 0,
        }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    /// 进入一层递归：超过上限立即返回 Err，返回前调用方负责 depth -= 1
    fn enter_depth(&mut self) -> ParseResult<()> {
        if self.depth >= MAX_DEPTH {
            return Err(format!("SQL nesting too deep (> {MAX_DEPTH})"));
        }
        self.depth += 1;
        Ok(())
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
        self.check_lex_error()?;
        // EXPLAIN [ANALYZE] 前缀剥离，内层语句按原类型返回：
        // MySQL 8.0.32+ 的 EXPLAIN ANALYZE 会真正执行语句（EXPLAIN ANALYZE DELETE 会删数据），
        // 因此必须让内层语句暴露成它本来的类型（DELETE 就按 DELETE 判定），而不是包一层。
        if self.current == Token::Explain {
            self.advance();
            if self.current == Token::Analyze {
                self.advance();
            }
            self.skip_comments();
            return self.parse_statement();
        }
        let stmt = match &self.current {
            Token::Select | Token::With => SQLStatement::Select(Box::new(self.parse_select()?)),
            Token::Insert | Token::Replace => SQLStatement::Insert(self.parse_insert()?),
            Token::Update => SQLStatement::Update(self.parse_update()?),
            Token::Delete => SQLStatement::Delete(self.parse_delete()?),
            Token::Create => SQLStatement::CreateTable(self.parse_create_table()?),
            Token::Drop => SQLStatement::DropObject(self.parse_drop()?),
            _ => return Err(format!("unexpected token: {:?}", self.current)),
        };
        self.check_lex_error()?;
        Ok(stmt)
    }

    /// 词法失败（当前只有可执行注释超深一种）会让 token 流提前截断：
    /// 语句本身可能"看起来解析成功"，但被丢弃的内容上层看不见，必须报错（fail-closed）
    fn check_lex_error(&mut self) -> ParseResult<()> {
        match self.lexer.take_error() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn parse_select(&mut self) -> ParseResult<SelectStatement> {
        self.enter_depth()?;
        let result = self.parse_select_inner();
        self.depth -= 1;
        result
    }

    fn parse_select_inner(&mut self) -> ParseResult<SelectStatement> {
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

        let mut limit = if self.current == Token::Limit {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };

        // MySQL 的 LIMIT offset, count 写法：逗号前是偏移量，逗号后才是行数
        let mut offset = if self.current == Token::Comma {
            self.advance();
            let count = self.parse_expr()?;
            let skip = limit.take();
            limit = Some(count);
            skip
        } else {
            None
        };

        if self.current == Token::Offset {
            self.advance();
            if offset.is_some() {
                return Err("LIMIT offset, count cannot be combined with OFFSET".to_string());
            }
            offset = Some(self.parse_expr()?);
        }

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
        self.enter_depth()?;
        let result = self.parse_or_expr();
        self.depth -= 1;
        result
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
        let mut left = self.parse_not_expr()?;
        while self.current == Token::And {
            self.advance();
            let right = self.parse_not_expr()?;
            left = SQLExpr::BinaryOp {
                left: Box::new(left),
                op: BinaryOpType::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// NOT 前缀：优先级低于比较运算符（SQL 标准 / MySQL / PG），
    /// 因此 NOT a = 1 是 NOT (a = 1)，NOT x LIKE 'a%' 是 NOT (x LIKE 'a%')。
    /// 若把 NOT 放在 parse_primary 里，NOT 只会吃掉一个 primary，
    /// 后面的 =/IN/LIKE/BETWEEN 会拼在 NOT 外面（NOT name LIKE 'a%' 变成 (NOT name) LIKE 'a%'，
    /// 恒 false 导致静默少返回数据）。
    fn parse_not_expr(&mut self) -> ParseResult<SQLExpr> {
        if self.current == Token::Not {
            self.enter_depth()?;
            self.advance();
            let inner = self.parse_not_expr();
            self.depth -= 1;
            return Ok(SQLExpr::UnaryOp {
                op: UnaryOpType::Not,
                expr: Box::new(inner?),
            });
        }
        self.parse_comparison()
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
        self.enter_depth()?;
        let result = self.parse_primary_inner();
        self.depth -= 1;
        result
    }

    fn parse_primary_inner(&mut self) -> ParseResult<SQLExpr> {
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
                    if self.current == Token::Mul {
                        // `t`.* — 与 Ident 分支对称，parse_select_items 还原为 Wildcard
                        self.advance();
                        Ok(SQLExpr::Identifier(vec![name, "*".to_string()]))
                    } else {
                        let col = self.parse_ident()?;
                        Ok(SQLExpr::Identifier(vec![name, col]))
                    }
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
            Token::Variable(name) => {
                let name = name.clone();
                self.advance();
                Ok(SQLExpr::Variable(name))
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
            // NOT 不在这里处理 —— 见 parse_not_expr，其优先级低于比较运算符
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
            Token::Ident(s) => {
                dt.push_str(s);
                self.advance();
            }
            Token::QuotedIdent(s) => {
                // 引号内的类型名要带着引号回写：原样输出 "a b" 会变成两个标识符，
                // 格式化结果无法重解析
                if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    dt.push_str(s);
                } else {
                    dt.push('`');
                    dt.push_str(&s.replace('`', "``"));
                    dt.push('`');
                }
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
                // 写 Token 的 SQL 文本（Display）而不是 Debug：Debug 不是 SQL，
                // 格式化后无法重解析（CREATE TABLE t (id 'x') → id StringLit("x")）
                dt.push_str(&self.current.to_string());
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
///
/// 迭代次数耗尽时返回 Err —— 不能返回 Ok 让调用方以为已解析完：
/// 例如 ";".repeat(10001) + "DROP TABLE users" 会先耗光迭代次数，
/// 静默返回空列表（防火墙据此放行未解析的语句）。
pub fn parse_sql(sql: &str) -> ParseResult<Vec<SQLStatement>> {
    const MAX_ITERATIONS: usize = 10_000;
    let mut parser = Parser::new(sql);
    let mut stmts = Vec::new();
    for _ in 0..MAX_ITERATIONS {
        parser.skip_comments();
        if parser.current == Token::Eof {
            // 词法失败会让 token 流截断成 Eof（此时语句列表可能为空）：
            // 必须报错，否则被丢弃的内容（如超深 /*! 里的 INTO OUTFILE）等于没检查
            parser.check_lex_error()?;
            return Ok(stmts);
        }
        if parser.current == Token::Semicolon {
            parser.advance();
            continue;
        }
        stmts.push(parser.parse_statement()?);
        if parser.current == Token::Semicolon {
            parser.advance();
        }
    }
    parser.skip_comments();
    if parser.current == Token::Eof {
        parser.check_lex_error()?;
        return Ok(stmts);
    }
    Err(format!("too many statements (> {MAX_ITERATIONS})"))
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        assert!(!s.distinct);
        assert_eq!(s.columns.len(), 2);
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["id".into()]), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(SQLExpr::Identifier(vec!["name".into()]), Some("n".into()))
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let TableReference::Table {
            name,
            alias,
            schema,
        } = s.from.as_ref().unwrap()
        else {
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
        let SQLStatement::Insert(ins) = first("INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)")
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
        let SQLStatement::Update(up) = first("UPDATE t SET a = 1, b = a + 1 WHERE id = 3") else {
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
        let SQLStatement::DropObject(d1) = &stmts[0] else {
            panic!()
        };
        assert_eq!(d1.object_type, DropObjectType::Table);
        assert!(d1.if_exists);
        assert_eq!(d1.name, "t");
        let SQLStatement::DropObject(d2) = &stmts[1] else {
            panic!()
        };
        assert_eq!(d2.object_type, DropObjectType::View);
        let SQLStatement::DropObject(d3) = &stmts[2] else {
            panic!()
        };
        assert_eq!(d3.object_type, DropObjectType::Index);
        assert!(!d3.if_exists);
    }

    #[test]
    fn test_with_cte() {
        let stmt = first("WITH x AS (SELECT 1), y (a, b) AS (SELECT 1, 2) SELECT * FROM x");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        assert_eq!(s.with_cte.len(), 2);
        assert_eq!(s.with_cte[0].name, "x");
        assert!(s.with_cte[0].columns.is_empty());
        assert_eq!(s.with_cte[1].name, "y");
        assert_eq!(
            s.with_cte[1].columns,
            vec!["a".to_string(), "b".to_string()]
        );
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let SelectItem::Expr(
            SQLExpr::Case {
                expr,
                whens,
                else_expr,
            },
            alias,
        ) = &s.columns[0]
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
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
        let Some(wc) = &s.where_clause else {
            panic!("no where")
        };
        walk(wc, &mut |e| match e {
            SQLExpr::Between { not, .. } => found_between = !*not,
            SQLExpr::InList { not, .. } => found_in = *not,
            SQLExpr::Like { not, .. } => found_not_like = *not,
            SQLExpr::IsNull { not, .. } => found_is_null = *not,
            SQLExpr::Exists(..) => found_exists = true,
            _ => {}
        });
        assert!(found_between && found_in && found_not_like && found_is_null && found_exists);
    }

    #[test]
    fn test_subquery_and_nested() {
        let stmt = first("SELECT (a + b) * c FROM t WHERE id IN (SELECT uid FROM u)");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let SelectItem::Expr(e, _) = &s.columns[0] else {
            panic!()
        };
        let SQLExpr::BinaryOp { op, .. } = e else {
            panic!("not binary")
        };
        assert_eq!(*op, BinaryOpType::Mul);
        let Some(SQLExpr::InSubQuery { expr, query, not }) = &s.where_clause else {
            panic!("not in-subquery")
        };
        assert!(!not);
        assert_eq!(**expr, SQLExpr::Identifier(vec!["id".into()]));
        let SQLStatement::Select(q) = query.as_ref() else {
            panic!()
        };
        assert_eq!(q.columns.len(), 1);
    }

    #[test]
    fn test_subquery_in_from() {
        let stmt = first("SELECT s.a FROM (SELECT a FROM t) s WHERE s.a > 1");
        let SQLStatement::Select(outer) = stmt else {
            panic!("not select")
        };
        let TableReference::SubQuery(inner, alias) = outer.from.as_ref().unwrap() else {
            panic!("not subquery")
        };
        assert_eq!(alias, "s");
        let SQLStatement::Select(q) = inner.as_ref() else {
            panic!()
        };
        assert_eq!(q.columns.len(), 1);
        assert_eq!(
            q.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["a".into()]), None)
        );
    }

    #[test]
    fn test_function_and_unary() {
        let stmt = first("SELECT COALESCE(a, 0), -x, NOT y FROM t WHERE NOT EXISTS (SELECT 1)");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Identifier(vec!["col".into()]), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(SQLExpr::Identifier(vec!["t".into(), "x".into()]), None)
        );
        assert_eq!(
            s.columns[2],
            SelectItem::Expr(SQLExpr::NumberLiteral("1".into()), Some("one".into()))
        );
        let TableReference::Table { name, alias, .. } = s.from.as_ref().unwrap() else {
            panic!("not table")
        };
        assert_eq!(name, "t");
        assert_eq!(alias.as_deref(), Some("al"));
    }

    #[test]
    fn test_string_literal_escapes_in_sql() {
        let stmt = first(r#"SELECT 'it''s', 'a\'b'"#);
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
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
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::BinaryOp { .. }) = &s.where_clause else {
            panic!()
        };
        let sql = crate::format::format_statement(&SQLStatement::Select(s));
        assert!(sql.contains("?"));
    }

    #[test]
    fn test_not_precedence_below_comparison() {
        // NOT 必须作用于整个比较表达式，而不是只吃掉左边的 primary
        let stmt = first("SELECT * FROM t WHERE NOT name LIKE 'a%'");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::UnaryOp {
            op: UnaryOpType::Not,
            expr,
        }) = &s.where_clause
        else {
            panic!("expected NOT (...) at top level, got {:?}", s.where_clause)
        };
        // 正向 LIKE 由 parse_comparison 生成为 BinaryOp{Like}
        assert!(
            matches!(
                expr.as_ref(),
                SQLExpr::BinaryOp {
                    op: BinaryOpType::Like,
                    ..
                }
            ),
            "NOT 应作用于 LIKE 整体，实际: {expr:?}"
        );

        let stmt = first("SELECT * FROM t WHERE NOT id IN (1, 2)");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::UnaryOp {
            op: UnaryOpType::Not,
            expr,
        }) = &s.where_clause
        else {
            panic!("expected NOT (...) at top level")
        };
        assert!(matches!(expr.as_ref(), SQLExpr::InList { not: false, .. }));

        let stmt = first("SELECT * FROM t WHERE NOT EXISTS (SELECT 1 FROM u)");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::UnaryOp {
            op: UnaryOpType::Not,
            expr,
        }) = &s.where_clause
        else {
            panic!("expected NOT (...) at top level")
        };
        assert!(matches!(expr.as_ref(), SQLExpr::Exists(_, false)));

        // NOT a = 1 与 NOT a BETWEEN 1 AND 2 同理
        let stmt = first("SELECT * FROM t WHERE NOT a = 1");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::UnaryOp { expr, .. }) = &s.where_clause else {
            panic!("expected NOT (...) at top level")
        };
        assert!(matches!(
            expr.as_ref(),
            SQLExpr::BinaryOp {
                op: BinaryOpType::Eq,
                ..
            }
        ));

        let stmt = first("SELECT * FROM t WHERE NOT a BETWEEN 1 AND 2");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::UnaryOp { expr, .. }) = &s.where_clause else {
            panic!("expected NOT (...) at top level")
        };
        assert!(matches!(expr.as_ref(), SQLExpr::Between { not: false, .. }));

        // NOT 仍可组合在 AND/OR 内部
        let stmt = first("SELECT * FROM t WHERE a AND NOT b OR c");
        let SQLStatement::Select(s) = stmt else {
            panic!("not select")
        };
        let Some(SQLExpr::BinaryOp {
            op: BinaryOpType::Or,
            ..
        }) = &s.where_clause
        else {
            panic!("expected OR at top level")
        };
    }

    #[test]
    fn test_deep_nesting_returns_err_not_crash() {
        // 超过深度上限返回 Err（栈溢出不可 catch，会让进程直接崩溃）
        let deep = format!("SELECT {}1{}", "(".repeat(20_000), ")".repeat(20_000));
        let err = parse_sql(&deep).unwrap_err();
        assert!(err.contains("nesting too deep"), "{err}");
        // 一元运算的递归同样受限
        let negs = format!("SELECT {}1", "-".repeat(20_000));
        let err = parse_sql(&negs).unwrap_err();
        assert!(err.contains("nesting too deep"), "{err}");
        let nots = format!("SELECT {}a", "NOT ".repeat(20_000));
        let err = parse_sql(&nots).unwrap_err();
        assert!(err.contains("nesting too deep"), "{err}");
        // 合理嵌套仍能解析
        let ok = format!("SELECT {}1{} FROM t", "(".repeat(20), ")".repeat(20));
        assert!(parse_sql(&ok).is_ok(), "{ok}");
    }

    #[test]
    fn test_mysql_limit_offset_comma() {
        // MySQL 写法：LIMIT offset, count
        let SQLStatement::Select(s) = first("SELECT * FROM t LIMIT 0, 10") else {
            panic!("not select")
        };
        assert_eq!(s.offset, Some(SQLExpr::NumberLiteral("0".into())));
        assert_eq!(s.limit, Some(SQLExpr::NumberLiteral("10".into())));
        // 原有 LIMIT n OFFSET m 不受影响
        let SQLStatement::Select(s) = first("SELECT * FROM t LIMIT 10 OFFSET 5") else {
            panic!("not select")
        };
        assert_eq!(s.limit, Some(SQLExpr::NumberLiteral("10".into())));
        assert_eq!(s.offset, Some(SQLExpr::NumberLiteral("5".into())));
        // 两种写法混用不是合法 MySQL，必须报错而不是猜
        assert!(parse_sql("SELECT * FROM t LIMIT 0, 10 OFFSET 5").is_err());
        // 格式化后语义不变
        let sql = crate::format::format_statement(&first("SELECT * FROM t LIMIT 0, 10"));
        assert_eq!(sql, "SELECT * FROM t LIMIT 10 OFFSET 0");
        assert!(parse_sql(&sql).is_ok());
    }

    #[test]
    fn test_mysql_variables() {
        let SQLStatement::Select(s) = first("SELECT @@version, @x, @@GLOBAL.sql_mode FROM t")
        else {
            panic!("not select")
        };
        assert_eq!(
            s.columns[0],
            SelectItem::Expr(SQLExpr::Variable("@@version".into()), None)
        );
        assert_eq!(
            s.columns[1],
            SelectItem::Expr(SQLExpr::Variable("@x".into()), None)
        );
        assert_eq!(
            s.columns[2],
            SelectItem::Expr(SQLExpr::Variable("@@GLOBAL.sql_mode".into()), None)
        );
        // WHERE 中同样可用，且格式化后按原样输出（加引号会变成同名列）
        let stmt = first("SELECT a FROM t WHERE id = @uid");
        assert_eq!(
            crate::format::format_statement(&stmt),
            "SELECT a FROM t WHERE id = @uid"
        );
    }

    #[test]
    fn test_explain_prefix_stripped() {
        // EXPLAIN 前缀剥离：内层语句按其本来类型返回（防火墙据此套用该类型的策略）
        assert!(matches!(first("EXPLAIN SELECT 1"), SQLStatement::Select(_)));
        assert!(matches!(
            first("EXPLAIN ANALYZE SELECT a FROM t WHERE id = 1"),
            SQLStatement::Select(_)
        ));
        // EXPLAIN ANALYZE 会真正执行语句 —— 必须暴露成 Delete 才拦得住
        assert!(matches!(
            first("EXPLAIN DELETE FROM t"),
            SQLStatement::Delete(_)
        ));
        assert!(matches!(
            first("EXPLAIN /* c */ UPDATE t SET a = 1"),
            SQLStatement::Update(_)
        ));
        // EXPLAIN 后无语句、以及尚不支持的 FORMAT 形式：返回 Err 而非 panic（外层 fail-closed）
        assert!(parse_sql("EXPLAIN").is_err());
        assert!(parse_sql("EXPLAIN FORMAT=TREE SELECT 1").is_err());
    }

    #[test]
    fn test_too_many_statements_returns_err() {
        // 迭代上限耗尽必须报错，不能静默丢掉未解析的 DROP
        let sql = format!("{}DROP TABLE users", ";".repeat(10_001));
        let err = parse_sql(&sql).unwrap_err();
        assert!(err.contains("too many statements"), "{err}");
        // 边界内仍然正常
        let ok = format!("{}SELECT 1", ";".repeat(10));
        assert_eq!(parse_sql(&ok).unwrap().len(), 1);
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
