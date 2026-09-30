//! 调试输出脱敏（fail-closed）
//!
//! 唯一原则：**逐段判定，任何判断不了的片段一律打码**。不因为"前一段看着安全"
//! 就放行后文（首键在白名单 ≠ 整串安全），也不回显不确定的键名
//! （键名位置同样可能放着口令）。详见 `druid-core/tests/redact_adversarial.rs`。

/// 已知安全的参数名（小写精确匹配）：值是连接/驱动选项，不是凭据。
/// fail-closed 约定 —— 不在表里的键连键名一起打码，确认安全再加。
const SAFE_PARAM_KEYS: &[&str] = &[
    // 账号/库名（用户名本身不是口令）
    "user",
    "username",
    "uid",
    "user id",
    "database",
    "databasename",
    "dbname",
    "schema",
    "currentschema",
    // 地址/端口
    "host",
    "port",
    "portnumber",
    "servername",
    "server",
    "instancename",
    "applicationname",
    "data source",
    "initial catalog",
    // 超时/池大小
    "connecttimeout",
    "connectiontimeout",
    "sockettimeout",
    "logintimeout",
    "commandtimeout",
    "maxpoolsize",
    "minpoolsize",
    "maxidle",
    "minidle",
    "pooling",
    "multipleactiveresultsets",
    // SSL 协商开关（值是布尔/模式）
    "ssl",
    "usessl",
    "sslmode",
    "requiressl",
    "verifyservercertificate",
    "trustservercertificate",
    "integrated security",
    "encrypt",
    "allowpublickeyretrieval",
    // 字符集/时区
    "useunicode",
    "characterencoding",
    "charset",
    "encoding",
    "collation",
    "servertimezone",
    "timezone",
    // 驱动行为开关
    "autoreconnect",
    "autoreconnectforpools",
    "rewritebatchedstatements",
    "useserverprepstmts",
    "cacheprepstmts",
    "useaffectedrows",
    "allowmultiqueries",
    "allowloadlocalinfile",
    "zerodatetimebehavior",
    "tinyint1isbit",
    "usecompression",
    "usecursorfetch",
    "defaultfetchsize",
    "useinformationschema",
];

/// 已知敏感的键名子串（大小写不敏感）：命中即打码值。键名本身是已知标签，保留便于排障。
const SENSITIVE_KEY_HINTS: [&str; 4] = ["pass", "pwd", "secret", "token"];

/// 段分隔符：`&`/`;`/`,`/`|` 与所有空白（URL、ADO.NET、MySQL 选项文件、libmysql DSN）
fn is_sep(c: char) -> bool {
    matches!(c, '&' | ';' | ',' | '|') || c.is_whitespace()
}

/// 参数区标记：出现过之后，没带 `=` 的裸文本语义不明（可能是裸 token）→ 打码
fn is_region_mark(c: char) -> bool {
    matches!(c, '?' | '#' | ';')
}

fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SENSITIVE_KEY_HINTS.iter().any(|h| k.contains(h))
}

fn is_safe_key(key: &str) -> bool {
    SAFE_PARAM_KEYS.contains(&key.trim().to_ascii_lowercase().as_str())
}

/// 键名标签：白名单与已知敏感键可回显，其余 `***`（键位置可能就是口令本身）
fn key_label(key: &str) -> &str {
    let k = key.trim();
    if is_safe_key(k) || is_sensitive_key(k) {
        k
    } else {
        "***"
    }
}

/// 值里是否出现 userinfo 分隔符（`@` 或 `%40`）：可能是凭据漏进了值里
fn has_userinfo_mark(v: &str) -> bool {
    v.contains('@')
        || v.as_bytes()
            .windows(3)
            .any(|w| w.eq_ignore_ascii_case(b"%40"))
}

/// 找 userinfo 分隔符：最后一个 `@` 或 `%40`，且它**不在 `k=v` 的值里**。
///
/// `?email=a@b.com` 的 `@` 属于参数值（不该触发 userinfo 打码，否则主机名被吞）；
/// `?x=a:SECRET@h` 这类 key 里带冒号的仍按凭据处理 —— 宁可多打。
fn userinfo_at(s: &str) -> Option<usize> {
    let enc = s
        .as_bytes()
        .windows(3)
        .rposition(|w| w.eq_ignore_ascii_case(b"%40"));
    let at = match (s.rfind('@'), enc) {
        (Some(a), Some(e)) => Some(a.max(e)),
        (a, e) => a.or(e),
    }?;
    let head = &s[..at];
    let seg_start = head.rfind(['?', '#', ';', '&', ',']).map_or(0, |p| p + 1);
    let is_value = head[seg_start..]
        .trim()
        .split_once('=')
        .is_some_and(|(k, _)| !k.contains(':'));
    if is_value {
        None
    } else {
        Some(at)
    }
}

