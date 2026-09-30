//! 脱敏对抗验证（`druid-core/src/redact.rs`）。
//!
//! 判定路径是真实的：`DruidConfig` 的 `Debug` 实现 —— 它是 `redact` 的唯一生产消费者
//! （`redact` 的入口是 `pub(crate)`，集成测试无法直接调用，也不该绕过真实调用点）。
//!
//! 目标口令 `S3CRET-D0-NOT-LEAK-7f21` 只要出现在 Debug 输出里即为泄漏。
//!
//! 三轮演进：Round 1 黑名单键名 → Round 2 白名单 + userinfo 整体打码 + URL 值递归
//! → Round 3（当前）**逐段判定**。Round 2 的四组泄漏同一根因：脱敏器在"以为已经
//! 安全"的地方放行了整段 —— 首键命中白名单就放行整串、`://` 之前不扫、
//! 未建模键名原样回显（键位置可能就是口令）、userinfo 只认字面量 `@`。
//! 现在：分隔符覆盖 `& ; ,` 与全部空白，每段独立判定；键名只有白名单/已知敏感标签
//! 才回显；没有 `=` 的裸文本在参数区一律打码；userinfo 判定用 `@`/`%40` 且只作用于
//! 不在参数值里的分隔符（`?email=a@b.com` 不再吞掉主机名）。
//! → Round 4（当前）：逐段判定被唯一一处例外破坏 —— `userinfo_at` 对「最后一个 `@`
//! 落在 `k=v` 值里」的豁免作用于**整串**，后段一个邮箱就能让前面的 `u:p@` 明文输出。
//! 见文件顶部 `finding_userinfo_masking_disabled_by_later_param_value`（`#[ignore]` 留证）。
//! 下方四组用例是第一轮 `#[ignore]` 留证的转正（原名 `finding_*`，现名 `masked_*`）。

use druid_core::DruidConfig;

/// 目标口令：任何输出里出现它都算泄漏
const SECRET: &str = "S3CRET-D0-NOT-LEAK-7f21";

/// 走真实路径：构造 DruidConfig 并取其 Debug 输出
fn debug_of(url: &str, props: &[&str]) -> String {
    let mut c = DruidConfig::new(url, "root", SECRET);
    c.connection_properties = props.iter().map(|s| s.to_string()).collect();
    format!("{:?}", c)
}

/// 返回泄漏明文的输入清单（空 = 全部被正确打码）。
/// 收集式而非断言式：一次运行就能拿到该组的**完整**泄漏列表，便于对照报告。
fn leaking_urls(urls: &[&str]) -> Vec<String> {
    urls.iter()
        .filter(|u| debug_of(u, &[]).contains(SECRET))
        .map(|u| u.to_string())
        .collect()
}

fn leaking_props(props: &[&str]) -> Vec<String> {
    props
        .iter()
        .filter(|p| debug_of("jdbc:mysql://h/db", &[p]).contains(SECRET))
        .map(|p| p.to_string())
        .collect()
}

// ══════════ Round 4（当前）：逐段判定的唯一边界 —— userinfo 豁免 ══════════

/// **Round 4 发现（本轮唯一，置顶留证）**：`userinfo_at` 的「这个 `@` 属于参数值」
/// 豁免会关掉**整条串**的 userinfo 打码 → 真正的凭据整段原样输出。
///
/// 机制（`druid-core/src/redact.rs:121` 与 `:158`）：`userinfo_at` 取最后一个
/// `@`/`%40`，回看它所在段是否形如 `k=v`（键不含 `:`）；是则返回 `None`，
/// `mask_rest` 据此**跳过 `mask_cred`** —— 位于串**开头**的 `user:pass@host` 一并放行。
/// 判定只看最后一个 `@` 所在的那一段，作用范围却是整串，与文件头「逐段判定」的原则不符。
///
/// 触发条件：连接串后段任意一个 `k=v` 参数的值里带 `@`（邮箱、`url=`、`a@b`）。
/// 真实性：中低（需要连接串恰好长这样），但一旦命中就是完整凭据明文进日志。
/// 修法方向（供参考，不在本轮范围）：userinfo 判定限定在 authority 段（第一个 `/` 之前），
/// 且 `k=v` 豁免只作用于**该段之后**的 `@`。
#[test]
#[ignore = "Round 4 发现：后段参数值里的 @ 关闭整串 userinfo 打码，凭据原样泄漏"]
fn finding_userinfo_masking_disabled_by_later_param_value() {
    let leaks = leaking_urls(&[
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=a@b.com",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?a=1&email=a@b.com",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db,email=a@b.com",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db;email=a@b.com",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=a%40b.com",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?url=jdbc:mysql://u:p@h2",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?x=a@b",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=a@",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=@b",
        // 无 `://` 形态（santize_thin）：凭据段落在「路径区」→ 裸文本保留 → 同样泄漏
        "root:S3CRET-D0-NOT-LEAK-7f21@h:3306?x=a@b",
    ]);
    let prop_leaks = leaking_props(&[
        "url=jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@h/db?email=a@b.com",
        "mysql://root:S3CRET-D0-NOT-LEAK-7f21@h/db?x=a@b",
    ]);
    assert!(
        leaks.is_empty() && prop_leaks.is_empty(),
        "userinfo 口令原样打印：\nURL  形态 {leaks:#?}\n属性形态 {prop_leaks:#?}"
    );
}

