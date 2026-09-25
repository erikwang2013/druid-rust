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

/// 简单 SQL 参数替换（? 占位符）
///
/// 单次遍历完成替换，参数值中的 ? 不会干扰后续替换。
pub fn substitute_params(sql: &str, params: &[&str]) -> String {
    let escaped: Vec<String> = params
        .iter()
        .map(|p| format!("'{}'", p.replace('\'', "''")))
        .collect();
    let mut result = String::with_capacity(sql.len() + params.len() * 8);
    let mut param_idx = 0;
    for ch in sql.chars() {
        if ch == '?' && param_idx < escaped.len() {
            result.push_str(&escaped[param_idx]);
            param_idx += 1;
        } else {
            result.push(ch);
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
    fn test_substitute_params() {
        let sql = "SELECT * FROM users WHERE id = ? AND name = ?";
        let result = substitute_params(sql, &["1", "Alice"]);
        assert_eq!(
            result,
            "SELECT * FROM users WHERE id = '1' AND name = 'Alice'"
        );
    }

    #[test]
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
    fn test_substitute_params_quote_escape() {
        // 单引号翻倍转义，防 SQL 注入
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