/// 凭据部分打码：`@` 之前一律按凭据处理。
/// 仅当冒号出现在任何其它分隔符之前时保留 `user:` 前缀（那是用户名）。
fn mask_cred(cred: &str, host: &str) -> String {
    let colon = cred.find(':').filter(|&c| {
        c < cred
            .find(['/', '?', '#', '&', ';', ',', '='])
            .unwrap_or(usize::MAX)
    });
    match colon {
        Some(c) => format!("{}***{}", &cred[..=c], host),
        None => format!("***{host}"),
    }
}

/// `://` 之后的正文：先按 userinfo 规则打码，再逐段处理
fn mask_rest(rest: &str) -> String {
    let masked = match userinfo_at(rest) {
        Some(at) => mask_cred(&rest[..at], &rest[at..]),
        None => rest.to_string(),
    };
    sanitize_segments(&masked, false)
}

/// `://` 之前的键名部分：`k=<url>` 形式的键名同样不可回显。
/// 返回（脱敏后的前缀, scheme 起点）
fn split_lead(before: &str) -> (String, usize) {
    let lead_end = before
        .rfind(|c: char| is_sep(c) || is_region_mark(c) || c == '=')
        .map_or(0, |p| p + 1);
    let lead = &before[..lead_end];
    let Some(key_tail) = lead.strip_suffix('=') else {
        return (sanitize_segments(lead, true), lead_end);
    };
    let key_start = key_tail
        .rfind(|c: char| is_sep(c) || is_region_mark(c))
        .map_or(0, |p| p + 1);
    let prefix = format!(
        "{}{}=",
        sanitize_segments(&key_tail[..key_start], true),
        key_label(&key_tail[key_start..])
    );
    (prefix, lead_end)
}

/// URL 脱敏：`k=<url>` 前缀、userinfo、逐段键值对，以及无 `://` 的
/// Oracle thin（`jdbc:oracle:thin:user/pass@host`）与裸 `user:pass@host`
pub(crate) fn sanitize_url(url: &str) -> String {
    let Some(i) = url.find("://") else {
        return sanitize_thin(url, false);
    };
    let (lead, head) = split_lead(&url[..i]);
    let scheme = &url[head..i];
    format!("{lead}{scheme}://{}", mask_rest(&url[i + 3..]))
}

/// 无 `://`：`jdbc:oracle:thin:u/p@h`、`root:pass@host:3306`、纯键值串
fn sanitize_thin(s: &str, params_only: bool) -> String {
    let Some(at) = userinfo_at(s) else {
        return sanitize_segments(s, params_only);
    };
    let (cred, host) = s.split_at(at);
    // `user/pass@host`：第一个 `/` 之后整段是口令（口令本身可能含 `/`），用户名保留
    let masked = match cred.find('/') {
        Some(slash) => format!("{}/***{}", &cred[..slash], host),
        None => mask_cred(cred, host),
    };
    sanitize_segments(&masked, params_only)
}

/// 对外入口：一条连接属性整体脱敏（连接属性没有 URL 的路径段，裸文本一律打码）
pub(crate) fn sanitize_value(s: &str) -> String {
    if s.contains("://") {
        sanitize_url(s)
    } else {
        sanitize_thin(s, true)
    }
}

/// 逐段脱敏：每段独立判定。`params_only` 为 true 时整串都算参数区（连接属性）；
/// 否则参数区标记（`?`/`#`/`;`）之前的裸段是 URL 的 host/路径，保留。
fn sanitize_segments(s: &str, params_only: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = 0;
    let mut in_params = params_only;
    for (i, c) in s.char_indices() {
        if !is_sep(c) && !is_region_mark(c) {
            continue;
        }
        out.push_str(&sanitize_segment(&s[start..i], in_params));
        if is_region_mark(c) {
            in_params = true;
        }
        out.push(c);
        start = i + c.len_utf8();
    }
    out.push_str(&sanitize_segment(&s[start..], in_params));
    out
}

