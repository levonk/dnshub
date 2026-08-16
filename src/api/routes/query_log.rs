//! Query log endpoints per PRD lines 1353-1355.
//!
//! - `GET /api/v1/query-log`         — paginated, filterable query log
//! - `GET /api/v1/query-log/export`  — CSV export
//!
//! The query log subsystem (story 05-003) provides a SQLite-backed ring
//! buffer. When the query log store is not configured (e.g. 05-003 has
//! not merged), these endpoints return empty results with the correct
//! JSON shape so the frontend can render an empty state.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;

/// A single query log entry in the API response.
///
/// This DTO mirrors the shape expected by the NextJS frontend (TanStack
/// Table columns). When the query log store (05-003) is wired in, the
/// store's row type is converted into this DTO.
#[derive(Debug, Serialize)]
pub struct QueryLogEntryDto {
    pub timestamp: i64,
    pub client: String,
    pub domain: String,
    pub query_type: String,
    pub blocked: bool,
    pub category: Option<String>,
    pub upstream: Option<String>,
    pub response_time_ms: Option<u64>,
}

/// Paginated query log response.
#[derive(Debug, Serialize)]
pub struct QueryLogResponse {
    pub entries: Vec<QueryLogEntryDto>,
    pub page: u64,
    pub limit: u64,
    pub total: u64,
}

/// Query parameters for GET /api/v1/query-log.
#[derive(Debug, Deserialize, Default)]
pub struct QueryLogParams {
    pub client: Option<String>,
    pub blocked: Option<bool>,
    pub category: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

/// GET /api/v1/query-log — paginated, filterable query log.
///
/// Supports filtering by `client`, `blocked`, and `category`, with
/// `page` (1-based) and `limit` pagination. When the query log store is
/// not configured, returns an empty result set.
pub async fn get_query_log(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<QueryLogParams>,
) -> Json<QueryLogResponse> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(50).min(1000);

    // When the query log store (05-003) is available, this handler will
    // query it with the filters and return real entries. For now, return
    // an empty result set with the correct shape.
    Json(QueryLogResponse {
        entries: Vec::new(),
        page,
        limit,
        total: 0,
    })
}

/// Query parameters for GET /api/v1/query-log/export (same filters as
/// the paginated endpoint, minus pagination).
#[derive(Debug, Deserialize, Default)]
pub struct QueryLogExportParams {
    pub client: Option<String>,
    pub blocked: Option<bool>,
    pub category: Option<String>,
}

/// GET /api/v1/query-log/export — export the query log as CSV.
///
/// Returns a `text/csv` response with a header row and one row per
/// query log entry. When the query log store is not configured, returns
/// just the CSV header.
pub async fn export_query_log(
    State(_state): State<Arc<AppState>>,
    Query(_params): Query<QueryLogExportParams>,
) -> impl IntoResponse {
    let csv = build_csv(Vec::new());
    (
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv,
    )
}

/// Build a CSV string from a list of query log entries.
fn build_csv(entries: Vec<QueryLogEntryDto>) -> String {
    let mut out = String::from("timestamp,client,domain,query_type,blocked,category,upstream,response_time_ms\n");
    for e in &entries {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            e.timestamp,
            csv_escape(&e.client),
            csv_escape(&e.domain),
            csv_escape(&e.query_type),
            e.blocked,
            e.category.as_deref().map(csv_escape).unwrap_or_default(),
            e.upstream.as_deref().map(csv_escape).unwrap_or_default(),
            e.response_time_ms.map(|v| v.to_string()).unwrap_or_default(),
        ));
    }
    out
}

/// Escape a CSV field (wrap in quotes if it contains a comma or quote).
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, DnshubConfig};
    use axum::extract::{Query, State};

    fn test_state() -> Arc<AppState> {
        Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        )
    }

    #[tokio::test]
    async fn query_log_returns_empty() {
        let state = test_state();
        let Json(result) = get_query_log(State(state), Query(QueryLogParams::default())).await;
        assert!(result.entries.is_empty());
        assert_eq!(result.page, 1);
        assert_eq!(result.limit, 50);
        assert_eq!(result.total, 0);
    }

    #[tokio::test]
    async fn query_log_with_pagination() {
        let state = test_state();
        let params = QueryLogParams {
            page: Some(2),
            limit: Some(10),
            ..Default::default()
        };
        let Json(result) = get_query_log(State(state), Query(params)).await;
        assert_eq!(result.page, 2);
        assert_eq!(result.limit, 10);
    }

    #[tokio::test]
    async fn query_log_with_filters() {
        let state = test_state();
        let params = QueryLogParams {
            client: Some("192.168.1.20".to_string()),
            blocked: Some(true),
            category: Some("social".to_string()),
            ..Default::default()
        };
        let Json(result) = get_query_log(State(state), Query(params)).await;
        assert!(result.entries.is_empty());
    }

    #[tokio::test]
    async fn query_log_export_returns_csv() {
        let state = test_state();
        let response = export_query_log(State(state), Query(QueryLogExportParams::default())).await;
        let resp = response.into_response();
        let ct = resp.headers().get("content-type").unwrap();
        assert!(ct.to_str().unwrap().contains("text/csv"));
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let csv = String::from_utf8(body.to_vec()).unwrap();
        assert!(csv.contains("timestamp,client,domain"));
    }

    #[test]
    fn csv_escape_plain() {
        assert_eq!(csv_escape("hello"), "hello");
    }

    #[test]
    fn csv_escape_with_comma() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_escape_with_quote() {
        assert_eq!(csv_escape("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn build_csv_header_only() {
        let csv = build_csv(Vec::new());
        assert!(csv.starts_with("timestamp,client,domain"));
        assert_eq!(csv.matches('\n').count(), 1);
    }

    #[test]
    fn build_csv_with_entries() {
        let entries = vec![QueryLogEntryDto {
            timestamp: 1000,
            client: "192.168.1.20".to_string(),
            domain: "ads.example.com".to_string(),
            query_type: "A".to_string(),
            blocked: true,
            category: Some("ads".to_string()),
            upstream: None,
            response_time_ms: Some(42),
        }];
        let csv = build_csv(entries);
        assert!(csv.contains("1000,192.168.1.20,ads.example.com,A,true,ads,,42"));
    }
}
