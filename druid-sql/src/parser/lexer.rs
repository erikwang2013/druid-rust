use crate::token::{lookup_keyword, Token};
use std::collections::VecDeque;

/// /*! ... */ 可执行注释的最大展开层数（正常 SQL 不会嵌套，设上限防止恶意输入递归膨胀）
const MAX_EXEC_COMMENT_DEPTH: u8 = 4;

/// MySQL 的 `--` 注释要求后随 ASCII 空白/控制字符（或行尾）。
/// 不能用 `char::is_whitespace()`：那是 Unicode 语义，U+00A0（不换行空格）会被误判成
/// 注释开始并吞掉整行，而 MySQL/MariaDB 对 `--\u{a0}` 一律报 1064 —— 解析结果与真实语义分歧。
fn is_comment_space(c: char) -> bool {
    c.is_ascii_whitespace() || c.is_ascii_control()
}

/// SQL 词法分析器（MySQL 方言：`--` 需后随空白，`/*!` 内容按代码处理）
pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    /// 可执行注释展开出的 token（按顺序排在后续词法扫描之前）
    pending: VecDeque<Token>,
    /// 当前可执行注释展开层数
    exec_depth: u8,
    /// 词法致命错误：由 Parser 取出并转成 Err（token 流本身继续，不截断）
    error: Option<String>,
}

impl Lexer {
    pub fn new(sql: &str) -> Self {
        Lexer {
            chars: sql.chars().collect(),
            pos: 0,
            pending: VecDeque::new(),
            exec_depth: 0,
            error: None,
        }
    }

    /// 取出并清除词法错误（Parser 在语句边界调用，用于把词法失败变成解析失败）
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// 记录词法错误。**不截断 token 流**：后续 token 继续产出，
    /// 让按 token 兜底扫描的调用方（wall 的 classify / 多语句检测）仍看得见全部内容；
    /// 上层由 Parser 把该错误转成 Err（fail-closed）
    fn record_error(&mut self, msg: String) {
        if self.error.is_none() {
            self.error = Some(msg);
        }
    }

