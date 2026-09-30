use crate::config::{DenyOperation, WallConfig};
use druid_sql::ast::SQLStatement;
use druid_sql::token::Token;

#[derive(Debug, Clone)]
pub struct WallCheckResult {
    pub allowed: bool,
    pub violations: Vec<Violation>,
}
impl WallCheckResult {
    pub fn pass() -> Self {
        WallCheckResult {
            allowed: true,
            violations: vec![],
        }
    }
    pub fn deny(msg: String) -> Self {
        WallCheckResult {
            allowed: false,
            violations: vec![Violation::new(&msg)],
        }
    }
}
#[derive(Debug, Clone)]
pub struct Violation {
    pub message: String,
}
impl Violation {
    pub fn new(msg: &str) -> Self {
        Violation {
            message: msg.into(),
        }
    }
}

pub struct WallChecker {
    config: WallConfig,
}

impl WallChecker {
    pub fn new(config: WallConfig) -> Self {
        WallChecker { config }
    }

    pub fn check(&self, sql: &str, stmt: &SQLStatement) -> WallCheckResult {
        if !self.config.enabled {
            return WallCheckResult::pass();
        }
        if sql.len() > self.config.max_sql_length {
            return WallCheckResult::deny(format!(
                "SQL len {} > max {}",
                sql.len(),
                self.config.max_sql_length
            ));
        }
        // 首关键字归类：TRUNCATE/ALTER/CREATE INDEX/GRANT/CALL 等 parser 未建模的语句
        // 在 AST 层没有对应类型，只有靠它才能命中 deny_operations
        if let Some(op) = Self::classify(sql) {
            let r = self.check_op(&op);
            if !r.allowed {
                return r;
            }
        }
        if let Some(v) = self.check_expr_rules(stmt) {
            return v;
        }
        match stmt {
            SQLStatement::Select(s) => self.check_select(sql, s),
            SQLStatement::Insert(_) => self.check_op(&DenyOperation::Insert),
            SQLStatement::Update(s) => self.check_update(s),
            SQLStatement::Delete(s) => self.check_delete(s),
            SQLStatement::CreateTable(_) => self.check_op(&DenyOperation::CreateTable),
            SQLStatement::DropObject(_) => self.check_op(&DenyOperation::DropTable),
        }
    }

    /// 解析失败时的兜底判定（fail-closed）：
    /// 1) 首关键字可归类且该类操作在 deny_operations 中 → 按归类拒绝（TRUNCATE/ALTER/...）
    /// 2) SELECT 命中 INTO OUTFILE / INTO DUMPFILE → 拒绝
    /// 3) deny_unparsable（默认 true），或语句类型完全无法识别且配置了 deny_operations → 拒绝
    pub fn check_unparsable(&self, sql: &str, err: &str) -> WallCheckResult {
        if !self.config.enabled {
            return WallCheckResult::pass();
        }
        if sql.len() > self.config.max_sql_length {
            return WallCheckResult::deny(format!(
                "SQL len {} > max {}",
                sql.len(),
                self.config.max_sql_length
            ));
        }
        // 事务控制语句：parser 未建模且不承载可拒绝语义，精确形状匹配后放行
        if is_transaction_control(sql) {
            return WallCheckResult::pass();
        }
        let kind = Self::classify(sql);
        if let Some(ref op) = kind {
            if self.config.deny_operations.contains(op) {
                return WallCheckResult::deny(format!("{} denied", op));
            }
        }
        if kind == Some(DenyOperation::Select)
            && !self.config.select_into_outfile_allow
            && has_into_file_clause(sql)
        {
            return WallCheckResult::deny("INTO OUTFILE denied".into());
        }
        let deny = self.config.deny_unparsable
            || (kind.is_none() && !self.config.deny_operations.is_empty());
        if deny {
            return WallCheckResult::deny(format!("unparseable SQL: {}", err));
        }
        WallCheckResult::pass()
    }

    /// 多语句策略：allow_multi_statements=false（默认）时拒绝多条语句
    pub fn check_statement_count(&self, count: usize) -> WallCheckResult {
        if !self.config.enabled || count <= 1 || self.config.allow_multi_statements {
            WallCheckResult::pass()
        } else {
            WallCheckResult::deny(format!("multiple statements not allowed ({} found)", count))
        }
    }

