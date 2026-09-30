use std::collections::HashMap;

/// 将驼峰命名转为下划线命名
pub fn camel_to_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_uppercase() {
            // 处理连续大写（缩写词）：URLParser → url_parser
            if i > 0
                && (!chars[i - 1].is_uppercase()
                    || (i + 1 < chars.len() && chars[i + 1].is_lowercase()))
            {
                result.push('_');
            }
            result.push_str(&c.to_lowercase().to_string());
        } else {
            result.push(c);
        }
        i += 1;
    }
    result
}

/// 将下划线命名转为驼峰命名
pub fn snake_to_camel(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize = false;
    for c in s.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            result.push(c.to_uppercase().next().unwrap_or(c));
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    result
}

/// 简单 SQL 参数替换（? 占位符），仅对「反斜杠不是转义符」的方言正确
///
/// 单次遍历完成替换，参数值中的 ? 不会干扰后续替换。
///
/// # 已废弃
///
/// 本函数只把单引号翻倍，对 PostgreSQL（`standard_conforming_strings = on` 时为默认）、
/// Oracle、SQL Server 等方言正确；MySQL/MariaDB 默认把 `\` 当转义符，用它拼接时
/// `\` + `'` 可以逃逸字面量造成注入（如参数 `["\\", " OR 1=1 -- "]`），
/// 这类方言请用 [`substitute_params_mysql`]。
///
/// 任何场景都更推荐使用数据库驱动的绑定参数，而不是字符串拼接。
#[deprecated(
    note = "不要用字符串拼接构造 SQL（此函数对 MySQL/MariaDB 不安全）；请使用绑定参数，MySQL/MariaDB 至少要用 substitute_params_mysql"
)]
pub fn substitute_params(sql: &str, params: &[&str]) -> String {
    let escaped: Vec<String> = params
        .iter()
        .map(|p| format!("'{}'", p.replace('\'', "''")))
        .collect();
    substitute_escaped(sql, &escaped)
}

/// 简单 SQL 参数替换（? 占位符），按 MySQL/MariaDB 默认转义规则处理参数
///
/// 先转义反斜杠（`\` → `\\`）再翻倍单引号，堵住 `\` 逃逸引号的注入路径。
/// 注意：`NO_BACKSLASH_ESCAPES` 模式下反斜杠本就是普通字符，多转义出的 `\\`
/// 会被原样存成两个反斜杠（数据语义改变），该模式请使用绑定参数。
///
/// 与 [`substitute_params`] 一样，推荐优先使用数据库驱动的绑定参数。
pub fn substitute_params_mysql(sql: &str, params: &[&str]) -> String {
    let escaped: Vec<String> = params
        .iter()
        .map(|p| format!("'{}'", p.replace('\\', "\\\\").replace('\'', "''")))
        .collect();
    substitute_escaped(sql, &escaped)
}

/// 扫描引号区，返回闭合引号之后的下标（未闭合则到串尾）
///
/// 单/双引号按 MySQL 默认规则：`''`/`""` 双写与 `\x` 都是转义；
/// 反引号是标识符，只认双写（其中的 `\` 无特殊含义）。
fn skip_quoted(cs: &[char], start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < cs.len() {
        match cs[i] {
            c if c == quote => {
                i += 1;
                if cs.get(i) != Some(&quote) {
                    break; // 未被双写 → 闭合
                }
                i += 1; // 双写转义，仍在串内
            }
            '\\' if quote != '`' => i = (i + 2).min(cs.len()),
            _ => i += 1,
        }
    }
    i
}

