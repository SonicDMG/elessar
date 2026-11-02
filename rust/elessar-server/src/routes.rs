//! The HTTP surface, route for route as `server/app.py`: the mock services, the
//! API and the web client. Every response carries `Cache-Control: no-store`.

use crate::app::{ApiError, App};
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

type Shared = Arc<App>;
type Params = Query<Vec<(String, String)>>;

/// The web client and the API, for one app.
pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/mock/{pack}/{source}", get(mock))
        .route("/api/scenarios", get(scenarios))
        .route("/api/state", get(state))
        .route("/api/timeline", get(timeline))
        .route("/api/jev/cache", get(jev_cache).post(jev_cache_op))
        .route("/api/action", axum::routing::post(action))
        .route("/api/agent", axum::routing::post(agent))
        .fallback(fallback)
        .layer(middleware::from_fn(log_request))
        .with_state(app)
}

/// `  GET /path -> 200` on stderr, as the Python server writes its log.
async fn log_request(request: Request, next: Next) -> Response {
    let (method, path) = (request.method().clone(), request.uri().path_and_query().map_or(String::new(), ToString::to_string));
    let response = next.run(request).await;
    eprintln!("  {method} {path} -> {}", response.status().as_u16());
    response
}

fn respond(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn json_bytes(status: StatusCode, body: Vec<u8>) -> Response {
    respond(status, "application/json", body)
}

fn error_json(status: StatusCode, message: &str) -> Response {
    json_bytes(status, json!({"error": message}).to_string().into_bytes())
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(m) => error_json(StatusCode::BAD_REQUEST, &m),
            ApiError::NotFound(m) => error_json(StatusCode::NOT_FOUND, &m),
            ApiError::Internal(m) => error_json(StatusCode::INTERNAL_SERVER_ERROR, &m),
        }
    }
}

fn ok(result: Result<Vec<u8>, ApiError>) -> Response {
    match result {
        Ok(body) => json_bytes(StatusCode::OK, body),
        Err(e) => e.into_response(),
    }
}

/// `?scenario=`, as `parse_qs` reads it: the first non-blank value, else the default.
fn scenario_param(app: &App, Query(params): Params) -> String {
    params.into_iter().find(|(k, v)| k == "scenario" && !v.is_empty()).map_or_else(|| app.default.clone(), |(_, v)| v)
}

/// A POST body as a JSON object. Empty means `{}`, as in Python.
fn parse_body(body: &[u8]) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_slice(body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(ApiError::BadRequest("the request body must be a JSON object".into()))
    }
}

async fn scenarios(State(app): State<Shared>) -> Response {
    json_bytes(StatusCode::OK, app.scenarios().to_string().into_bytes())
}

async fn state(State(app): State<Shared>, params: Params) -> Response {
    let pid = scenario_param(&app, params);
    ok(app.state(&pid).await)
}

async fn timeline(State(app): State<Shared>, params: Params) -> Response {
    let pid = scenario_param(&app, params);
    ok(app.timeline(&pid).await)
}

async fn jev_cache(State(app): State<Shared>, params: Params) -> Response {
    let pid = scenario_param(&app, params);
    ok(app.jev_cache(app.clone(), &pid).await)
}

async fn jev_cache_op(State(app): State<Shared>, body: Bytes) -> Response {
    match parse_body(&body) {
        Ok(payload) => ok(app.jev_cache_op(app.clone(), payload).await),
        Err(e) => e.into_response(),
    }
}

async fn action(State(app): State<Shared>, body: Bytes) -> Response {
    match parse_body(&body) {
        Ok(payload) => ok(app.action(payload).await),
        Err(e) => e.into_response(),
    }
}

async fn agent(State(app): State<Shared>, body: Bytes) -> Response {
    match parse_body(&body) {
        Ok(payload) => ok(app.ask_agent(payload).await),
        Err(e) => e.into_response(),
    }
}