/// 对照：把后段那个 `@` 拿掉，同一前缀就正常打码 —— 证明泄漏由该豁免触发，
/// 而不是 userinfo 打码整体失效。
#[test]
fn masked_controls_next_to_the_userinfo_exemption() {
    let leaks = leaking_urls(&[
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?email=abc",
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21@host/db?a=1&b=2",
        "jdbc:mysql://root:Ab@S3CRET-D0-NOT-LEAK-7f21@host/db?email=abc",
    ]);
    assert!(leaks.is_empty(), "对照组也泄漏（比预期更糟）：{leaks:#?}");

    // 同一条串走**连接属性**路径（params_only=true）：凭据段没有 `=` → 按裸 token 打码，
    // 未被该豁免波及。两边行为不同，正是「authority 当路径放行」这一取舍的直接后果。
    assert!(
        leaking_props(&["root:S3CRET-D0-NOT-LEAK-7f21@h:3306?x=a@b"]).is_empty(),
        "连接属性路径也泄漏了（面比记录更大）"
    );
}

/// 声明的取舍（非缺陷，Round 4 复核，两侧都钉住）：
///   1. authority/路径里的**裸文本**可读 —— `jdbc:mysql://SECRET/db` 与
///      `jdbc:mysql://myhost/db` 文本上不可区分，保留主机名/库名才有排障价值。
///      承诺打码的只有 userinfo（`u:p@`）与 `k=v` 的值位置。
///   2. 白名单键（`user=`/`host=`/`Server=`…）的值按设计可读 —— 但值里出现
///      `@`/`%40` 或 `://` 时强制打码（凭据可能漏进了值）。
#[test]
fn declared_tradeoffs_are_bounded() {
    for url in [
        "jdbc:mysql://S3CRET-D0-NOT-LEAK-7f21/db",
        "jdbc:mysql://host/S3CRET-D0-NOT-LEAK-7f21",
    ] {
        let out = debug_of(url, &[]);
        assert!(
            out.contains("S3CRET-D0-NOT-LEAK-7f21"),
            "声明「裸 authority/路径可读」已被打破（现在是 {out}）——若是有意收紧请更新注释"
        );
    }
    let out = debug_of("jdbc:mysql://host/db", &["user=S3CRET-D0-NOT-LEAK-7f21"]);
    assert!(
        out.contains("user=S3CRET-D0-NOT-LEAK-7f21"),
        "声明「白名单键的值可读」已被打破（现在是 {out}）——若是有意收紧请更新注释"
    );

    // 白名单键的值里带 userinfo 标记 / URL：强制打码（这条是承诺，不是取舍）
    let leaks = leaking_urls(&[
        "jdbc:mysql://host/db?user=S3CRET-D0-NOT-LEAK-7f21@x",
        "jdbc:mysql://host/db?host=jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h2",
    ]);
    assert!(leaks.is_empty(), "白名单键的值未强制打码：{leaks:#?}");
    let leaks = leaking_props(&["Server=h;User Id=u;Host=S3CRET-D0-NOT-LEAK-7f21@x"]);
    assert!(leaks.is_empty(), "白名单键的值未强制打码：{leaks:#?}");
}