    /// 按 SQL 首关键字归类语句类型（跳过注释）。
    /// parser 只建模了 SELECT/INSERT/UPDATE/DELETE/CREATE TABLE/DROP，
    /// 其余语句类型只能在这里归类才能被 deny_operations 覆盖。
    pub fn classify(sql: &str) -> Option<DenyOperation> {
        let mut toks = tokens_without_comments(sql);
        let first = toks.next()?;
        Some(match first {
            Token::Select | Token::With => DenyOperation::Select,
            Token::Insert | Token::Replace => DenyOperation::Insert,
            Token::Update => DenyOperation::Update,
            Token::Delete => DenyOperation::Delete,
            Token::Truncate => DenyOperation::Truncate,
            Token::Alter => DenyOperation::AlterTable,
            Token::Drop => DenyOperation::DropTable,
            Token::Grant => DenyOperation::Grant,
            Token::Revoke => DenyOperation::Revoke,
            // CREATE [UNIQUE] INDEX 与 CREATE TABLE/VIEW/DATABASE 区分
            Token::Create => {
                let a = toks.next();
                let b = toks.next();
                if matches!(a, Some(Token::Index)) || matches!(b, Some(Token::Index)) {
                    DenyOperation::CreateIndex
                } else {
                    DenyOperation::CreateTable
                }
            }
            // CALL/EXECUTE 不是关键字 token，按标识符文本判定
            Token::Ident(w) => match w.to_ascii_uppercase().as_str() {
                "CALL" => DenyOperation::Call,
                "EXECUTE" => DenyOperation::Execute,
                _ => return None,
            },
            _ => return None,
        })
    }

    /// 函数黑名单 + schema 黑名单（递归遍历实现见 rules.rs）
    fn check_expr_rules(&self, stmt: &SQLStatement) -> Option<WallCheckResult> {
        crate::rules::check_expr_rules(stmt, &self.config.deny_functions, &self.config.deny_schemas)
    }

    fn check_op(&self, op: &DenyOperation) -> WallCheckResult {
        if self.config.deny_operations.contains(op) {
            WallCheckResult::deny(format!("{} denied", op))
        } else {
            WallCheckResult::pass()
        }
    }

    fn check_select(&self, sql: &str, _: &druid_sql::ast::SelectStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Select) {
            return WallCheckResult::deny("SELECT denied".into());
        }
        if !self.config.select_into_outfile_allow && has_into_file_clause(sql) {
            return WallCheckResult::deny("INTO OUTFILE denied".into());
        }
        WallCheckResult::pass()
    }

    fn check_update(&self, s: &druid_sql::ast::UpdateStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Update) {
            return WallCheckResult::deny("UPDATE denied".into());
        }
        if self.config.update_delete_require_where && s.where_clause.is_none() {
            return WallCheckResult::deny("UPDATE without WHERE".into());
        }
        WallCheckResult::pass()
    }

    fn check_delete(&self, s: &druid_sql::ast::DeleteStatement) -> WallCheckResult {
        if self.config.deny_operations.contains(&DenyOperation::Delete) {
            return WallCheckResult::deny("DELETE denied".into());
        }
        if self.config.update_delete_require_where && s.where_clause.is_none() {
            return WallCheckResult::deny("DELETE without WHERE".into());
        }
        WallCheckResult::pass()
    }

    pub fn quick_check(&self, sql: &str) -> WallCheckResult {
        if !self.config.enabled {
            return WallCheckResult::pass();
        }
        if sql.len() > self.config.max_sql_length {
            return WallCheckResult::deny("SQL too long".into());
        }
        // 两种 sql_mode 的分歧闸门：字符串边界无法确定时 fail-closed（其返回值不参与匹配）
        if let Err(reason) = strip_string_literals(sql) {
            return WallCheckResult::deny(format!("ambiguous string literal: {}", reason));
        }
        // 匹配文本一律由词法器 token 重建，见 bare_text 的 ⚠️ 说明
        let bare = bare_text(sql);
        // 函数名匹配忽略空白：SLEEP (1) 与 SLEEP(1) 同样拦截
        let compact: String = bare.chars().filter(|c| !c.is_whitespace()).collect();
        for func in &self.config.deny_functions {
            if compact.contains(&format!("{}(", func.to_lowercase())) {
                return WallCheckResult::deny(format!("forbidden function: {}", func));
            }
        }
        for kw in &self.config.deny_keywords {
            let kw_lower = kw.to_lowercase();
            if let Some(pos) = bare.find(&kw_lower) {
                let before = pos == 0 || {
                    let c = bare.as_bytes()[pos - 1];
                    !c.is_ascii_alphanumeric() && c != b'_'
                };
                let after = {
                    let end = pos + kw_lower.len();
                    end >= bare.len() || {
                        let c = bare.as_bytes()[end];
                        !c.is_ascii_alphanumeric() && c != b'_'
                    }
                };
                if before && after {
                    return WallCheckResult::deny(format!("forbidden: {}", kw));
                }
            }
        }
        WallCheckResult::pass()
    }
}

