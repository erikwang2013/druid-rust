//! Druid-Rust 监控控制台（基于 axum 的只读 HTTP 端点）
//!
//! 暴露数据源概览、SQL 统计、慢 SQL 与监控首页；所有端点均为只读 GET。
//!
//! # 安全
//!
//! `/druid/sql.json` 等端点会返回 SQL 原文（可能含字面量中的密码/手机号等敏感数据），
//! 因此：
//! - 生产环境务必使用 [`start_server_with_token`] / [`make_router_with_token`] 启用
//!   `Authorization: Bearer <token>` 校验，不要把无鉴权端点暴露在集群内网；
//! - [`start_server`] / [`make_router`] 是**无鉴权**快捷方式，仅用于本机调试；
//! - `/druid/*` 响应（含 401）都带 `Cache-Control: no-store`，避免统计内容被代理/浏览器缓存；
//! - `/druid/sql.json`、`/druid/slow-sql.json` 支持 `?limit=`（默认 1000）+`?offset=` 分页。

#![warn(missing_docs)]

use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{routing::get, Json, Router};
use druid_stat::StatFilter;

/// 项目宠物「小德」——德鲁伊猫头鹰。
///
/// 与 README 共用同一份 `docs/assets/mascot.svg`，避免资产副本。
/// 注：跨 crate 目录的 `include_str!` 会让 `cargo package` 失败；
/// 若将来要发布 druid-console 到 crates.io，把该文件挪进本 crate 即可。
const MASCOT_SVG: &str = include_str!("../../docs/assets/mascot.svg");

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
        .replace('/', "&#x2F;")
}

/// 序列化失败不再降级成 `HTTP 200 + null`（会让抓取端静默失效），而是 500 + 日志
fn json_or_error<T: serde::Serialize>(
    value: &T,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    serde_json::to_value(value).map(Json).map_err(|e| {
        tracing::error!("druid-console: JSON serialization failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "serialization failed".to_string(),
        )
    })
}

/// 常量时间字节比较，避免 token 校验被计时侧信道推断
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Clone)]
struct AppState {
    stat_filter: Arc<StatFilter>,
}

/// SQL 列表分页参数：`?limit=100&offset=0`（默认 limit=1000, offset=0）
#[derive(serde::Deserialize)]
struct Pagination {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize {
    1000
}

/// 无鉴权路由（仅本机调试用，生产请用 [`make_router_with_token`]）
pub fn make_router(stat_filter: Arc<StatFilter>) -> Router {
    make_router_with_token(stat_filter, None)
}

/// 带可选 Bearer 鉴权的路由
///
/// `token` 为 `Some(t)` 时要求所有 `/druid/*` 请求携带 `Authorization: Bearer t`，
/// 否则返回 401（token 为空串则全部 401，不会静默降级为无鉴权）。
pub fn make_router_with_token(stat_filter: Arc<StatFilter>, token: Option<String>) -> Router {
    let state = Arc::new(AppState { stat_filter });
    let router = Router::new()
        .route("/druid/stat.json", get(stat_json))
        .route("/druid/sql.json", get(sql_json))
        .route("/druid/slow-sql.json", get(slow_sql_json))
        .route("/druid/index.html", get(index_page))
        .route("/druid/mascot.svg", get(mascot_svg))
        .with_state(state);

    let router = match token {
        Some(token) => router.layer(middleware::from_fn_with_state(
            Arc::new(token),
            auth_middleware,
        )),
        None => router,
    };
    // 放在最外层：401 响应同样带 no-store
    router.layer(middleware::from_fn(no_store))
}

/// 校验 `Authorization: Bearer <token>`
async fn auth_middleware(State(token): State<Arc<String>>, req: Request, next: Next) -> Response {
    // 空 token 一律拒绝：否则 `Authorization: Bearer `（空凭据）会被判为合法
    let authorized = !token.is_empty()
        && req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes()));
    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "unauthorized",
        )
            .into_response()
    }
}

/// 统计响应可能含 SQL 原文，禁止任何缓存
async fn no_store(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

async fn mascot_svg() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], MASCOT_SVG)
}

/// 无鉴权启动控制台（仅本机调试用，见 crate 文档的安全说明）
pub async fn start_server(
    stat_filter: Arc<StatFilter>,
    addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    start_server_with_token(stat_filter, addr, None).await
}

