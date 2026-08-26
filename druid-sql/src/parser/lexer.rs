use crate::token::{lookup_keyword, Token};

/// SQL 词法分析器
pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(sql: &str) -> Self {
        Lexer {
            chars: sql.chars().collect(),
            pos: 0,
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
        self.skip_whitespace();

        match self.peek() {
            None => Token::Eof,
            Some(c) => {
                // 注释
                if c == '-' && self.peek_next() == Some('-') {
                    self.advance();
                    self.advance();
                    let comment = self.read_line_comment();
                    return Token::Comment(comment);
                }
                if c == '/' && self.peek_next() == Some('*') {
                    self.advance();
                    self.advance();
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
                match c {
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
        assert_eq!(tokenize("123"), vec![Token::Number("123".into()), Token::Eof]);
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