// ────────────── 必须打码（已生效，回归锚点） ──────────────

#[test]
fn masked_shapes_must_not_leak() {
    // userinfo（含口令里带各种分隔符、%3A 编码冒号、以及无 scheme 的 user:pass@host）
    assert_eq!(
        leaking_urls(&[
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
        ]),
        Vec::<String>::new(),
        "以上 userinfo 形态出现明文泄漏"
    );
    // 查询串 / 片段里的敏感键与未建模键
    assert_eq!(
        leaking_urls(&[
            "jdbc:mysql://host/db?password=S3CRET-D0-NOT-LEAK-7f21",
            "jdbc:mysql://host/db?credential=S3CRET-D0-NOT-LEAK-7f21",
            "jdbc:mysql://host/db#password=S3CRET-D0-NOT-LEAK-7f21",
            "jdbc:mysql://host/db#S3CRET-D0-NOT-LEAK-7f21",
            "jdbc:mysql:db?password=S3CRET-D0-NOT-LEAK-7f21",
        ]),
        Vec::<String>::new(),
        "以上查询串形态出现明文泄漏"
    );
    // 连接属性：k=v、ADO.NET/SQL Server 串、嵌套 URL
    assert_eq!(
        leaking_props(&[
            "password=S3CRET-D0-NOT-LEAK-7f21",
            "credential=S3CRET-D0-NOT-LEAK-7f21",
            "url=jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h/db",
            "jdbc:mysql://h/db?user=root&password=S3CRET-D0-NOT-LEAK-7f21",
            "Server=h;Database=db;User Id=u;Password=S3CRET-D0-NOT-LEAK-7f21",
            "Server=h;Uid=u;Pwd=S3CRET-D0-NOT-LEAK-7f21",
            "jdbcUrl=jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h2/db",
        ]),
        Vec::<String>::new(),
        "以上连接属性形态出现明文泄漏"
    );
}

/// 白名单键保留可读的运维信息（未被过度打码）
#[test]
fn whitelisted_keys_stay_readable() {
    let out = debug_of(
        "jdbc:mysql://h/db",
        &["user=root&port=3306", "charset=utf8mb4"],
    );
    assert!(out.contains("user=root"), "{out}");
    assert!(out.contains("port=3306"), "{out}");
    assert!(out.contains("charset=utf8mb4"), "{out}");
    let out = debug_of("jdbc:mysql://root:***@h/db", &[]);
    assert!(out.contains("root:***@h/db"), "{out}");
}

/// 过度打码已修复：`@` 只在其所在片段不是 `k=v` 值时才当 userinfo 分隔符，
/// 因此查询串里的邮箱不会再吞掉主机名/库名（值本身按未建模键打码）。
/// 旧行为是 `jdbc:mysql://host/db?email=a@b.com` → `jdbc:mysql://***@b.com`。
#[test]
fn at_in_query_value_keeps_host_readable() {
    let out = debug_of("jdbc:mysql://host/db?email=a@b.com", &[]);
    assert!(out.contains("jdbc:mysql://host/db?"), "{out}");
    assert!(!out.contains("a@b.com"), "{out}");
}

// ══════════════════ Round 3 转正：原四组泄漏现已全部打码 ══════════════════

/// 原泄漏 1（`finding_safe_key_first_pair_leaks_rest`）：
/// 旧实现只按 `&`/`;` 切段，首键命中白名单即整串 `push_str` 放行；
/// 现改为逐段判定 + 分隔符覆盖全部空白与 `,`/`|`。
#[test]
fn masked_pairs_across_all_separators() {
    let leaks = leaking_props(&[
        "user=root password=S3CRET-D0-NOT-LEAK-7f21",
        "host=localhost user=root password=S3CRET-D0-NOT-LEAK-7f21",
        "user=root\npassword=S3CRET-D0-NOT-LEAK-7f21",
        "user=root\tpassword=S3CRET-D0-NOT-LEAK-7f21",
        "user=root,password=S3CRET-D0-NOT-LEAK-7f21",
    ]);
    assert!(
        leaks.is_empty(),
        "以下连接属性把口令原样打印（首键在白名单 → 整串放行）：{leaks:#?}"
    );
    // 逐段判定不等于全打码：白名单段仍可读
    let out = debug_of("jdbc:mysql://h/db", &["user=root password=topsecret"]);
    assert!(out.contains("user=root"), "{out}");
    assert!(!out.contains("topsecret"), "{out}");
}