/// `pipeline.CONTENT_TYPES`, by source format.
fn format_content_type(format: &str) -> &'static str {
    match format {
        "csv" => "text/csv",
        "jsonl" => "application/x-ndjson",
        "nested-json" | "columnar" => "application/json",
        "xml" => "application/xml",
        "kv" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// The raw payload after the declared latency, in its own wire format.
async fn mock(State(app): State<Shared>, UrlPath((pid, sid)): UrlPath<(String, String)>) -> Response {
    let source = match app.mock_source(&pid, &sid) {
        Ok(source) => source,
        Err(e) => return e.into_response(),
    };
    tokio::time::sleep(Duration::from_millis(source.latency_ms)).await;
    let Some(path) = &source.path else {
        return error_json(StatusCode::NOT_FOUND, &format!("source '{sid}' has no file"));
    };
    let body = match tokio::fs::read(path).await {
        Ok(body) => body,
        Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, &format!("cannot read source '{sid}': {e}")),
    };
    let mut response = respond(StatusCode::OK, &format!("{}; charset=utf-8", format_content_type(&source.format)), body);
    let headers = response.headers_mut();
    for (name, value) in [("x-source-format", &source.format), ("x-source-name", &source.name)] {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    response
}

/// What `mimetypes.guess_type` says for what `web/` can hold; `.js` as `text/javascript`.
fn static_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("js" | "mjs") => "text/javascript",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("ico") => "image/vnd.microsoft.icon",
        Some("txt") => "text/plain",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// The file under `web` a URL path names, or `None` when there is no such file
/// or it lies outside. Symlinks and `..` are resolved before the check. The
/// path is taken as written, without percent decoding, as Python takes it.
pub fn static_file(web: &Path, url_path: &str) -> Option<PathBuf> {
    let rel = if url_path.is_empty() || url_path == "/" { "index.html" } else { url_path.trim_start_matches('/') };
    let root = web.canonicalize().ok()?;
    let full = root.join(rel).canonicalize().ok()?;
    (full != root && full.starts_with(&root) && full.is_file()).then_some(full)
}

/// `GET` of anything else is a file of the web client; anything else is a 404.
async fn fallback(State(app): State<Shared>, method: Method, uri: Uri) -> Response {
    if method != Method::GET {
        return error_json(StatusCode::NOT_FOUND, "not found");
    }
    let Some(file) = static_file(&app.config.web_dir, uri.path()) else {
        return error_json(StatusCode::NOT_FOUND, "not found");
    };
    match tokio::fs::read(&file).await {
        Ok(body) => respond(StatusCode::OK, static_content_type(&file), body),
        Err(_) => error_json(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_files_stay_inside_the_web_directory() {
        let base = std::env::temp_dir().join(format!("elessar-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let web = base.join("web");
        std::fs::create_dir_all(web.join("sub")).unwrap();
        std::fs::write(web.join("index.html"), "<html>").unwrap();
        std::fs::write(web.join("sub/a.js"), "1").unwrap();
        std::fs::write(base.join("secret.txt"), "no").unwrap();
        std::fs::create_dir_all(base.join("webby")).unwrap();
        std::fs::write(base.join("webby/x.txt"), "no").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("secret.txt"), web.join("link.txt")).unwrap();
        let web_real = web.canonicalize().unwrap();

        assert_eq!(static_file(&web, "/"), Some(web_real.join("index.html")));
        assert_eq!(static_file(&web, ""), Some(web_real.join("index.html")));
        assert_eq!(static_file(&web, "/sub/a.js"), Some(web_real.join("sub/a.js")));
        assert_eq!(static_file(&web, "/sub/../index.html"), Some(web_real.join("index.html")));
        for bad in ["/../secret.txt", "/sub/../../secret.txt", "//etc/passwd", "/../webby/x.txt", "/link.txt", "/sub", "/missing.js", "/..", "/%2e%2e/secret.txt", "/index.html\0"] {
            assert_eq!(static_file(&web, bad), None, "{bad}");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn content_types_match_python() {
        assert_eq!(static_content_type(Path::new("a.js")), "text/javascript");
        assert_eq!(static_content_type(Path::new("a.css")), "text/css");
        assert_eq!(static_content_type(Path::new("index.html")), "text/html");
        assert_eq!(static_content_type(Path::new("noext")), "application/octet-stream");
        assert_eq!(format_content_type("nested-json"), "application/json");
        assert_eq!(format_content_type("kv"), "text/plain");
    }

    #[test]
    fn a_post_body_must_be_an_object_or_empty() {
        assert_eq!(parse_body(b"").unwrap(), json!({}));
        assert_eq!(parse_body(br#"{"a": 1}"#).unwrap(), json!({"a": 1}));
        assert!(parse_body(b"[1]").is_err());
        assert!(parse_body(b"{nope").is_err());
    }
}
