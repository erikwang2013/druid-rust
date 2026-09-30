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
//! → Round 4：逐段判定被唯一一处例外破坏 —— `userinfo_at` 对「最后一个 `@` 落在
//! `k=v` 值里」的豁免作用于**整串**，后段一个邮箱就能让前面的 `u:p@` 明文输出。
//! **已修复**：`userinfo_at` 改为从右往左取第一个**非**参数值里的 `@`/`%40`，
//! 豁免只作用于被豁免的那个分隔符本身，判定范围 = 作用范围。
//! 回归用例见 `masked_userinfo_beside_param_value`（原 `finding_*`），对照组
//! `masked_controls_next_to_the_userinfo_exemption` 保持绿。
//! → Round 5：两段**从未被判定过**却原样输出的文本。
//!   (a) `://` 之前的 scheme 段被当成「结构性位置」——它其实可判定
//!   （`k=jdbc:mysql://h` 与 `k=<口令>://h` 只在内容上不同）；
//!   (b) 敏感键名的回显走子串匹配，子串能匹配任意文本 → `<口令>pass=x` 键名整段打印。
//! **均已修复**：`SAFE_SCHEME_LABELS` / `SENSITIVE_KEY_LABELS` 精确白名单
//! （scheme 还要 `:` 逐段比对），未知的一律打码。scheme 因此退出
//! `declared_tradeoffs_are_bounded` 的取舍，回到「承诺打码」。
//! 回归用例：`masked_scheme_position`、`masked_key_label_position`。
//! 至此同源缺陷的全部三次露头（Round 3 首键放行整串、Round 4 一个 `@` 放行整串、
//! Round 5 未判定的 scheme 段/键名段）都收敛为
//! 「**判定范围 = 作用范围，每一段都要判定**」；剩下的原样输出只有
//! `declared_tradeoffs_are_bounded` 里钉住的用户名/host/库名三个位置与白名单键值。
//!
//! 本文件当前没有被忽略的用例：所有历史发现要么已修复转正（`masked_*` 回归锚点），
//! 要么是 `declared_tradeoffs_are_bounded` 里显式钉住的已声明限制。

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

// ══════════ Round 4：userinfo 豁免的作用范围（已修复） ══════════

