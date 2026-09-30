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

/// 已知敏感的键名子串（大小写不敏感）：命中即打码**值**。
/// 回显键名另走 `SENSITIVE_KEY_LABELS` 精确匹配 —— 子串判定不足以证明键名是标签。
const SENSITIVE_KEY_HINTS: [&str; 4] = ["pass", "pwd", "secret", "token"];

/// 已知安全的 scheme 标签（空格分隔，小写）：按 `:` 拆开后**逐段**比对，
/// `jdbc:oracle:thin` 三段都要在表里。覆盖 `druid-util::sql::detect_db_type_from_url`
/// 认识的驱动，外加 http(s)。与键名同一套 fail-closed 约定 —— scheme 位置放着口令时
/// 与 `mysql`/`jdbc` 同形，认不出来就打码（未知驱动打码后 host/库名仍可读；确认安全再加一个词）。
///
/// 必须**精确**匹配：`detect_db_type_from_url` 用子串识别驱动（便于使用），照搬会放行
/// `<口令>mysql://h` 这类输入。
const SAFE_SCHEME_LABELS: &str = "jdbc mysql mariadb postgres postgresql oracle thin oci \
    sqlserver mssql db2 h2 clickhouse doris starrocks hive hive2 presto impala snowflake \
    bigquery redshift spark phoenix teradata informix informix-sqli athena gaussdb dameng \
    dm odps maxcompute hologres sqlite redis mongodb mongo http https";

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

/// scheme 段是否可回显：`:` 拆开后**每一段**都要在白名单里。
/// 只看第一段会放过 `jdbc:SECRET` —— 判定范围必须覆盖整个 scheme 段，而不是它的一段。
fn is_safe_scheme(scheme: &str) -> bool {
    scheme.split(':').all(|l| {
        let l = l.to_ascii_lowercase();
        SAFE_SCHEME_LABELS.split(' ').any(|s| s == l)
    })
}

/// 可回显的敏感键名（空格分隔，小写**精确**匹配）：只决定**标签**能不能回显。
/// 子串启发式（`SENSITIVE_KEY_HINTS`）只用来判断「值要不要打码」—— 它匹配的是任意文本，
/// 拿它决定回显等于把键名位置的口令原样放行（`<口令>pass=x` 的键名整段是口令）。
const SENSITIVE_KEY_LABELS: &str = "password passwd pwd pass secret token accesstoken access_token";

/// 键名是否**精确**等于已知敏感标签（大小写不敏感）
fn is_sensitive_label(key: &str) -> bool {
    let k = key.trim().to_ascii_lowercase();
    SENSITIVE_KEY_LABELS.split(' ').any(|s| s == k)
}

/// 键名标签：白名单与精确已知的敏感键可回显，其余 `***`（键位置可能就是口令本身）
fn key_label(key: &str) -> &str {
    let k = key.trim();
    if is_safe_key(k) || is_sensitive_label(k) {
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

/// 找 userinfo 分隔符：从右往左，第一个**不属于参数值**的 `@` 或 `%40`。
///
/// 逐个候选回看，而不是只看最后一个：`?email=a@b.com` 的 `@` 落在 `k=v` 值里，
/// 该豁免只作用于**它自己** —— 不能因此关掉整串的判定，否则它前面的
/// `root:pass@host` 会被当成"已验证安全"整段放行。判定范围必须等于作用范围，
/// 这是「逐段判定」在本模块的最后一块拼图。
///
/// `?x=a:SECRET@h` 这类 key 里带冒号的仍按凭据处理 —— 宁可多打。
fn userinfo_at(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    for i in (0..b.len()).rev() {
        let enc = i + 3 <= b.len() && b[i..i + 3].eq_ignore_ascii_case(b"%40");
        if (b[i] == b'@' || enc) && !is_param_value_at(s, i) {
            return Some(i);
        }
    }
    None
}

/// `at` 处的 `@`/`%40` 是否落在参数区的 `k=v` 值里（`?email=a@b.com` 属于值，不是凭据）
fn is_param_value_at(s: &str, at: usize) -> bool {
    let head = &s[..at];
    let seg_start = head.rfind(['?', '#', ';', '&', ',']).map_or(0, |p| p + 1);
    head[seg_start..]
        .trim()
        .split_once('=')
        .is_some_and(|(k, _)| !k.contains(':'))
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
    // scheme 段按白名单判定：`k=SECRET://host` 里 `://` 之前的那截就是口令本身
    let scheme = &url[head..i];
    let scheme = if is_safe_scheme(scheme) {
        scheme
    } else {
        "***"
    };
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
        // 键名同样要判定：精确已知才回显（`<口令>pass` 这类只能匹配到子串，整段打码）
        return format!("{}=***", key_label(k));
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
                // 后段参数值里再带 `@`：豁免只作用于那个 `@`，不许关掉整串判定
                "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=a@b.com",
                "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?url=jdbc:mysql://u:p@h2",
                "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db;a=1&email=a@b.com",
                "root:S3CRET-D0-NOT-LEAK-7f21@h:3306?x=a@b",
                "url=jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@h/db?email=a@b.com",
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
                // 键名位置的口令：即使只匹配到敏感子串（pass/secret/token）也整段打码
                "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21pass=x",
                "jdbc:mysql://host/db?passwordS3CRET-D0-NOT-LEAK-7f21=x",
            ],
            sanitize_url,
        );
        assert_eq!(
            sanitize_url("jdbc:mysql://host/db?credential=S3CRET"),
            "jdbc:mysql://host/db?***=***"
        );
        // 非精确的敏感键名（含子串但非已知标签）连键名一起打码 —— 代价是不再显示键名
        assert_eq!(
            sanitize_url("jdbc:mysql://host/db?mydb_password=x"),
            "jdbc:mysql://host/db?***=***"
        );
    }

    /// scheme 位置与口令同形：只认白名单，`jdbc:mysql` 保留，未知标签整段打码
    #[test]
    fn scheme_position_is_masked_unless_known() {
        assert_masked(
            &[
                "S3CRET-D0-NOT-LEAK-7f21://host/db",
                "jdbc:S3CRET-D0-NOT-LEAK-7f21://host/db",
                "jdbc:mysql:S3CRET-D0-NOT-LEAK-7f21://host/db",
            ],
            sanitize_value,
        );
        assert_eq!(sanitize_url("S3CRET://host/db"), "***://host/db");
        // 白名单 scheme 原样保留（脱敏不能退化成全打码）
        assert_eq!(sanitize_url("https://host/x"), "https://host/x");
        assert_eq!(
            sanitize_url("jdbc:sqlserver://h/db"),
            "jdbc:sqlserver://h/db"
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