/// 行注释结束位置（换行符本身不含在内；无换行则到串尾）
fn line_comment_end(cs: &[char], start: usize) -> usize {
    cs[start..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(cs.len(), |p| start + p)
}

/// 把已转义好的字面量按 ? 顺序填入 SQL
///
/// 词法状态机：只有在普通态出现的 `?` 才是占位符。单引号/双引号/反引号字符串、
/// `#`/`--` 行注释与 `/* */` 块注释里的 `?` 原样保留，
/// 否则 `SELECT 'a?b', ?` 会被插成 `SELECT 'a'x' b', ...` 这样的坏 SQL。
fn substitute_escaped(sql: &str, escaped: &[String]) -> String {
    let cs: Vec<char> = sql.chars().collect();
    let mut result = String::with_capacity(sql.len() + escaped.len() * 8);
    let mut param_idx = 0;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        match c {
            '\'' | '"' | '`' => {
                let end = skip_quoted(&cs, i, c);
                result.extend(&cs[i..end]);
                i = end;
            }
            // 行注释：MySQL 的 `#`（无空白要求）与 `--`（要求后跟空白或行尾，
            // `5--2` 是 5 减负 2，不是注释）
            '#' => {
                let end = line_comment_end(&cs, i);
                result.extend(&cs[i..end]);
                i = end;
            }
            '-' if cs.get(i + 1) == Some(&'-')
                && cs.get(i + 2).is_none_or(|c| c.is_whitespace()) =>
            {
                let end = line_comment_end(&cs, i);
                result.extend(&cs[i..end]);
                i = end;
            }
            // 块注释（含 MySQL 版本注释 /*! ... */）：未闭合则吃到串尾
            '/' if cs.get(i + 1) == Some(&'*') => {
                let close = cs[i + 2..]
                    .windows(2)
                    .position(|w| w[0] == '*' && w[1] == '/')
                    .map_or(cs.len(), |p| i + 2 + p + 2);
                result.extend(&cs[i..close]);
                i = close;
            }
            '?' if param_idx < escaped.len() => {
                result.push_str(&escaped[param_idx]);
                param_idx += 1;
                i += 1;
            }
            _ => {
                result.push(c);
                i += 1;
            }
        }
    }
    result
}

