//! Jev, TypeSafe's System One decision model, reached through OpenRouter (spec §11).
//!
//! Answers are cached on disk by (model, state, questions), in the files the
//! Python engine reads and writes, so a replay is instant, free and
//! deterministic in either engine. The API key is never logged.

use chrono::Utc;
use elessar_core::jevcache;
use elessar_core::pipeline::py;
use elessar_core::session::{JevJob, JevResult};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Requests in flight at once, as `jev.ask_many` in Python.
const WORKERS: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(30);

/// One file in the cache directory.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub payload: Option<Value>,
    pub model: Option<Value>,
    pub at: Option<Value>,
}

pub struct JevClient {
    http: reqwest::Client,
    endpoint: String,
    key: Option<String>,
    pub model: String,
    pub cache_dir: PathBuf,
}

impl JevClient {
    pub fn new(http: reqwest::Client, endpoint: String, key: Option<String>, model: String, cache_dir: PathBuf) -> JevClient {
        JevClient { http, endpoint, key, model, cache_dir }
    }

    pub fn available(&self) -> bool {
        self.key.is_some()
    }

    fn payload(&self, state: &Value, questions: &Value) -> Value {
        json!({"model": self.model, "state": state, "questions": questions})
    }

    /// Where the answer to this exact ask lives, cached or not.
    pub fn cache_path(&self, state: &Value, questions: &Value) -> PathBuf {
        jevcache::cache_path(&self.cache_dir, &self.payload(state, questions))
    }

    /// One request: a state and named questions, answered from the cache first.
    pub async fn ask(&self, state: &Value, questions: &Value) -> JevResult {
        let payload = self.payload(state, questions);
        let path = jevcache::cache_path(&self.cache_dir, &payload);
        if let Some(cached) = jevcache::load_cached(&path) {
            return result_of(&cached, true);
        }
        let Some(key) = &self.key else {
            return JevResult { error: Some("Jev is off: no OPENROUTER_API_KEY".into()), ..JevResult::default() };
        };
        let started = Instant::now();
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(key)
            .header("Content-Type", "application/json")
            .header("X-Title", "ELESSAR")
            .timeout(TIMEOUT)
            .body(py::dumps(&payload, false))
            .send()
            .await;
        let body = match read_body(response).await {
            Ok(body) => body,
            Err(error) => return JevResult { error: Some(error), ..JevResult::default() },
        };
        let entry = cache_entry(payload, &body, &self.model, started.elapsed());
        if let Err(e) = write_entry(&path, &entry) {
            eprintln!("  could not write the Jev cache file {}: {e}", path.display());
        }
        result_of(&entry, false)
    }

    /// Run the requests of one source, at most [`WORKERS`] at once. Results
    /// come back in job order, by record index.
    pub async fn ask_many(self: &Arc<Self>, jobs: Vec<JevJob>) -> Vec<(u32, JevResult)> {
        let workers = Arc::new(Semaphore::new(WORKERS));
        let tasks: Vec<_> = jobs
            .into_iter()
            .map(|job| {
                let (client, workers) = (self.clone(), workers.clone());
                let task = tokio::spawn(async move {
                    let _turn = workers.acquire_owned().await;
                    client.ask(&job.state, &job.questions).await
                });
                (job.index, task)
            })
            .collect();
        let mut results = Vec::with_capacity(tasks.len());
        for (index, task) in tasks {
            results.push((index, task.await.unwrap_or_else(|e| JevResult { error: Some(e.to_string()), ..JevResult::default() })));
        }
        results
    }

    /// Every readable cache file, by name.
    pub fn list_cache(&self) -> Vec<CacheEntry> {
        let Ok(dir) = std::fs::read_dir(&self.cache_dir) else { return Vec::new() };
        let mut names: Vec<PathBuf> = dir.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|path| {
                let d = jevcache::load_cached(&path)?;
                let field = |k: &str| d.get(k).filter(|v| !v.is_null()).cloned();
                Some(CacheEntry { payload: field("payload"), model: field("model"), at: field("at"), path })
            })
            .collect()
    }

    /// Delete cache entries, only ever directly inside the cache directory.
    /// Returns how many were removed.
    pub fn forget<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) -> usize {
        let unique: HashSet<&Path> = paths.into_iter().collect();
        unique.into_iter().filter(|p| p.parent() == Some(self.cache_dir.as_path()) && std::fs::remove_file(p).is_ok()).count()
    }
}

/// The response body as JSON, or the message for the record's `error`.
async fn read_body(response: Result<reqwest::Response, reqwest::Error>) -> Result<Value, String> {
    let response = response.map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let detail: String = text.chars().take(300).collect();
        return Err(format!("HTTP {}: {detail}", status.as_u16()));
    }
    py::json_loads(&text).map_err(|e| format!("Jev sent something other than JSON: {e:?}"))
}