/// **Round 4 发现（已修复）**：`userinfo_at` 的「这个 `@` 属于参数值」豁免会关掉
/// **整条串**的 userinfo 打码 → 真正的凭据整段原样输出。
///
/// 机制（旧实现）：`userinfo_at` 只取**最后一个** `@`/`%40`，回看它所在段是否形如
/// `k=v`（键不含 `:`）；是则返回 `None`，`mask_rest` 据此**跳过 `mask_cred`** ——
/// 位于串开头的 `user:pass@host` 一并放行。判定只看最后一个 `@` 所在的那一段，
/// 作用范围却是整串，与「逐段判定」的原则不符（与 Round 3 的「首键放行整串」同源）。
///
/// 修法：从右往左取第一个**非**参数值里的 `@`/`%40`，豁免只作用于被豁免的那个分隔符
/// 本身。对照组 `masked_controls_next_to_the_userinfo_exemption` 证明：移除后段那个
/// `@` 后行为不变，本组泄漏确由该豁免触发。
#[test]
fn masked_userinfo_beside_param_value() {
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

// ══════════ Round 5：scheme 位置（可判定 → fail-closed，已修复） ══════════

/// **Round 5 发现（已修复）**：`sanitize_url` 原样输出 `://` 之前、最后一个分隔符/`=`
/// 之后的那截（"scheme"）。它不是结构性的：`k=SECRET://host` 里 `://` 之前就是口令，
/// 且与 `k=jdbc:mysql://host` 只在内容上不同 —— 判定得了，就不该按「结构性位置」放行。
///
/// 旧输出：`jdbcUrl=<口令>://host/db` → `***=<口令>://host/db`。
/// 修法：`is_safe_scheme` 按 `:` **逐段**比对 `SAFE_SCHEME_LABELS`，未知标签整段打码。
/// 逐段而非只看首段：`jdbc:SECRET://h` 不能因为 `jdbc` 在白名单里就放行整个 scheme 段
/// （与 Round 3「首键放行整串」、Round 4「一个 `@` 放行整串」同一根因的第三次露头）。
#[test]
fn masked_scheme_position() {
    let leaks = leaking_urls(&[
        "S3CRET-D0-NOT-LEAK-7f21://host/db",
        "jdbc:S3CRET-D0-NOT-LEAK-7f21://host/db",
        "jdbc:mysql:S3CRET-D0-NOT-LEAK-7f21://host/db",
        "jdbc:mysql://host/db?url=S3CRET-D0-NOT-LEAK-7f21://h2/db",
    ]);
    let prop_leaks = leaking_props(&[
        "jdbcUrl=S3CRET-D0-NOT-LEAK-7f21://host/db",
        "url=S3CRET-D0-NOT-LEAK-7f21://host/db",
    ]);
    assert!(
        leaks.is_empty() && prop_leaks.is_empty(),
        "scheme 位置口令原样打印：\nURL 形态 {leaks:#?}\n属性形态 {prop_leaks:#?}"
    );

    // 已知 scheme 必须原样保留（白名单不能退化成把 scheme 也打掉）
    for (url, want) in [
        ("jdbc:mysql://h:3306/db", "jdbc:mysql://h:3306/db"),
        ("jdbc:sqlserver://h/db", "jdbc:sqlserver://h/db"),
        ("https://h/x", "https://h/x"),
        ("jdbc:mysql://root:***@h/db", "jdbc:mysql://root:***@h/db"),
    ] {
        assert!(debug_of(url, &[]).contains(want), "{url} → 期望包含 {want}");
    }
}

/// **Round 5 发现（已修复，同轮第二个面）**：敏感键名的回显走的是**子串**匹配
/// （`SENSITIVE_KEY_HINTS`），而子串能匹配任意文本 —— `<口令>pass=x` 的键名整段就是口令，
/// 却因为含 "pass" 被当成"已知标签"原样打印（与 Round 3「未知键名不回显」同一个面，
/// 只是被子串启发式绕开了）。
///
/// 修法：判断「值要不要打码」仍用子串启发式（方向是 fail-closed，多打没错）；
/// 判断「键名能不能回显」改为**精确**白名单 `SENSITIVE_KEY_LABELS` —— 与
/// `SAFE_PARAM_KEYS` 同一套约定：认不出来就打码。代价是非精确的敏感键名
/// （`mydb_password=xxx`）不再显示键名；值本来就打码，可以接受。
#[test]
fn masked_key_label_position() {
    let leaks = leaking_urls(&[
        "jdbc:mysql://host/db?S3CRET-D0-NOT-LEAK-7f21pass=x",
        "jdbc:mysql://host/db?passwordS3CRET-D0-NOT-LEAK-7f21=x",
        "jdbc:mysql://host/db;S3CRET-D0-NOT-LEAK-7f21token=x",
    ]);
    let prop_leaks = leaking_props(&[
        "S3CRET-D0-NOT-LEAK-7f21pass=x",
        "mydb_password=S3CRET-D0-NOT-LEAK-7f21",
    ]);
    assert!(
        leaks.is_empty() && prop_leaks.is_empty(),
        "键名位置口令原样打印：\nURL 形态 {leaks:#?}\n属性形态 {prop_leaks:#?}"
    );
    // 精确已知的敏感标签仍回显（排障要知道"哪个键被打码了"）
    assert!(
        debug_of("jdbc:mysql://host/db?password=x", &[]).contains("?password=***"),
        "已知敏感标签被过度打码"
    );
}

/// 声明的已知限制（**不是**未修缺陷，两侧都钉住）：
///   1. **结构性位置**（用户名、host、库名）按 URI 语义可读。它们与任意 token
///      在文本上不可区分（`jdbc:mysql://SECRET/db` 与 `jdbc:mysql://myhost/db` 同形，
///      `SECRET:x@h` 与 `root:x@h` 同形），要打码就得连主机名/库名/用户名一起打掉 ——
///      而「用户名保留」正是本文件锚点 `root:***@h/db` 明确要求的。可接受的代价：
///      正常配置里口令不会放在这些位置；一旦放了，本用例会红，提醒更新注释而非静默漂移。
///      **承诺打码的只有口令位置**：userinfo 冒号之后、`k=v` 的值位置。
///   2. 白名单键（`user=`/`host=`/`Server=`…）的值按设计可读 —— 但值里出现
///      `@`/`%40` 或 `://` 时强制打码（凭据可能漏进了值）。
///   3. （Round 5）scheme 位置**不在**上面的取舍里：它与 `k=v` 的键名同源，可以判定，
///      所以按 `SAFE_SCHEME_LABELS` fail-closed 白名单处理，未知标签整段打码 ——
///      见 `masked_scheme_position`。
#[test]
fn declared_tradeoffs_are_bounded() {
    for url in [
        // host / 库名 / 用户名 三个结构性位置
        "jdbc:mysql://S3CRET-D0-NOT-LEAK-7f21/db",
        "jdbc:mysql://host/S3CRET-D0-NOT-LEAK-7f21",
        "jdbc:mysql://S3CRET-D0-NOT-LEAK-7f21:pw@host/db",
    ] {
        let out = debug_of(url, &[]);
        assert!(
            out.contains("S3CRET-D0-NOT-LEAK-7f21"),
            "声明「结构性位置可读」已被打破（现在是 {out}）——若是有意收紧请更新注释"
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