/// 用词法 token 重建「字面量之外」的匹配文本——deny_functions / deny_keywords 的唯一输入源。
///
/// ⚠️ quick_check 必须与 lexer 同源，**不得自行实现字符串解析**：字符串（`'..'`、`N'..'`）、
/// 十六进制字面量（`X'..'`）的边界一律由 `druid-sql` 词法器决定，这里只把「数据」token
/// 换成空格，其余 token 原样拼回。历史两次 `deny_keywords` 整体致盲都是因为 checker
/// 自建了引号状态机，被一个奇数引号翻转奇偶后把其后全部文本当字符串跳过：
///   1) Round 2：`\'` 致盲（自建单引号扫描只数奇偶）——修成双模式扫描后，
///   2) Round 3：`"O'Brien"` / `` `a'b` `` 里的奇数个 `'` 再次致盲（扫描器不认 `"` 与反引号），
///      真机实测 `UPDATE t SET name = "O'Brien" WHERE secret = 42` 可执行并改数据。
///
/// 注释 token 保留内容（Display 带内容）——`SELECT 1--<tab>SLEEP(1)` 的 SLEEP 写在注释里，
/// 但 MySQL 会不会执行该注释取决于 `--` 后是否真为空白，故保持拦截。
/// 引号标识符取引号内原文：MySQL/MariaDB 会真正执行 `` `SLEEP`(1) ``，不是数据。
fn bare_text(sql: &str) -> String {
    let mut out = String::new();
    for t in druid_sql::parser::lexer::tokenize(sql) {
        match t {
            // 数据 token：不参与匹配（字符串内的单词不应命中关键字/函数名）
            Token::StringLit(_) | Token::HexString(_) => out.push(' '),
            // `@x` / `@@version` / `@'a b'` 是**变量名**，MySQL 不会把它当代码执行
            // （对比 QuotedIdent：`` `SLEEP`(1) `` 会被真正执行，故那边必须保留原文）。
            // 判据与上方注释同一原则：后端当代码执行 → 保留；当数据 → 丢弃。
            Token::Variable(_) => out.push(' '),
            Token::Eof => {}
            Token::QuotedIdent(s) => {
                out.push_str(&s);
                out.push(' ');
            }
            other => {
                out.push_str(&other.to_string());
                out.push(' ');
            }
        }
    }
    out.to_lowercase()
}

/// sql_mode 分歧闸门：`\` 与 `'` 的组合在两种 sql_mode 下字符串边界不同，
/// 同一段文本对后端的语义因此无法确定，只能 fail-closed。
/// 返回值仅用于内部一致性比较，**不参与任何匹配**（匹配文本来自 `bare_text`）。
///
/// 这里按两种真实存在的行为各扫一遍：
/// - MySQL 默认模式：`\'` 与 `\\` 是转义
/// - `NO_BACKSLASH_ESCAPES` 模式：`\` 是普通字符，字符串在 `\'` 的引号处闭合
///
/// - 两遍结果一致 → 确定，返回该文本
/// - 默认模式闭合、NBE 未闭合 → 用默认模式结果（如 `'it\'s'`，默认模式合法）
/// - 默认模式未闭合、NBE 闭合 → 两种模式对「字符串边界」分歧，而 NBE 模式会
///   把分歧区间当代码执行（如 `'a\', SLEEP(1), 'x'`）→ fail-closed 拒绝
/// - 两遍都未闭合 → 保持既有行为（两端 DB 均报 1064），返回已收集的文本
///
/// ⚠️ 两次扫描不是冗余，**不要合并成一次**：同一段文本在两种 sql_mode 下
/// 字符串边界不同、执行语义也不同（NBE 下分歧区间会被当作代码执行），
/// 只扫一遍就等于让攻击者用一个 `\'` 选用对自己有利的那种解析。
/// 回归测试：`wall_rules.rs::test_sql_mode_divergence_on_escaped_quote_fails_closed`。
///
/// ⚠️ quick_check 必须与 lexer 同源，**不得再在这里（或任何地方）自行实现字符串解析**：
/// 本函数只回答「两种 sql_mode 是否分歧」，字符串/标识符/注释的边界以 `bare_text`
/// 的词法 token 为唯一实现。历史两次绕过（`\'` 与 `"`/反引号）都源于 checker 自建状态机。
fn strip_string_literals(s: &str) -> Result<String, &'static str> {
    let (esc, esc_closed) = scan_outside_strings(s, true);
    let (nbe, nbe_closed) = scan_outside_strings(s, false);
    match (esc_closed, nbe_closed) {
        (true, true) => {
            if esc == nbe {
                Ok(esc)
            } else {
                Err("backslash-quote changes string boundary")
            }
        }
        (true, false) => Ok(esc),
        (false, true) => Err("backslash-quote changes string boundary"),
        (false, false) => Ok(esc),
    }
}