/// A cache file's content, in the shape `jev.ask` writes:
/// `{payload, answers, model, ms, cost, at}`.
pub fn cache_entry(payload: Value, body: &Value, model: &str, took: Duration) -> Value {
    let cost = body.get("usage").and_then(|u| u.get("cost")).filter(|c| !c.is_null()).cloned().unwrap_or_else(|| json!(0.0));
    json!({
        "payload": payload,
        "answers": body.get("answers").cloned().unwrap_or_else(|| json!({})),
        "model": body.get("model").cloned().unwrap_or_else(|| json!(model)),
        "ms": took.as_millis() as u64,
        "cost": cost,
        "at": Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    })
}

static TEMP_FILES: AtomicU64 = AtomicU64::new(0);

/// Write a cache file in one step, so a reader in either engine never sees half of one.
pub fn write_entry(path: &Path, entry: &Value) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".tmp-{}-{}", std::process::id(), TEMP_FILES.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&temp, py::dumps(entry, false))?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

fn result_of(entry: &Value, cached: bool) -> JevResult {
    JevResult {
        answers: entry.get("answers").cloned(),
        model: entry.get("model").and_then(Value::as_str).map(String::from),
        ms: entry.get("ms").and_then(Value::as_u64),
        cost: entry.get("cost").and_then(Value::as_f64),
        cached,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("elessar-jev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_cache_file_has_the_shape_python_writes() {
        let dir = temp_dir("shape");
        let payload = py::json_loads(r#"{"model": "jev-latest", "state": {"source": "Déjà", "record": {"amount": 1.50, "n": 3}}, "questions": {"s_1": {"type": "noul", "instructions": "Why?"}}}"#).unwrap();
        let body = json!({"answers": {"s_1": {"noul": 0.25}}, "model": "jev-20260101", "usage": {"cost": 0.0001}});
        let entry = cache_entry(payload.clone(), &body, "jev-latest", Duration::from_millis(42));
        let path = jevcache::cache_path(&dir, &payload);
        write_entry(&path, &entry).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        // json.dump: separators ", " and ": ", key order as written, non-ASCII escaped, floats as written.
        assert!(text.is_ascii(), "{text}");
        assert!(text.starts_with(r#"{"payload": {"model": "jev-latest", "state": {"source": ""#), "{text}");
        assert!(text.contains(r#", "record": {"amount": 1.5, "n": 3}}, "questions": "#), "{text}");
        assert_eq!(py::json_loads(&text).unwrap()["payload"], payload);
        assert!(text.contains(r#""answers": {"s_1": {"noul": 0.25}}, "model": "jev-20260101", "ms": 42, "cost": 0.0001, "at": ""#), "{text}");
        let at = py::json_loads(&text).unwrap()["at"].as_str().unwrap().to_string();
        assert_eq!((at.len(), &at[4..5], &at[10..11], &at[19..]), (20, "-", "T", "Z"));
        // Read back, the stored payload still gives the file's own name.
        let stored = py::json_loads(&text).unwrap();
        assert_eq!(jevcache::cache_path(&dir, &stored["payload"]), path);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp file left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_answer_without_usage_costs_nothing_and_keeps_our_model() {
        let entry = cache_entry(json!({}), &json!({"usage": null}), "jev-latest", Duration::ZERO);
        assert_eq!(entry["cost"], json!(0.0));
        assert_eq!(entry["model"], json!("jev-latest"));
        assert_eq!(entry["answers"], json!({}));
    }

    #[test]
    fn forget_stays_inside_the_cache_directory() {
        let dir = temp_dir("forget");
        std::fs::create_dir_all(&dir).unwrap();
        let (inside, outside) = (dir.join("a.json"), dir.with_extension("outside.json"));
        std::fs::write(&inside, "{}").unwrap();
        std::fs::write(&outside, "{}").unwrap();
        let client = JevClient::new(reqwest::Client::new(), String::new(), None, "m".into(), dir.clone());
        let sneaky = dir.join("sub/../..").join(outside.file_name().unwrap());
        assert_eq!(client.forget([outside.as_path(), sneaky.as_path(), inside.as_path(), inside.as_path()]), 1);
        assert!(outside.exists() && !inside.exists());
        std::fs::remove_file(&outside).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listing_skips_what_is_not_a_cache_file() {
        let dir = temp_dir("list");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.json"), r#"{"answers": {}, "model": "m", "at": null}"#).unwrap();
        std::fs::write(dir.join("a.json"), r#"{"payload": {"x": 1}, "answers": {}, "model": "m", "at": "t"}"#).unwrap();
        std::fs::write(dir.join("bad.json"), "not json").unwrap();
        std::fs::write(dir.join(".tmp-1-1"), "{}").unwrap();
        let client = JevClient::new(reqwest::Client::new(), String::new(), None, "m".into(), dir.clone());
        let entries = client.list_cache();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].payload.is_some() && entries[1].payload.is_none());
        assert_eq!(entries[1].at, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