/// 原泄漏 2（`finding_pairs_before_scheme_are_not_masked`）：
/// 旧实现把 `://` 之前整段当 scheme 原样输出；现在 `k=<url>` 的键名与
/// 前缀里的 `k=v` 都要过逐段脱敏。
#[test]
fn masked_pairs_before_scheme() {
    let leaks = leaking_props(&[
        "password=S3CRET-D0-NOT-LEAK-7f21;url=jdbc:mysql://h/db",
        "password=S3CRET-D0-NOT-LEAK-7f21 url=jdbc:mysql://h/db",
        "user=root;password=S3CRET-D0-NOT-LEAK-7f21;jdbc:mysql://h/db",
    ]);
    assert!(
        leaks.is_empty(),
        "以下连接属性在 `://` 之前泄漏口令：{leaks:#?}"
    );
}

/// 原泄漏 3（`finding_bare_token_and_key_position_leaks`）：
/// 参数区（首个 `?`/`#`/`;` 之后）里没有 `=` 的裸文本一律打码；
/// 未建模键名不再回显（`***=***`），`?junk/user=` 也不能靠路径尾伪装成白名单键。
#[test]
fn masked_bare_token_and_key_position() {
    let leaks = leaking_urls(&[
        "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21",
        "jdbc:mysql://host/db;S3CRET-D0-NOT-LEAK-7f21",
        "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21=x",
        "jdbc:mysql://host/db#S3CRET-D0-NOT-LEAK-7f21=x",
        "jdbc:mysql://host/db?junk/user=S3CRET-D0-NOT-LEAK-7f21",
    ]);
    assert!(leaks.is_empty(), "以下 URL 形态把口令原样打印：{leaks:#?}");
}

/// 原泄漏 4（`finding_userinfo_without_literal_at`，真实性较低）：
/// userinfo 分隔符同时认 `@` 与 `%40`；无 scheme 的 `口令@host` 走无 `://` 分支，
/// `@` 前一律按凭据处理。
#[test]
fn masked_userinfo_without_literal_at() {
    let leaks = leaking_urls(&[
        "jdbc:mysql://root:S3CRET-D0-NOT-LEAK-7f21%40host/db",
        "S3CRET-D0-NOT-LEAK-7f21@host:3306",
    ]);
    assert!(leaks.is_empty(), "以下 URL 形态把口令原样打印：{leaks:#?}");
}

/// 组合形态自检：把上面几组的元素交叉拼接（分隔符 × 区域 × 键名位置），
/// 确保是"逐段判定"在兜底，而不是逐条补丁碰巧盖住。
#[test]
fn masked_combined_shapes_selfcheck() {
    let leaks = leaking_urls(&[
        // 口令里带 `;`/`#`/`%40` 的 userinfo
        "jdbc:mysql://root:Ab;S3CRET-D0-NOT-LEAK-7f21@host/db",
        "jdbc:mysql://root:Ab%40S3CRET-D0-NOT-LEAK-7f21@host/db",
        // 查询值里再带 `#` 与裸 token
        "jdbc:mysql://h/db?x=a#S3CRET-D0-NOT-LEAK-7f21",
        "jdbc:mysql://h/db?S3CRET-D0-NOT-LEAK-7f21&user=root",
        // 空白分隔的 URL 参数
        "jdbc:mysql://h/db?user=root password=S3CRET-D0-NOT-LEAK-7f21",
        // userinfo 与查询串同时带口令
        "jdbc:mysql://u:S3CRET-D0-NOT-LEAK-7f21@h/db?pwd=S3CRET-D0-NOT-LEAK-7f21",
    ]);
    assert!(leaks.is_empty(), "组合形态泄漏：{leaks:#?}");

    let leaks = leaking_props(&[
        // 未建模键 + 空格分隔 + 后接 URL
        "password=S3CRET-D0-NOT-LEAK-7f21;jdbc:mysql://h/db",
        "x=S3CRET-D0-NOT-LEAK-7f21",
        "credential=S3CRET-D0-NOT-LEAK-7f21 autoReconnect=true",
    ]);
    assert!(leaks.is_empty(), "组合属性泄漏：{leaks:#?}");
}