/// 单段：`k=v` 按键名规则；裸段按区域规则
fn sanitize_segment(seg: &str, in_params: bool) -> String {
    if seg.is_empty() {
        return String::new(); // 连续分隔符之间没有内容，别凭空打码
    }
    let Some((k, v)) = seg.split_once('=') else {
        // 裸文本：参数区里语义不明（可能是裸 token）→ 打码；路径段保留
        return if in_params { "***".into() } else { seg.into() };
    };
    if is_sensitive_key(k) {
        return format!("{k}=***");
    }
    if is_safe_key(k) {
        if v.contains("://") {
            return format!("{k}={}", sanitize_url(v));
        }
        // 值里出现 userinfo 分隔符：可能是凭据漏下来了 → 只留键名
        return if has_userinfo_mark(v) {
            format!("{k}=***")
        } else {
            seg.into()
        };
    }
    // 未建模键：键名不回显，值里的 URL 递归脱敏后保留（便于排障），其余打码
    if v.contains("://") {
        format!("***={}", sanitize_url(v))
    } else {
        "***=***".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 任何输出里出现它都算泄漏（与 tests/redact_adversarial.rs 同一个哨兵）
    const SECRET: &str = "S3CRET-D0-NOT-LEAK-7f21";

    fn assert_masked(inputs: &[&str], f: fn(&str) -> String) {
        for s in inputs {
            let out = f(s);
            assert!(!out.contains(SECRET), "明文泄漏: {s} -> {out}");
        }
    }

    /// userinfo：口令里含任何分隔符都整段在 `@` 之前打码
    #[test]
    fn userinfo_is_masked_in_every_shape() {
        assert_masked(
            &[
                "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root%3AS3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab/S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab?S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab#S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab=S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab&S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:mysql://root:Ab@S3CRET-D0-NOT-LEAK-7f21@host/db",
                "jdbc:oracle:thin:scott/S3CRET-D0-NOT-LEAK-7f21/x@host:1521:ORCL",
                "root:S3CRET-D0-NOT-LEAK-7f21@host:3306",
                // 无字面量 `@`：%40 编码、无冒号的 `口令@host`
                "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21%40host/db",
                "S3CRET-D0-NOT-LEAK-7f21@host:3306",
            ],
            sanitize_url,
        );
        assert_eq!(
            sanitize_url("jdbc:mysql://u:p@h/db"),
            "jdbc:mysql://u:***@h/db"
        );
        assert_eq!(
            sanitize_url("jdbc:oracle:thin:scott/tiger@h:1521/ORCL"),
            "jdbc:oracle:thin:scott/***@h:1521/ORCL"
        );
    }

    /// 逐段判定：分隔符覆盖空白/换行/逗号，首键在白名单也不放行整串
    #[test]
    fn pairs_are_judged_per_segment() {
        assert_masked(
            &[
                "user=root password=S3CRET-D0-NOT-LEAK-7f21",
                "host=localhost user=root password=S3CRET-D0-NOT-LEAK-7f21",
                "user=root\npassword=S3CRET-D0-NOT-LEAK-7f21",
                "user=root\tpassword=S3CRET-D0-NOT-LEAK-7f21",
                "user=root,password=S3CRET-D0-NOT-LEAK-7f21",
                "password=S3CRET-D0-NOT-LEAK-7f21;url=jdbc:mysql://h/db",
                "password=S3CRET-D0-NOT-LEAK-7f21 url=jdbc:mysql://h/db",
                "user=root;password=S3CRET-D0-NOT-LEAK-7f21;jdbc:mysql://h/db",
            ],
            sanitize_value,
        );
        assert_eq!(
            sanitize_value("user=root password=topsecret"),
            "user=root password=***"
        );
    }

    /// 裸 token 与键名位置：`?SECRET`、`?SECRET=x` 连键名一起打码
    #[test]
    fn bare_token_and_key_position_are_masked() {
        assert_masked(
            &[
                "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21",
                "jdbc:mysql://host/db;S3CRET-D0-NOT-LEAK-7f21",
                "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21=x",
                "jdbc:mysql://host/db#S3CRET-D0-NOT-LEAK-7f21=x",
                "jdbc:mysql://host/db?junk/user=S3CRET-D0-NOT-LEAK-7f21",
            ],
            sanitize_url,
        );
        assert_eq!(
            sanitize_url("jdbc:mysql://host/db?credential=S3CRET"),
            "jdbc:mysql://host/db?***=***"
        );
    }

    /// 参数值里的 `@`（如邮箱）不触发 userinfo：主机名/库名必须留下
    #[test]
    fn at_in_query_value_keeps_host() {
        let out = sanitize_url("jdbc:mysql://host/db?email=a@b.com");
        assert!(out.contains("jdbc:mysql://host/db?"), "{out}");
        assert!(!out.contains("a@b.com"), "{out}");
    }

    /// 未建模键的 URL 值仍递归脱敏后保留，便于排障
    #[test]
    fn nested_url_values_are_recursively_masked() {
        assert_masked(
            &[
                "jdbc:mysql://host/db?url=jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h2/db",
                "jdbcUrl=jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h2/db",
            ],
            sanitize_value,
        );
        assert_eq!(
            sanitize_value("jdbcUrl=jdbc:mysql://u:p@h/db"),
            "***=jdbc:mysql://u:***@h/db"
        );
    }

    /// 白名单键保留可读信息（脱敏不能退化成全打码）
    #[test]
    fn whitelisted_keys_stay_readable() {
        assert_eq!(
            sanitize_value("user=root&port=3306&useSSL=false"),
            "user=root&port=3306&useSSL=false"
        );
        assert_eq!(
            sanitize_value("Server=h;Database=db;Uid=u"),
            "Server=h;Database=db;Uid=u"
        );
        assert_eq!(
            sanitize_url("jdbc:mysql://h:3306/db"),
            "jdbc:mysql://h:3306/db"
        );
        assert_eq!(
            sanitize_url("jdbc:mysql://root:***@h/db"),
            "jdbc:mysql://root:***@h/db"
        );
    }
}