/// 解析连接属性字符串 "key1=value1;key2=value2"
pub fn parse_properties(prop_str: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in prop_str.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// 截断 SQL 用于日志显示（安全处理多字节字符）
pub fn truncate_sql(sql: &str, max_len: usize) -> String {
    if sql.len() <= max_len {
        sql.to_string()
    } else {
        let end = sql
            .char_indices()
            .take(max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(max_len.min(sql.len()));
        format!("{}...", &sql[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camel_to_snake() {
        assert_eq!(camel_to_snake("DruidDataSource"), "druid_data_source");
        assert_eq!(camel_to_snake("maxActive"), "max_active");
        assert_eq!(camel_to_snake("URL"), "url");
    }

    #[test]
    fn test_camel_to_snake_acronyms() {
        assert_eq!(camel_to_snake("URLParser"), "url_parser");
        assert_eq!(camel_to_snake("XMLHttpRequest"), "xml_http_request");
        assert_eq!(camel_to_snake("getURL"), "get_url");
        assert_eq!(camel_to_snake("ABC"), "abc");
        assert_eq!(camel_to_snake("a"), "a");
        assert_eq!(camel_to_snake(""), "");
        assert_eq!(camel_to_snake("already_snake"), "already_snake");
        assert_eq!(camel_to_snake("中文Name"), "中文_name"); // 非 ASCII 后的大写同样插下划线
    }

    #[test]
    fn test_snake_to_camel() {
        assert_eq!(snake_to_camel("druid_data_source"), "druidDataSource");
        assert_eq!(snake_to_camel("max_active"), "maxActive");
        assert_eq!(snake_to_camel("url"), "url");
        assert_eq!(snake_to_camel(""), "");
        assert_eq!(snake_to_camel("_abc"), "Abc"); // 前导下划线
        assert_eq!(snake_to_camel("a__b"), "aB"); // 连续下划线
        assert_eq!(snake_to_camel("中文_名"), "中文名");
    }

    #[test]
    #[allow(deprecated)] // 仍需覆盖废弃 API 的既有行为
    fn test_substitute_params() {
        let sql = "SELECT * FROM users WHERE id = ? AND name = ?";
        let result = substitute_params(sql, &["1", "Alice"]);
        assert_eq!(
            result,
            "SELECT * FROM users WHERE id = '1' AND name = 'Alice'"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn test_substitute_params_edge_cases() {
        // 无参数：原样保留 ?
        assert_eq!(substitute_params("a = ?", &[]), "a = ?");
        // 参数少于占位符：剩余 ? 保留
        assert_eq!(
            substitute_params("a = ? AND b = ?", &["1"]),
            "a = '1' AND b = ?"
        );
        // 参数多于占位符：多余参数被忽略
        assert_eq!(substitute_params("a = ?", &["1", "2"]), "a = '1'");
        // 无占位符
        assert_eq!(substitute_params("SELECT 1", &["x"]), "SELECT 1");
        // 空字符串
        assert_eq!(substitute_params("", &[]), "");
        assert_eq!(substitute_params("?", &[""]), "''");
        // 参数值中的 ? 不再参与替换
        assert_eq!(substitute_params("a = ?", &["?"]), "a = '?'");
    }

    #[test]
    #[allow(deprecated)]
    fn test_substitute_params_quote_escape() {
        // 单引号翻倍转义：仅对反斜杠不是转义符的方言安全（见函数文档，MySQL 请用 mysql 版本）
        assert_eq!(
            substitute_params("name = ?", &["O'Reilly"]),
            "name = 'O''Reilly'"
        );
        // 参数值本身含单引号后仍与后续占位符区分
        assert_eq!(
            substitute_params("a = ? AND b = ?", &["it's", "x'y"]),
            "a = 'it''s' AND b = 'x''y'"
        );
        // Unicode 参数
        assert_eq!(substitute_params("v = ?", &["你好"]), "v = '你好'");
    }

    /// 按 MySQL 默认转义规则解析出第一个字符串字面量的原始值，返回 (字面量值, 剩余文本)
    ///
    /// 只实现 `\\`、`\'` 两种转义（恰好是转义器会产出的序列）。
    fn parse_mysql_literal(s: &str) -> Option<(String, String)> {
        let start = s.find('\'')? + 1;
        let mut value = String::new();
        let mut chars = s[start..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => value.push(chars.next()?.1), // 转义：\ 后的字符即字面值（\\ → \，\' → '）
                '\'' => return Some((value, s[start + i + 1..].to_string())),
                other => value.push(other),
            }
        }
        None
    }

    #[test]
    fn test_substitute_params_mysql_escapes() {
        // 反斜杠成对转义，单引号翻倍
        assert_eq!(substitute_params_mysql("p = ?", &["a\\b"]), "p = 'a\\\\b'");
        assert_eq!(
            substitute_params_mysql("p = ?", &["O'Reilly"]),
            "p = 'O''Reilly'"
        );
        // 组合：\' → \\''（先补反斜杠，再翻倍引号）
        assert_eq!(substitute_params_mysql("p = ?", &["\\'"]), "p = '\\\\'''");
        assert_eq!(
            substitute_params_mysql("a = ? AND b = ?", &["x'y", "z\\"]),
            "a = 'x''y' AND b = 'z\\\\'"
        );
        // 无参数 / 参数少于占位符时共享的填充行为
        assert_eq!(substitute_params_mysql("a = ?", &[]), "a = ?");
        assert_eq!(
            substitute_params_mysql("a = ? AND b = ?", &["1"]),
            "a = '1' AND b = ?"
        );
    }

    #[test]
    fn test_substitute_params_mysql_backslash_injection() {
        // 旧实现（只翻倍单引号）会生成 name = '\' AND pass = ' OR 1=1 -- '：
        // \ 吃掉第一个引号，字面量延续到 AND pass = 之后，OR 1=1 成为活的恒真条件
        let sql = substitute_params_mysql("name = ? AND pass = ?", &["\\", " OR 1=1 -- "]);
        assert_eq!(sql, "name = '\\\\' AND pass = ' OR 1=1 -- '");
        // 语义校验：按 MySQL 规则解析出的两个字面量必须恰好还原为原参数，字面量之外无残留 SQL
        let (v1, rest) = parse_mysql_literal(&sql).unwrap();
        assert_eq!(v1, "\\");
        let (v2, rest2) = parse_mysql_literal(&rest).unwrap();
        assert_eq!(v2, " OR 1=1 -- ");
        assert!(
            rest2.is_empty(),
            "字面量之外不应有可在 SQL 中生效的残留: {rest2:?}"
        );
    }

    #[test]
    fn test_substitute_params_placeholder_only_in_normal_state() {
        // 字符串字面量里的 ? 不是占位符（旧实现会把 'a?b' 插坏）
        assert_eq!(
            substitute_params_mysql("SELECT 'a?b', ? FROM t", &["x"]),
            "SELECT 'a?b', 'x' FROM t"
        );
        // 转义引号 / 双写引号内的 ? 同样原样保留
        assert_eq!(
            substitute_params_mysql(r"SELECT 'it''s ?', ? FROM t", &["y"]),
            r"SELECT 'it''s ?', 'y' FROM t"
        );
        assert_eq!(
            substitute_params_mysql(r"SELECT 'a\'?', ? FROM t", &["y"]),
            r"SELECT 'a\'?', 'y' FROM t"
        );
        // 双引号、反引号标识符
        assert_eq!(
            substitute_params_mysql(r#"SELECT "a?b", ? FROM t"#, &["y"]),
            r#"SELECT "a?b", 'y' FROM t"#
        );
        assert_eq!(
            substitute_params_mysql("SELECT `a?b`, ? FROM t", &["y"]),
            "SELECT `a?b`, 'y' FROM t"
        );
        // 行注释与块注释
        assert_eq!(
            substitute_params_mysql("SELECT ? -- 注释里的 ?\n, ?", &["a", "b"]),
            "SELECT 'a' -- 注释里的 ?\n, 'b'"
        );
        // MySQL 的 `#` 行注释到行尾（无空白要求，与 `--` 不同）
        assert_eq!(
            substitute_params_mysql("SELECT ?, # 注释里的 ?\n?", &["a", "b"]),
            "SELECT 'a', # 注释里的 ?\n'b'"
        );
        assert_eq!(
            substitute_params_mysql("SELECT /* ? */ ? FROM t", &["z"]),
            "SELECT /* ? */ 'z' FROM t"
        );
        // MySQL 里 `5--2` 不是注释，后面的 ? 仍是占位符
        assert_eq!(
            substitute_params_mysql("SELECT 5--2, ? FROM t", &["z"]),
            "SELECT 5--2, 'z' FROM t"
        );
        // 未闭合的字符串 / 块注释：整段按原样保留，不误替换
        assert_eq!(substitute_params_mysql("SELECT 'a?", &["z"]), "SELECT 'a?");
        assert_eq!(
            substitute_params_mysql("SELECT /* ?", &["z"]),
            "SELECT /* ?"
        );
        // 参数值里的 ? 不影响后续占位符
        assert_eq!(
            substitute_params_mysql("a = ? AND b = ?", &["?", "2"]),
            "a = '?' AND b = '2'"
        );
    }

    #[test]
    fn test_parse_properties() {
        let props = parse_properties("key1=val1;key2=val2");
        assert_eq!(props.get("key1").unwrap(), "val1");
        assert_eq!(props.get("key2").unwrap(), "val2");
        assert_eq!(props.len(), 2);
    }

    #[test]
    fn test_parse_properties_edge_cases() {
        assert!(parse_properties("").is_empty());
        assert!(parse_properties(";;;").is_empty());
        assert!(parse_properties("noequalsign").is_empty()); // 无 = 被忽略
        let props = parse_properties("  k1 = v1 ; k2=v2 ; badpair ");
        assert_eq!(props.get("k1").unwrap(), "v1"); // trim 键和值
        assert_eq!(props.get("k2").unwrap(), "v2");
        assert_eq!(props.len(), 2);
        // 值中含 = 只切第一个
        let props = parse_properties("a=b=c");
        assert_eq!(props.get("a").unwrap(), "b=c");
        // 重复键后者覆盖
        let props = parse_properties("a=1;a=2");
        assert_eq!(props.get("a").unwrap(), "2");
        assert_eq!(props.len(), 1);
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate_sql("hello world", 5), "hello...");
        assert_eq!(truncate_sql("hi", 10), "hi");
    }

    #[test]
    fn test_truncate_edge_cases() {
        assert_eq!(truncate_sql("", 5), "");
        assert_eq!(truncate_sql("abc", 0), "...");
        assert_eq!(truncate_sql("abc", 3), "abc"); // 恰好相等不截断
                                                   // 多字节：按字节判断截断，按字符取界（char_indices 保证不越界 panic）
        assert_eq!(truncate_sql("你好世界", 10), "你好世界...");
        assert_eq!(truncate_sql("你好", 2), "你好..."); // 2 个字符以内，字符数不受字节数限制
        assert_eq!(truncate_sql("你好", 6), "你好");
        // 截断点落在多字节字符中间 → 前移，不 panic
        let s = truncate_sql("abc你def", 5);
        assert!(s.ends_with("..."));
        assert!(s.starts_with("abc"));
        // 长文本
        let long = "x".repeat(1000);
        assert_eq!(truncate_sql(&long, 10), "xxxxxxxxxx...");
    }
}