    /// 把 /*! ... */ 的内容当普通 SQL 代码重新 token 化并排队。
    /// 内容会被 MySQL 真正执行，丢弃即漏检（如 /*!50000 SLEEP(5) */）。
    fn expand_exec_comment(&mut self, content: &str) {
        let mut sub = Lexer {
            chars: content.chars().collect(),
            pos: 0,
            pending: VecDeque::new(),
            exec_depth: self.exec_depth + 1,
            error: None,
        };
        loop {
            let token = sub.next_token();
            if token == Token::Eof {
                break;
            }
            self.pending.push_back(token);
        }
        // 子词法器的错误要冒泡（否则超深嵌套在展开层里被静默吞掉）
        if let Some(e) = sub.take_error() {
            self.record_error(e);
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                s.push(self.advance().expect("peeked char missing"));
            } else {
                break;
            }
        }
        s
    }

    fn read_number(&mut self) -> String {
        let mut s = String::new();
        let mut has_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(self.advance().expect("peeked char missing"));
            } else if c == '.' && !has_dot {
                has_dot = true;
                s.push(self.advance().expect("peeked char missing"));
            } else {
                break;
            }
        }
        s
    }

    fn read_string(&mut self, quote: char) -> String {
        self.advance(); // skip opening quote
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == '\\' {
                s.push(self.advance().unwrap_or('\\'));
            } else if c == quote {
                if self.peek() == Some(quote) {
                    self.advance();
                    s.push(quote);
                } else {
                    break;
                }
            } else {
                s.push(c);
            }
        }
        s
    }

    fn read_quoted_ident(&mut self, quote: char) -> String {
        self.advance(); // skip opening quote
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == quote {
                if self.peek() == Some(quote) {
                    self.advance();
                    s.push(quote);
                } else {
                    break;
                }
            } else {
                s.push(c);
            }
        }
        s
    }

    fn read_line_comment(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == '\n' {
                break;
            }
            s.push(c);
        }
        s
    }

    fn read_block_comment(&mut self) -> String {
        let mut s = String::new();
        let mut depth = 1;
        while let Some(c) = self.advance() {
            if c == '/' && self.peek() == Some('*') {
                self.advance();
                depth += 1;
                s.push_str("/*");
            } else if c == '*' && self.peek() == Some('/') {
                self.advance();
                depth -= 1;
                if depth == 0 {
                    break;
                }
                s.push_str("*/");
            } else {
                s.push(c);
            }
        }
        s
    }

    pub fn next_token(&mut self) -> Token {
        loop {
            // 可执行注释展开出的 token 优先返回（`continue` 后也必须先取队列）
            if let Some(token) = self.pending.pop_front() {
                return token;
            }
            self.skip_whitespace();

            match self.peek() {
                None => return Token::Eof,
                Some(c) => {
                    // `--` 注释：MySQL 要求后随空白/控制字符（或行尾）才算注释。
                    // SELECT 1--SLEEP(5) 在 MySQL 里是 1 - (-SLEEP(5))，SLEEP 会真的执行，
                    // 一律当注释吞掉整行会让 AST 里看不到 SLEEP。
                    if c == '-' && self.peek_next() == Some('-') {
                        let after = self.chars.get(self.pos + 2).copied();
                        let is_comment = match after {
                            None => true,
                            Some(ch) => is_comment_space(ch),
                        };
                        if is_comment {
                            self.advance();
                            self.advance();
                            let comment = self.read_line_comment();
                            return Token::Comment(comment);
                        }
                    }
                    if c == '/' && self.peek_next() == Some('*') {
                        self.advance();
                        self.advance();
                        // MySQL 可执行注释 /*! [版本号] 代码 */：内容会被执行
                        if self.peek() == Some('!') {
                            if self.exec_depth >= MAX_EXEC_COMMENT_DEPTH {
                                // 超限不静默丢弃：记错误让 parse_sql 失败（fail-closed），
                                // 但仍按普通块注释产出 token —— token 流保持完整，
                                // 按 token 兜底扫描的调用方仍看得见后续内容
                                self.record_error(format!(
                                    "executable comment nested too deep (> {MAX_EXEC_COMMENT_DEPTH})"
                                ));
                                let comment = self.read_block_comment();
                                return Token::BlockComment(comment);
                            }
                            self.advance();
                            let content = self.read_block_comment();
                            // 去掉紧跟的 5/6 位版本号（如 /*!50000）
                            let code = content.trim_start_matches(|ch: char| ch.is_ascii_digit());
                            self.expand_exec_comment(code);
                            continue;
                        }
                        let comment = self.read_block_comment();
                        return Token::BlockComment(comment);
                    }

                    // 字符串
                    if c == '\'' || c == 'N' && self.peek_next() == Some('\'') {
                        if c == 'N' {
                            self.advance();
                        }
                        return Token::StringLit(self.read_string('\''));
                    }
                    if c == 'X' && self.peek_next() == Some('\'') {
                        self.advance();
                        return Token::HexString(self.read_string('\''));
                    }

                    // 标识符（包括被引号包裹的）
                    if c == '"' || c == '`' {
                        return Token::QuotedIdent(self.read_quoted_ident(c));
                    }

                    // MySQL 变量：@user_var / @@system_var / @@GLOBAL.name / @'quoted'
                    if c == '@' {
                        let mut s = String::from("@");
                        self.advance();
                        if self.peek() == Some('@') {
                            self.advance();
                            s.push('@');
                        }
                        match self.peek() {
                            Some(quote @ ('\'' | '"' | '`')) => {
                                s.push_str(&self.read_quoted_ident(quote));
                            }
                            _ => {
                                while let Some(ch) = self.peek() {
                                    if ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '$' {
                                        s.push(self.advance().expect("peeked char missing"));
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                        return Token::Variable(s);
                    }

                    // 数字
                    if c.is_ascii_digit() {
                        return Token::Number(self.read_number());
                    }
                    // 小数 .123
                    if c == '.' && self.peek_next().is_some_and(|n| n.is_ascii_digit()) {
                        return Token::Number(self.read_number());
                    }

                    // 标识符或关键字
                    if c.is_alphabetic() || c == '_' {
                        let ident = self.read_ident();
                        if let Some(kw) = lookup_keyword(&ident) {
                            return kw;
                        }
                        return Token::Ident(ident);
                    }

                    // 操作符和标点（单字符先行匹配）
                    self.advance();
                    return match c {
                        '?' => Token::Placeholder,
                        '=' => Token::Eq,
                        '<' => match self.peek() {
                            Some('>') => {
                                self.advance();
                                Token::Neq
                            }
                            Some('=') => {
                                self.advance();
                                Token::Leq
                            }
                            _ => Token::Lt,
                        },
                        '>' => match self.peek() {
                            Some('=') => {
                                self.advance();
                                Token::Geq
                            }
                            _ => Token::Gt,
                        },
                        '+' => Token::Plus,
                        '-' => match self.peek() {
                            Some('>') => {
                                self.advance();
                                Token::Arrow
                            }
                            _ => Token::Minus,
                        },
                        '*' => Token::Mul,
                        '/' => Token::Div,
                        '%' => Token::Mod,
                        '.' => Token::Dot,
                        ',' => Token::Comma,
                        ';' => Token::Semicolon,
                        '(' => Token::LParen,
                        ')' => Token::RParen,
                        '[' => Token::LBracket,
                        ']' => Token::RBracket,
                        ':' => match self.peek() {
                            Some(':') => {
                                self.advance();
                                Token::DoubleColon
                            }
                            Some('=') => {
                                self.advance();
                                Token::Assign
                            }
                            _ => Token::Ident(":".to_string()),
                        },
                        '|' => {
                            if self.peek() == Some('|') {
                                self.advance();
                                Token::Concat
                            } else {
                                Token::Ident("|".to_string())
                            }
                        }
                        _ => Token::Ident(c.to_string()),
                    };
                }
            }
        }
    }
}

/// 将 SQL 文本转换为 Token 流
pub fn tokenize(sql: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(sql);
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token();
        if token == Token::Eof {
            tokens.push(token);
            break;
        }
        tokens.push(token);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_select() {
        let tokens = tokenize("SELECT id, name FROM users");
        assert!(tokens.contains(&Token::Select));
        assert!(tokens.contains(&Token::Ident("id".to_string())));
        assert!(tokens.contains(&Token::Ident("name".to_string())));
        assert!(tokens.contains(&Token::From));
        assert!(tokens.contains(&Token::Ident("users".to_string())));
    }

    #[test]
    fn test_string() {
        let tokens = tokenize("'hello world'");
        assert_eq!(tokens[0], Token::StringLit("hello world".to_string()));
    }

    #[test]
    fn test_where_clause() {
        let tokens = tokenize("WHERE age > 18 AND name LIKE '%foo%'");
        assert!(tokens.contains(&Token::Where));
        assert!(tokens.contains(&Token::Gt));
        assert!(tokens.contains(&Token::And));
        assert!(tokens.contains(&Token::Like));
    }

    #[test]
    fn test_comment_skip() {
        let tokens = tokenize("SELECT 1 -- inline comment\nSELECT 2");
        assert_eq!(tokens.iter().filter(|t| **t == Token::Select).count(), 2);
    }

    #[test]
    fn test_empty_and_whitespace() {
        assert_eq!(tokenize(""), vec![Token::Eof]);
        assert_eq!(tokenize("  \t\n\r "), vec![Token::Eof]);
    }

    #[test]
    fn test_numbers() {
        assert_eq!(
            tokenize("123"),
            vec![Token::Number("123".into()), Token::Eof]
        );
        assert_eq!(
            tokenize("3.14"),
            vec![Token::Number("3.14".into()), Token::Eof]
        );
        assert_eq!(tokenize(".5"), vec![Token::Number(".5".into()), Token::Eof]);
        // 两个点只吞第一个，剩余部分以 . 开头重新读成小数
        assert_eq!(
            tokenize("1.2.3"),
            vec![
                Token::Number("1.2".into()),
                Token::Number(".3".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_operators() {
        assert_eq!(
            tokenize("<= >= <> < > = + - * / % ||"),
            vec![
                Token::Leq,
                Token::Geq,
                Token::Neq,
                Token::Lt,
                Token::Gt,
                Token::Eq,
                Token::Plus,
                Token::Minus,
                Token::Mul,
                Token::Div,
                Token::Mod,
                Token::Concat,
                Token::Eof
            ]
        );
        assert_eq!(
            tokenize("-> :: :="),
            vec![Token::Arrow, Token::DoubleColon, Token::Assign, Token::Eof]
        );
    }

    #[test]
    fn test_quoted_ident_doubled_quote() {
        assert_eq!(
            tokenize("\"a\"\"b\""),
            vec![Token::QuotedIdent("a\"b".into()), Token::Eof]
        );
        assert_eq!(
            tokenize("`weird`"),
            vec![Token::QuotedIdent("weird".into()), Token::Eof]
        );
    }

    #[test]
    fn test_string_escapes() {
        // 反斜杠转义
        assert_eq!(
            tokenize(r"'it\'s'"),
            vec![Token::StringLit("it's".into()), Token::Eof]
        );
        // SQL 标准双写引号转义
        assert_eq!(
            tokenize("'it''s'"),
            vec![Token::StringLit("it's".into()), Token::Eof]
        );
        // 空字符串
        assert_eq!(
            tokenize("''"),
            vec![Token::StringLit("".into()), Token::Eof]
        );
        // N'...' 前缀
        assert_eq!(
            tokenize("N'abc'"),
            vec![Token::StringLit("abc".into()), Token::Eof]
        );
        // X'...' 十六进制
        assert_eq!(
            tokenize("X'AB01'"),
            vec![Token::HexString("AB01".into()), Token::Eof]
        );
        // 未闭合字符串不 panic，吞到末尾
        assert_eq!(
            tokenize("'abc"),
            vec![Token::StringLit("abc".into()), Token::Eof]
        );
    }

    #[test]
    fn test_comments() {
        assert_eq!(
            tokenize("-- hi\nSELECT 1"),
            vec![
                Token::Comment(" hi".into()),
                Token::Select,
                Token::Number("1".into()),
                Token::Eof
            ]
        );
        // 块注释（含嵌套）
        assert_eq!(
            tokenize("/* a /* b */ c */ SELECT 1"),
            vec![
                Token::BlockComment(" a /* b */ c ".into()),
                Token::Select,
                Token::Number("1".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_dash_dash_requires_whitespace() {
        // MySQL: `--` 后无空白/控制字符时不是注释，而是两个减号
        let tokens = tokenize("SELECT 1--SLEEP(5)");
        assert!(!tokens.iter().any(|t| matches!(t, Token::Comment(_))));
        assert!(tokens.contains(&Token::Minus));
        assert!(tokens
            .iter()
            .any(|t| matches!(t, Token::Ident(s) if s == "SLEEP")));

        // 后随空白、换行或行尾才算注释
        assert_eq!(
            tokenize("-- x"),
            vec![Token::Comment(" x".into()), Token::Eof]
        );
        assert_eq!(
            tokenize("--"),
            vec![Token::Comment(String::new()), Token::Eof]
        );
        assert_eq!(
            tokenize("--\nSELECT 1"),
            vec![
                Token::Comment("".into()),
                Token::Select,
                Token::Number("1".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_dash_dash_ascii_whitespace_only_matches_mysql() {
        // 与 MySQL 语义对齐：`--` 后只认 ASCII 空白/控制字符才算注释开始。
        // Unicode 空白（U+00A0 不换行空格、U+3000 全角空格、U+0085 NEL）在 MySQL/MariaDB
        // 都是一律 1064 —— 若这里当注释吞掉整行，`; DROP TABLE users` 就会从 AST 里消失
        for sql in [
            "SELECT 1 --\u{a0}; DROP TABLE users",
            "SELECT 1 --\u{3000}; DROP TABLE users",
            "SELECT 1 --\u{85}; DROP TABLE users",
        ] {
            let tokens = tokenize(sql);
            assert!(
                !tokens.iter().any(|t| matches!(t, Token::Comment(_))),
                "{sql:?} 被当成注释：Unicode 空白后不应开始注释"
            );
            assert!(tokens.contains(&Token::Drop), "{sql:?} 的 DROP 必须可见");
            assert!(tokens.contains(&Token::Table), "{sql:?} 的 TABLE 必须可见");
        }

        // ASCII 空白/控制字符（含 \x0b \x0c \0）与行尾：才算注释
        for after in [" ", "\t", "\n", "\r", "\u{b}", "\u{c}", "\0"] {
            let tokens = tokenize(&format!("--{after}x"));
            assert!(
                matches!(tokens[0], Token::Comment(_)),
                "--{after:?} 应开始注释（MySQL 认这些字符），实测 {:?}",
                tokens[0]
            );
        }
        assert_eq!(
            tokenize("--"),
            vec![Token::Comment(String::new()), Token::Eof]
        );
    }

    #[test]
    fn test_exec_comment_depth_ceiling_is_error_matches_mysql() {
        // 与 MySQL 语义对齐：超过展开上限不再静默丢弃（丢弃 = 内容对墙不可见 = 放行），
        // 而是词法失败 → parse_sql 报 Err → 上层 fail-closed 拒绝。
        // MySQL/MariaDB 本身不支持嵌套 /*!（均报 1064），拒绝与真实语义一致。
        let sql = "SELECT 1 /*! /*! /*! /*! /*! INTO OUTFILE '/tmp/x' */ */ */ */ */";
        let err = crate::parser::parse_sql(sql).unwrap_err();
        assert!(err.contains("too deep"), "错误信息应说明超深：{err}");

        let err = crate::parser::parse_sql("/*! /*! /*! /*! /*! DROP TABLE users */ */ */ */ */")
            .unwrap_err();
        assert!(err.contains("too deep"), "错误信息应说明超深：{err}");

        // token 流不截断：超深注释之后的内容仍按 token 产出
        // （按 token 兜底扫描的调用方 —— 如 wall 的多语句检测 —— 必须看得见它们）
        let tokens = tokenize("/*! /*! /*! /*! /*! X */ */ */ */ */ DROP TABLE users");
        assert!(
            tokens.contains(&Token::Drop),
            "超深注释后的 DROP 必须仍可见"
        );
        assert!(
            tokens.contains(&Token::Table),
            "超深注释后的 TABLE 必须仍可见"
        );

        // 上限之内仍然展开（不误伤正常写法）
        assert!(crate::parser::parse_sql("SELECT 1 /*!50000 , SLEEP(5) */").is_ok());
        // 普通块注释的嵌套与 /*! 无关，不受影响
        assert!(tokenize("/* a /* b */ c */")
            .iter()
            .any(|t| matches!(t, Token::BlockComment(_))));
    }

    #[test]
    fn test_executable_comment_expands_content() {
        // /*!50000 ... */ 内容会被 MySQL 执行，不能整体丢弃
        let tokens = tokenize("SELECT 1 /*!50000 , SLEEP(5) */");
        assert!(tokens.contains(&Token::Comma));
        assert!(tokens
            .iter()
            .any(|t| matches!(t, Token::Ident(s) if s == "SLEEP")));
        assert!(tokens.contains(&Token::Number("5".into())));

        // 无版本号形式同样展开
        let tokens = tokenize("SELECT * FROM t /*! WHERE 1 = 1 */");
        assert!(tokens.contains(&Token::Where));

        // 普通块注释仍然作为注释保留
        assert_eq!(
            tokenize("/* plain */ SELECT 1"),
            vec![
                Token::BlockComment(" plain ".into()),
                Token::Select,
                Token::Number("1".into()),
                Token::Eof
            ]
        );
    }

    #[test]
    fn test_executable_comment_visible_in_ast() {
        // 端到端：可执行注释里的 SLEEP 必须出现在 AST 中（防火墙据此拦截）
        let stmts = crate::parser::parse_sql("SELECT 1 /*!50000 , SLEEP(5)*/ FROM t").unwrap();
        let sql = crate::format::format_statement(&stmts[0]);
        assert!(sql.contains("SLEEP(5)"), "{sql}");

        // SELECT 1--SLEEP(5) 是 1 - (-SLEEP(5))，SLEEP 同样可见
        let stmts = crate::parser::parse_sql("SELECT 1--SLEEP(5) FROM t").unwrap();
        let sql = crate::format::format_statement(&stmts[0]);
        assert!(sql.contains("SLEEP(5)"), "{sql}");
    }

    #[test]
    fn test_variable_tokens() {
        assert_eq!(
            tokenize("@@version @x @@GLOBAL.sql_mode @'q'"),
            vec![
                Token::Variable("@@version".into()),
                Token::Variable("@x".into()),
                Token::Variable("@@GLOBAL.sql_mode".into()),
                Token::Variable("@q".into()),
                Token::Eof
            ]
        );
        // 单独的 @ 退化为原行为（标识符 "@"），不会 panic
        assert_eq!(
            tokenize("@ "),
            vec![Token::Variable("@".into()), Token::Eof]
        );
    }

    #[test]
    fn test_misc_tokens() {
        assert_eq!(
            tokenize("a_b1 ? ; ( ) [ ] , ."),
            vec![
                Token::Ident("a_b1".into()),
                Token::Placeholder,
                Token::Semicolon,
                Token::LParen,
                Token::RParen,
                Token::LBracket,
                Token::RBracket,
                Token::Comma,
                Token::Dot,
                Token::Eof
            ]
        );
        // 关键字大小写不敏感
        assert_eq!(
            tokenize("sElEcT FrOm"),
            vec![Token::Select, Token::From, Token::Eof]
        );
    }

    #[test]
    fn test_next_token_sequential() {
        // 逐 token 读取与 tokenize 一致
        let mut l = Lexer::new("SELECT 1");
        assert_eq!(l.next_token(), Token::Select);
        assert_eq!(l.next_token(), Token::Number("1".into()));
        assert_eq!(l.next_token(), Token::Eof);
        assert_eq!(l.next_token(), Token::Eof); // Eof 后仍返回 Eof
    }
}