/// 分歧闸门专用扫描：返回（字符串外部文本, 字符串是否全部闭合）。
/// ⚠️ 只给 `strip_string_literals` 判断两种 sql_mode 是否分歧用，产物不做匹配；
/// 任何「哪段是字符串」的判定都必须走 `bare_text` 的词法 token。
/// `backslash_escapes=true` 时 `\'` / `\\` 视为转义（MySQL 默认模式）；
/// `''`（双写引号）在两种模式下都是转义引号。
fn scan_outside_strings(s: &str, backslash_escapes: bool) -> (String, bool) {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
            continue;
        }
        // 进入字符串，找到闭合引号
        loop {
            match chars.next() {
                None => return (out, false), // 未闭合
                Some('\\') if backslash_escapes => {
                    if chars.next().is_none() {
                        return (out, false); // 字符串以反斜杠结尾
                    }
                }
                Some('\'') => {
                    if chars.peek() == Some(&'\'') {
                        chars.next(); // '' 继续留在字符串内
                    } else {
                        break;
                    }
                }
                Some(_) => {}
            }
        }
    }
    (out, true)
}

/// 事务控制语句安全通道。parser 未建模这些语句，且它们不承载可拒绝语义，
/// 但默认 fail-closed 会把 `BEGIN`/`COMMIT` 一并拒掉 —— 真实应用根本没法开事务。
///
/// 只接受精确的 token 形状（末尾分号除外），避免借控制语句夹带载荷：
/// `BEGIN; DROP TABLE t`、`SAVEPOINT a; DROP TABLE t` 形状不匹配，照常拒绝。
fn is_transaction_control(sql: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    for t in tokens_without_comments(sql) {
        let w = match t {
            Token::Ident(w) => w.to_ascii_uppercase(),
            Token::QuotedIdent(_) => "\u{ab}ident\u{bb}".to_string(), // 任意引号标识符
            Token::Semicolon => ";".to_string(),
            Token::Begin => "BEGIN".to_string(),
            Token::Commit => "COMMIT".to_string(),
            Token::Rollback => "ROLLBACK".to_string(),
            Token::End => "END".to_string(),
            Token::Start => "START".to_string(),
            Token::Transaction => "TRANSACTION".to_string(),
            Token::With => "WITH".to_string(),
            Token::Eof => continue,
            _ => return false, // 出现其它关键字/运算符/字面量 → 不是纯事务控制语句
        };
        words.push(w);
    }
    while words.last().is_some_and(|w| w == ";") {
        words.pop();
    }
    let w: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
    matches!(
        w.as_slice(),
        ["BEGIN"]
            | ["BEGIN", "WORK"]
            | ["END"]
            | ["COMMIT"]
            | ["COMMIT", "WORK"]
            | ["ROLLBACK"]
            | ["ROLLBACK", "WORK"]
            | ["ROLLBACK", "TO", _]
            | ["ROLLBACK", "TO", "SAVEPOINT", _]
            | ["START", "TRANSACTION"]
            | ["SAVEPOINT", _]
            | ["RELEASE", "SAVEPOINT", _]
    ) || (w.len() >= 2
        && w[0] == "START"
        && w[1] == "TRANSACTION"
        && w[2..].iter().all(|x| {
            matches!(
                *x,
                "READ" | "ONLY" | "WRITE" | "WITH" | "CONSISTENT" | "SNAPSHOT"
            )
        }))
}

/// SQL 词法 token（跳过注释）——空白/注释不影响判定
fn tokens_without_comments(sql: &str) -> impl Iterator<Item = Token> + '_ {
    druid_sql::parser::lexer::tokenize(sql)
        .into_iter()
        .filter(|t| !matches!(t, Token::Comment(_) | Token::BlockComment(_)))
}

/// 按 token 序列判定 INTO OUTFILE / INTO DUMPFILE。
/// token 化天然忽略空白与注释：`INTO  OUTFILE`、`INTO/**/OUTFILE`、大小写混写均可命中，
/// 不用子串匹配（子串匹配会被多空格/注释绕过，且漏掉 INTO DUMPFILE）
fn has_into_file_clause(sql: &str) -> bool {
    let mut toks = tokens_without_comments(sql).peekable();
    while let Some(t) = toks.next() {
        if t == Token::Into {
            if let Some(Token::Ident(w)) = toks.peek() {
                let w = w.to_ascii_uppercase();
                if w == "OUTFILE" || w == "DUMPFILE" {
                    return true;
                }
            }
        }
    }
    false
}