/// 启动控制台，`token` 为 `Some` 时启用 Bearer 鉴权
pub async fn start_server_with_token(
    stat_filter: Arc<StatFilter>,
    addr: &str,
    token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let authenticated = token.is_some();
    let app = make_router_with_token(stat_filter, token);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    if authenticated {
        tracing::info!("Druid console listening on http://{} (Bearer auth)", addr);
    } else {
        tracing::warn!(
            "Druid console listening on http://{} WITHOUT authentication — SQL text is exposed",
            addr
        );
    }
    axum::serve(listener, app).await?;
    Ok(())
}

async fn stat_json(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let stat = state.stat_filter.get_datasource_stat();
    json_or_error(&stat)
}

async fn sql_json(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Pagination>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let stats = state.stat_filter.get_sql_stats();
    let page: Vec<_> = stats
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .collect();
    json_or_error(&page)
}

async fn slow_sql_json(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Pagination>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let stats = state.stat_filter.get_sql_stats();
    let slow = state.stat_filter.get_slow_sql_from(&stats);
    let page: Vec<_> = slow
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .collect();
    json_or_error(&page)
}

async fn index_page(State(state): State<Arc<AppState>>) -> axum::response::Html<String> {
    let stat = state.stat_filter.get_datasource_stat();
    let sql_stats = state.stat_filter.get_sql_stats();
    // 复用同一份快照，避免重复取锁/克隆/排序
    let slow = state.stat_filter.get_slow_sql_from(&sql_stats);

    let mut rows = String::new();
    for s in sql_stats.iter().take(20) {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}ms</td><td>{}ms</td><td>{}</td><td>{}</td></tr>",
            html_escape(&s.sql),
            s.execute_count,
            s.total_time_ms,
            s.max_time_ms,
            s.error_count,
            html_escape(s.last_execute_time.as_deref().unwrap_or("-"))
        ));
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Druid Monitor</title>
<link rel="icon" type="image/svg+xml" href="/druid/mascot.svg">
<style>body{{font-family:monospace;margin:20px;background:#f5f5f5}}
h1{{color:#333;margin:0}} .stat{{display:flex;gap:15px;flex-wrap:wrap;margin:15px 0}}
.hdr{{display:flex;align-items:center;gap:14px;margin-bottom:10px}}
.hdr svg{{width:56px;height:56px;flex:none}}
.card{{background:#fff;padding:15px;border-radius:8px;min-width:120px;text-align:center}}
.card .val{{font-size:28px;font-weight:bold;color:#1890ff}}
.card .label{{color:#999;font-size:12px;margin-top:5px}}
table{{width:100%;border-collapse:collapse;background:#fff;margin-top:20px}}
th,td{{padding:8px 12px;text-align:left;border-bottom:1px solid #eee;font-size:13px}}
th{{background:#fafafa;font-weight:bold}}</style></head>
<body>
<div class="hdr">{mascot}<h1>Druid Monitor — {name}</h1></div>
<div class="stat">
<div class="card"><div class="val">{active}</div><div class="label">Active</div></div>
<div class="card"><div class="val">{idle}</div><div class="label">Idle</div></div>
<div class="card"><div class="val">{borrow}</div><div class="label">Borrows</div></div>
<div class="card"><div class="val">{exec}</div><div class="label">SQL Exec</div></div>
<div class="card"><div class="val">{err}</div><div class="label">Errors</div></div>
<div class="card"><div class="val">{slow_count}</div><div class="label">Slow SQL</div></div>
</div>
<h2>SQL Stats (Top 20)</h2>
<table><tr><th>SQL</th><th>Exec Count</th><th>Total</th><th>Max</th><th>Errors</th><th>Last Run</th></tr>
{rows}</table>
</body></html>"#,
        mascot = MASCOT_SVG,
        name = html_escape(&stat.name),
        active = stat.active_count,
        idle = stat.idle_count,
        borrow = stat.borrow_count,
        exec = stat.execute_count,
        err = stat.error_count,
        slow_count = slow.len(),
        rows = rows,
    );

    axum::response::Html(html)
}

#[cfg(test)]
mod tests;
