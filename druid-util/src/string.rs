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
            if i > 0 && (!chars[i - 1].is_uppercase() ||
                (i + 1 < chars.len() && chars[i + 1].is_lowercase())) {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap_or(c));
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

/// 简单 SQL 参数替换（? 占位符）
///
/// 使用占位符标记避免二次替换：参数值中的 ? 不会干扰替换。
pub fn substitute_params(sql: &str, params: &[&str]) -> String {
    const MARKER: &str = "\x00PARAM\x00";
    let mut result = String::with_capacity(sql.len() + params.len() * 8);
    let mut param_idx = 0;
    for ch in sql.chars() {
        if ch == '?' && param_idx < params.len() {
            result.push_str(MARKER);
            result.push_str(&param_idx.to_string());
            result.push_str(MARKER);
            param_idx += 1;
        } else {
            result.push(ch);
        }
    }
    for (i, param) in params.iter().enumerate().take(param_idx) {
        let quoted = format!("'{}'", param.replace('\'', "''"));
        let placeholder = format!("{}{}{}", MARKER, i, MARKER);
        result = result.replace(&placeholder, &quoted);
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
    fn test_snake_to_camel() {
        assert_eq!(snake_to_camel("druid_data_source"), "druidDataSource");
        assert_eq!(snake_to_camel("max_active"), "maxActive");
    }

    #[test]
    fn test_substitute_params() {
        let sql = "SELECT * FROM users WHERE id = ? AND name = ?";
        let result = substitute_params(sql, &["1", "Alice"]);
        assert_eq!(
            result,
            "SELECT * FROM users WHERE id = '1' AND name = 'Alice'"
        );
    }

    #[test]
    fn test_parse_properties() {
        let props = parse_properties("key1=val1;key2=val2");
        assert_eq!(props.get("key1").unwrap(), "val1");
        assert_eq!(props.get("key2").unwrap(), "val2");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate_sql("hello world", 5), "hello...");
        assert_eq!(truncate_sql("hi", 10), "hi");
    }
}
