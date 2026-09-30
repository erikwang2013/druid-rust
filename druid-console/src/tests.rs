use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use druid_filter::Filter;
use druid_stat::StatFilter;
use std::sync::Arc;
use tower::ServiceExt;

fn test_app() -> Router {
    make_router(Arc::new(StatFilter::new("test-ds", 1000)))
}

fn app_with(filter: StatFilter) -> Router {
    make_router(Arc::new(filter))
}

async fn get(app: Router, uri: &str) -> axum::response::Response {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    app.oneshot(req).await.unwrap()
}

async fn get_authed(app: Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {t}"));
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), 102400)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn test_stat_json_endpoint() {
    let resp = get(test_app(), "/druid/stat.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_sql_json_endpoint() {
    let resp = get(test_app(), "/druid/sql.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_slow_sql_json_endpoint() {
    let resp = get(test_app(), "/druid/slow-sql.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_index_html_endpoint() {
    let resp = get(test_app(), "/druid/index.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 102400)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Druid Monitor"));
    assert!(html.contains("<!DOCTYPE html>"));
}

#[tokio::test]
async fn test_all_responses_are_no_store() {
    for uri in [
        "/druid/stat.json",
        "/druid/sql.json",
        "/druid/slow-sql.json",
        "/druid/index.html",
        "/druid/mascot.svg",
    ] {
        let resp = get(test_app(), uri).await;
        assert_eq!(
            resp.headers().get("cache-control").unwrap(),
            "no-store",
            "{uri} 缺少 Cache-Control: no-store"
        );
    }
}

#[tokio::test]
async fn test_auth_requires_token() {
    let app = make_router_with_token(
        Arc::new(StatFilter::new("test-ds", 1000)),
        Some("s3cret".to_string()),
    );

    // 无 token / 错误 token → 401
    for token in [None, Some("wrong")] {
        let resp = get_authed(app.clone(), "/druid/sql.json", token).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(resp.headers().get("cache-control").unwrap(), "no-store");
        assert!(resp.headers().contains_key("www-authenticate"));
    }

    // 正确 token → 200
    let resp = get_authed(app.clone(), "/druid/sql.json", Some("s3cret")).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // 非 Bearer 方案 → 401
    let req = Request::builder()
        .uri("/druid/sql.json")
        .header("Authorization", "Basic czNjcmV0")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn test_auth_empty_token_rejects_all() {
    let app = make_router_with_token(
        Arc::new(StatFilter::new("test-ds", 1000)),
        Some(String::new()),
    );
    for token in [None, Some("")] {
        let resp = get_authed(app.clone(), "/druid/stat.json", token).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

#[test]
fn test_ct_eq() {
    assert!(ct_eq(b"abc", b"abc"));
    assert!(!ct_eq(b"abc", b"abd"));
    assert!(!ct_eq(b"abc", b"ab"));
    assert!(ct_eq(b"", b""));
}

#[tokio::test]
async fn test_sql_json_pagination() {
    let filter = StatFilter::new("ds", 1000);
    for i in 0..3 {
        let sql = format!("SELECT {i}");
        filter.statement_execute_after(
            &druid_filter::FilterContext::new("ds").with_sql(&sql),
            1,
            0,
        );
    }
    let app = app_with(filter);

    let v = json_body(get(app.clone(), "/druid/sql.json?limit=2").await).await;
    assert_eq!(v.as_array().unwrap().len(), 2);

    let v = json_body(get(app.clone(), "/druid/sql.json?limit=2&offset=2").await).await;
    assert_eq!(v.as_array().unwrap().len(), 1);

    let v = json_body(get(app.clone(), "/druid/sql.json").await).await;
    assert_eq!(v.as_array().unwrap().len(), 3); // 默认 limit=1000
}

#[tokio::test]
async fn test_index_uses_single_stats_snapshot() {
    let filter = StatFilter::new("ds", 100);
    filter.statement_execute_after(
        &druid_filter::FilterContext::new("ds").with_sql("SLOW Q"),
        500,
        0,
    );
    let resp = get(app_with(filter), "/druid/index.html").await;
    let body = axum::body::to_bytes(resp.into_body(), 102400)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("SLOW Q"));
    assert!(html.contains(">1</div><div class=\"label\">Slow SQL"));
}

#[tokio::test]
async fn test_html_escape_xss() {
    let escaped = html_escape("<script>alert('xss')</script>");
    assert!(!escaped.contains('<'));
    assert!(!escaped.contains('>'));
    assert!(escaped.contains("&lt;"));
    assert!(escaped.contains("&gt;"));
}

#[test]
fn test_html_escape_all_chars() {
    let escaped = html_escape("&<>\"'/");
    assert_eq!(escaped, "&amp;&lt;&gt;&quot;&#x27;&#x2F;");
    assert_eq!(html_escape("plain text 123"), "plain text 123");
    assert_eq!(html_escape(""), "");
    // 无特殊字符时原样返回
    assert_eq!(html_escape("select * from t"), "select * from t");
}

#[tokio::test]
async fn test_stat_json_body() {
    let resp = get(test_app(), "/druid/stat.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/json"
    );
    let v = json_body(resp).await;
    assert_eq!(v["name"], "test-ds");
    assert_eq!(v["active_count"], 0);
    assert_eq!(v["execute_count"], 0);
}

#[tokio::test]
async fn test_sql_json_body_is_array() {
    let resp = get(test_app(), "/druid/sql.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = json_body(resp).await;
    assert!(v.is_array());
}

#[tokio::test]
async fn test_slow_sql_json_body_is_array() {
    let resp = get(test_app(), "/druid/slow-sql.json").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v = json_body(resp).await;
    assert!(v.is_array());
}

#[tokio::test]
async fn test_index_contains_datasource_name() {
    let resp = get(test_app(), "/druid/index.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    let body = axum::body::to_bytes(resp.into_body(), 102400)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("Druid Monitor — test-ds"));
    assert!(html.contains("SQL Stats (Top 20)"));
}

#[tokio::test]
async fn test_mascot_svg_endpoint() {
    let resp = get(test_app(), "/druid/mascot.svg").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("content-type").unwrap(), "image/svg+xml");
    let body = axum::body::to_bytes(resp.into_body(), 102400)
        .await
        .unwrap();
    let svg = String::from_utf8_lossy(&body);
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("德鲁伊猫头鹰"));
}

#[tokio::test]
async fn test_index_embeds_mascot() {
    let resp = get(test_app(), "/druid/index.html").await;
    let body = axum::body::to_bytes(resp.into_body(), 204800)
        .await
        .unwrap();
    let html = String::from_utf8_lossy(&body);
    // 宠物以 inline SVG 形式出现在页头，并注册为 favicon
    assert!(html.contains(r#"<div class="hdr"><svg"#));
    assert!(html.contains(r#"href="/druid/mascot.svg""#));
    assert_eq!(html.matches("<svg").count(), 1);
}

#[tokio::test]
async fn test_unknown_route_404() {
    let resp = get(test_app(), "/druid/nope").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[test]
fn test_druid_config_serde_roundtrip() {
    // 跨 crate 验证 druid-core 配置序列化：
    // 密码 skip_serializing 不落盘，且反序列化（缺省）不报错
    let mut cfg = druid_core::DruidConfig::new("jdbc:mysql://h/db", "root", "secret");
    cfg.max_active = 16;
    cfg.keep_alive = true;

    let json = serde_json::to_string(&cfg).unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("password"));

    let back: druid_core::DruidConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.url, cfg.url);
    assert_eq!(back.username, "root");
    assert_eq!(back.password, "");
    assert_eq!(back.max_active, 16);
    assert!(back.keep_alive);
}

#[test]
fn test_druid_config_serde_partial_json() {
    let json = r#"{"url":"jdbc:h2:mem:test","username":"sa"}"#;
    let cfg: druid_core::DruidConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.password, "");
    assert_eq!(cfg.max_active, 8); // serde default
    assert!(cfg.test_on_borrow); // serde default = true
}

#[test]
fn test_db_type_serde_renames() {
    use druid_core::DbType;
    assert_eq!(
        serde_json::to_string(&DbType::SqlServer).unwrap(),
        "\"sqlserver\""
    );
    assert_eq!(serde_json::to_string(&DbType::DM).unwrap(), "\"dm\"");
    assert_eq!(
        serde_json::to_string(&DbType::TransactSql).unwrap(),
        "\"transact-sql\""
    );
    assert_eq!(serde_json::to_string(&DbType::ODPS).unwrap(), "\"odps\"");
    assert_eq!(serde_json::to_string(&DbType::MySQL).unwrap(), "\"MySQL\"");

    let back: DbType = serde_json::from_str("\"dm\"").unwrap();
    assert_eq!(back, DbType::DM);
    let back: DbType = serde_json::from_str("\"sqlserver\"").unwrap();
    assert_eq!(back, DbType::SqlServer);
}

/// 吉祥物资产防漂移：`docs/assets/mascot.svg`（README 用）与
/// `druid-console/assets/mascot.svg`（`include_str!` 用）必须字节一致。
///
/// 必须有两份副本，因为 `include_str!` 不能引用 crate 目录外的文件——
/// 那样 `cargo package` 打出的 tarball 会缺文件、编译失败。
/// 从 registry 解包构建时仓库里的 `docs/` 不存在，此处自动跳过。
#[test]
fn mascot_matches_docs_copy() {
    let docs =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/assets/mascot.svg");
    if !docs.exists() {
        return; // 已发布的 tarball 内没有 docs/，跳过
    }
    assert_eq!(
        std::fs::read_to_string(&docs).unwrap(),
        include_str!("../assets/mascot.svg"),
        "两份 mascot.svg 已漂移：更新了 docs/assets/ 却忘了 druid-console/assets/"
    );
}
