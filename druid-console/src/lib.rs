use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};
use druid_stat::StatFilter;

/// HTML 实体转义
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// 应用状态
struct AppState {
    stat_filter: Arc<StatFilter>,
}

/// 启动 Druid 监控控制台 HTTP 服务
pub async fn start_server(
    stat_filter: Arc<StatFilter>,
    addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(AppState { stat_filter });

    let app = Router::new()
        .route("/druid/stat.json", get(stat_json))
        .route("/druid/sql.json", get(sql_json))
        .route("/druid/slow-sql.json", get(slow_sql_json))
        .route("/druid/index.html", get(index_page))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Druid console listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn stat_json(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let stat = state.stat_filter.get_datasource_stat();
    Json(serde_json::to_value(stat).unwrap())
}

async fn sql_json(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let stats = state.stat_filter.get_sql_stats();
    Json(serde_json::to_value(stats).unwrap())
}

async fn slow_sql_json(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let stats = state.stat_filter.get_slow_sql();
    Json(serde_json::to_value(stats).unwrap())
}

async fn index_page(State(state): State<Arc<AppState>>) -> axum::response::Html<String> {
    let stat = state.stat_filter.get_datasource_stat();
    let sql_stats = state.stat_filter.get_sql_stats();
    let slow = state.stat_filter.get_slow_sql();

    let mut rows = String::new();
    for s in sql_stats.iter().take(20) {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}ms</td><td>{}ms</td><td>{}</td><td>{}</td></tr>",
            html_escape(&s.sql),
            s.execute_count,
            s.total_time_ms,
            s.max_time_ms,
            s.error_count,
            s.last_execute_time.as_deref().unwrap_or("-")
        ));
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Druid Monitor</title>
<style>body{{font-family:monospace;margin:20px;background:#f5f5f5}}
h1{{color:#333}} .stat{{display:flex;gap:15px;flex-wrap:wrap;margin:15px 0}}
.card{{background:#fff;padding:15px;border-radius:8px;min-width:120px;text-align:center}}
.card .val{{font-size:28px;font-weight:bold;color:#1890ff}}
.card .label{{color:#999;font-size:12px;margin-top:5px}}
table{{width:100%;border-collapse:collapse;background:#fff;margin-top:20px}}
th,td{{padding:8px 12px;text-align:left;border-bottom:1px solid #eee;font-size:13px}}
th{{background:#fafafa;font-weight:bold}}</style></head>
<body>
<h1>Druid Monitor — {name}</h1>
<div class="stat">
<div class="card"><div class="val">{active}</div><div class="label">活跃连接</div></div>
<div class="card"><div class="val">{idle}</div><div class="label">空闲连接</div></div>
<div class="card"><div class="val">{borrow}</div><div class="label">借用次数</div></div>
<div class="card"><div class="val">{exec}</div><div class="label">SQL 执行</div></div>
<div class="card"><div class="val">{err}</div><div class="label">错误数</div></div>
<div class="card"><div class="val">{slow_count}</div><div class="label">慢SQL</div></div>
</div>
<h2>SQL 统计 (Top 20)</h2>
<table><tr><th>SQL</th><th>执行次数</th><th>总耗时</th><th>最大</th><th>错误</th><th>最后执行</th></tr>
{rows}</table>
</body></html>"#,
        name = stat.name,
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
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use druid_stat::StatFilter;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let stat_filter = Arc::new(StatFilter::new("test-ds", 1000));
        let state = Arc::new(AppState { stat_filter });
        Router::new()
            .route("/druid/stat.json", get(stat_json))
            .route("/druid/sql.json", get(sql_json))
            .route("/druid/slow-sql.json", get(slow_sql_json))
            .route("/druid/index.html", get(index_page))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_stat_json_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/druid/stat.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_sql_json_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/druid/sql.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_slow_sql_json_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/druid/slow-sql.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_index_html_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/druid/index.html")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 10240).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("Druid Monitor"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    #[tokio::test]
    async fn test_html_escape_xss() {
        let escaped = html_escape("<script>alert('xss')</script>");
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
        assert!(escaped.contains("&lt;"));
        assert!(escaped.contains("&gt;"));
    }
}
