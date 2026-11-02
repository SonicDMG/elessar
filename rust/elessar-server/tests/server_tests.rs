//! Drives the server over a bound local port. No test reaches the network:
//! Jev and the model are off, or point at a fake on another local port.

use axum::routing::post;
use axum::{Json, Router};
use elessar_server::app::App;
use elessar_server::config::Config;
use elessar_server::routes;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf()
}

fn temp_cache(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("elessar-server-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn config(cache: PathBuf) -> Config {
    let root = repo();
    Config {
        scenarios_dir: root.join("scenarios"),
        web_dir: root.join("web"),
        jev_cache_dir: cache,
        default_scenario: "change-cascade".into(),
        api_key: None,
        decision_model: "jev-latest".into(),
        inference_model: "test-model".into(),
        // Port 9 refuses: a test that reached for these by mistake fails instead of calling out.
        jev_endpoint: "http://127.0.0.1:9/systemone".into(),
        messages_endpoint: "http://127.0.0.1:9/messages".into(),
    }
}

async fn start(config: Config) -> (String, Arc<App>) {
    let app = App::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    app.set_base_url(base.clone());
    let router = routes::router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, app)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn get(base: &str, path: &str) -> (u16, Value) {
    let response = client().get(format!("{base}{path}")).send().await.unwrap();
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap())
}

async fn post_json(base: &str, path: &str, body: Value) -> (u16, Value) {
    let response = client().post(format!("{base}{path}")).body(body.to_string()).send().await.unwrap();
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap())
}

/// Every source of every pack over the wire, with Jev off.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_session_of_every_pack_over_http() {
    let cache = temp_cache("whole");
    let (base, _app) = start(config(cache.clone())).await;

    let (status, scenarios) = get(&base, "/api/scenarios").await;
    assert_eq!(status, 200);
    assert_eq!(scenarios["default"], "change-cascade");
    assert_eq!(scenarios["invalid"], json!({}));
    let ids: Vec<String> = scenarios["scenarios"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["change-cascade", "fraud-ring", "supply-chain"]);

    for entry in scenarios["scenarios"].as_array().unwrap() {
        let id = entry["id"].as_str().unwrap();
        let (status, state) = get(&base, &format!("/api/state?scenario={id}")).await;
        assert_eq!(status, 200);
        assert_eq!(state["scenario"]["id"], id);
        assert_eq!(state["nodes"].as_array().unwrap().len() as u64, entry["pieces"].as_u64().unwrap());
        assert_eq!(state["agent"], json!({"available": false, "model": null, "transcript": [], "proposals": {}}));
        assert_eq!(state["jev"]["available"], false);

        let sources: Vec<String> = state["sources"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(sources.len() as u64, entry["sources"].as_u64().unwrap());
        let mut last = Value::Null;
        for sid in &sources {
            let (status, reply) = post_json(&base, "/api/action", json!({"scenario": id, "action": "connect", "source": sid})).await;
            assert_eq!(status, 200, "{id} {sid}: {reply}");
            let card = reply["state"]["sources"].as_array().unwrap().iter().find(|s| s["id"] == sid.as_str()).unwrap();
            assert_eq!(card["connected"], true, "{id} {sid}: {card}");
            assert_eq!(card["error"], Value::Null);
            assert!(card["records"].as_u64().unwrap() > 0, "{id} {sid}");
            assert!(reply["events"].is_array());
            last = reply;
        }
        assert!(last["state"]["earned"].as_array().unwrap().iter().any(|k| k == "full_fusion"), "{id}");
        let (_, state) = get(&base, &format!("/api/state?scenario={id}")).await;
        assert!(state["audit"].as_array().unwrap().iter().any(|a| a["action"] == "connect" && a["detail"].as_str().unwrap().contains(&format!("from {base}/mock/{id}/"))), "{id}");

        let (status, tl) = get(&base, &format!("/api/timeline?scenario={id}")).await;
        assert_eq!(status, 200);
        for key in ["slices", "final", "event_count", "undated_count", "unknown_time"] {
            assert!(!tl[key].is_null(), "{id}: timeline.{key}");
        }

        let (status, report) = get(&base, &format!("/api/jev/cache?scenario={id}")).await;
        assert_eq!(status, 200);
        assert_eq!(report["available"], false);
        assert_eq!(report["files"], 0, "{id}: nothing cached in a fresh directory");
        let signals = report["signals"].as_array().unwrap();
        assert!(!signals.is_empty(), "{id}: every shipped pack has Jev signals");
        for s in signals {
            assert_eq!(s["cached"], 0);
            assert_eq!(s["live"].as_u64().unwrap() as usize, s["records"].as_array().unwrap().len());
            assert_eq!(s["connected"], true);
        }

        let (status, reply) = post_json(&base, "/api/action", json!({"scenario": id, "action": "reset"})).await;
        assert_eq!(status, 200);
        assert_eq!(reply["events"][0]["kind"], "reset");
        assert!(reply["state"]["sources"].as_array().unwrap().iter().all(|s| s["connected"] == false));
    }
    assert!(!cache.exists(), "Jev off writes nothing");
}

/// Statuses and error strings where the frontend reads them.
#[tokio::test(flavor = "multi_thread")]
async fn errors_read_as_they_do_in_python() {
    let (base, _app) = start(config(temp_cache("errors"))).await;
    let cases = [
        ("/api/action", json!({"scenario": "nope", "action": "reset"}), 400, "missing or unknown field 'nope'"),
        ("/api/action", json!({"action": "connect"}), 400, "missing or unknown field 'source'"),
        ("/api/action", json!({"action": "reconnect"}), 400, "missing or unknown field 'sources'"),
        ("/api/action", json!({"action": "dance"}), 400, "unknown action 'dance'"),
        ("/api/action", json!({}), 400, "unknown action 'None'"),
        ("/api/action", json!({"action": "engine", "engine": "magic"}), 400, "engine must be rules or jev"),
        ("/api/action", json!({"action": "judge", "signal": "x", "judgement": "maybe"}), 400, "judgement must be trust, discount or dismiss"),
        ("/api/action", json!({"action": "agent_decide", "proposal": "p9", "decision": "approve"}), 400, "no pending proposal with that id"),
        ("/api/agent", json!({"message": "hello"}), 400, "The agent is off: add OPENROUTER_API_KEY to .env"),
        ("/api/jev/cache", json!({"op": "dance"}), 400, "unknown op 'dance'"),
        ("/api/jev/cache", json!({}), 400, "unknown op 'None'"),
        ("/api/jev/cache", json!({"op": "forget", "signal": "nope"}), 400, "no Jev signal nope"),
        ("/api/nothing", json!({}), 404, "not found"),
    ];
    for (path, body, status, error) in cases {
        let (got, reply) = post_json(&base, path, body.clone()).await;
        assert_eq!((got, reply["error"].as_str().unwrap()), (status, error), "{path} {body}");
    }
    let bad = client().post(format!("{base}/api/action")).body("{nope").send().await.unwrap();
    assert_eq!(bad.status(), 400);

    for path in ["/api/state?scenario=nope", "/api/timeline?scenario=nope", "/api/jev/cache?scenario=nope"] {
        let (status, reply) = get(&base, path).await;
        assert_eq!((status, reply["error"].as_str().unwrap()), (404, "unknown scenario or source 'nope'"), "{path}");
    }
    let (status, reply) = get(&base, "/mock/change-cascade/nope").await;
    assert_eq!((status, reply["error"].as_str().unwrap()), (404, "unknown scenario or source 'nope'"));
    let (status, reply) = get(&base, "/mock/nope/x").await;
    assert_eq!((status, reply["error"].as_str().unwrap()), (404, "unknown scenario or source 'nope'"));
    let (status, reply) = get(&base, "/no/such/file.js").await;
    assert_eq!((status, reply["error"].as_str().unwrap()), (404, "not found"));

    // A source that cannot be fetched is reported on its card, not as an error.
    let (status, reply) = post_json(&base, "/api/action", json!({"action": "connect", "source": "nope"})).await;
    assert_eq!(status, 200);
    assert!(reply["state"]["audit"].as_array().unwrap().iter().any(|a| a["action"] == "connect-failed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_web_client_and_the_mocks_are_served() {
    let (base, app) = start(config(temp_cache("static"))).await;
    for (path, ctype) in [("/", "text/html"), ("/app.js", "text/javascript"), ("/style.css", "text/css")] {
        let response = client().get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers()["content-type"], ctype, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
    }

    let source = app.packs["fraud-ring"].source_order[0].clone();
    let declared = app.packs["fraud-ring"].source(&source).unwrap().clone();
    let started = std::time::Instant::now();
    let response = client().get(format!("{base}/mock/fraud-ring/{source}")).send().await.unwrap();
    assert!(started.elapsed().as_millis() as u64 >= declared.latency_ms, "the declared latency");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-source-format"], declared.format.as_str());
    assert_eq!(response.headers()["x-source-name"], declared.name.as_str());
    assert!(response.headers()["content-type"].to_str().unwrap().ends_with("; charset=utf-8"));
    assert_eq!(response.bytes().await.unwrap().as_ref(), std::fs::read(declared.path.as_ref().unwrap()).unwrap());

    // A raw request: hyper does not normalise `..`.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut raw = tokio::net::TcpStream::connect(base.trim_start_matches("http://")).await.unwrap();
    raw.write_all(b"GET /../server/app.py HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut answer = String::new();
    raw.read_to_string(&mut answer).await.unwrap();
    assert!(answer.starts_with("HTTP/1.1 404") || answer.starts_with("HTTP/1.1 400"), "{answer}");
    assert!(!answer.contains("ThreadingHTTPServer"));
}

/// A model reply that calls a tool.
fn tool_call(name: &str, input: Value) -> Value {
    json!({"content": [{"type": "tool_use", "id": "call-1", "name": name, "input": input}], "stop_reason": "tool_use", "usage": {"cost": 0.001}})
}

/// A local stand-in for OpenRouter that answers Jev and counts what it is asked.
struct Fake {
    url: String,
    jev_calls: Arc<AtomicUsize>,
    model_calls: Arc<AtomicUsize>,
    in_flight_peak: Arc<AtomicUsize>,
    /// The next model reply, once, in place of plain text.
    reply: Arc<Mutex<Option<Value>>>,
}

async fn fake_openrouter() -> Fake {
    let (jev_calls, model_calls, in_flight, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let reply: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let (jc, mc, peak2, next) = (jev_calls.clone(), model_calls.clone(), peak.clone(), reply.clone());
    let router = Router::new()
        .route(
            "/systemone",
            post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                let (jc, in_flight, peak) = (jc.clone(), in_flight.clone(), peak2.clone());
                async move {
                    jc.fetch_add(1, Ordering::SeqCst);
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer test-key");
                    assert_eq!(headers["x-title"], "ELESSAR");
                    assert_eq!(body["model"], "jev-latest");
                    assert!(body["state"]["source"].is_string() && body["state"]["record"].is_object());
                    let answers: serde_json::Map<String, Value> = body["questions"].as_object().unwrap().keys().map(|k| (k.clone(), json!({"noul": 0.9}))).collect();
                    Json(json!({"answers": answers, "model": "jev-fake-1", "usage": {"cost": 0.001}}))
                }
            }),
        )
        .route(
            "/messages",
            post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                let (mc, next) = (mc.clone(), next.clone());
                async move {
                    mc.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer test-key");
                    assert_eq!(headers["x-title"], "ELESSAR");
                    assert_eq!(body["model"], "test-model");
                    let scripted = next.lock().unwrap().take();
                    if let Some(reply) = scripted {
                        return Json(reply);
                    }
                    Json(json!({"content": [{"type": "text", "text": "Fake reply."}], "stop_reason": "end_turn", "usage": {"cost": 0.002}}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Fake { url, jev_calls, model_calls, in_flight_peak: peak, reply }
}

fn jev_on(cache: PathBuf, fake: &Fake) -> Config {
    Config { api_key: Some("test-key".into()), jev_endpoint: format!("{}/systemone", fake.url), messages_endpoint: format!("{}/messages", fake.url), ..config(cache) }
}

/// Live asks, the cache that follows, the inspector and its operations.
#[tokio::test(flavor = "multi_thread")]
async fn jev_asks_cache_and_forget() {
    let cache = temp_cache("jev");
    let fake = fake_openrouter().await;
    let (base, app) = start(jev_on(cache.clone(), &fake)).await;
    let pid = "fraud-ring";

    let (_, report) = get(&base, &format!("/api/jev/cache?scenario={pid}")).await;
    assert_eq!(report["available"], true);
    let signal = report["signals"][0].clone();
    let asks = signal["records"].as_array().unwrap().len();
    assert!(asks > 8, "more than the workers, to see the limit: {asks}");
    assert_eq!(signal["connected"], false, "the inspector covers unconnected sources");
    let source = signal["source"].as_str().unwrap().to_string();

    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "connect", "source": source})).await;
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), asks);
    assert!(fake.in_flight_peak.load(Ordering::SeqCst) <= 8, "at most 8 at once");
    assert_eq!(reply["state"]["jev"]["calls"], asks as u64);
    assert_eq!(reply["state"]["jev"]["cached"], 0);
    assert!(reply["state"]["audit"].as_array().unwrap().iter().any(|a| a["action"] == "jev" && a["detail"].as_str().unwrap().contains(&format!("{asks} calls, 0 cached, 0 errors"))));

    // One action: the audit reads connect, jev, insights, as Python writes it.
    let actions: Vec<&str> = reply["state"]["audit"].as_array().unwrap().iter().map(|a| a["action"].as_str().unwrap()).collect();
    let (connect, jev) = (actions.iter().position(|a| *a == "connect").unwrap(), actions.iter().position(|a| *a == "jev").unwrap());
    assert!(connect < jev, "{actions:?}");
    assert!(actions[connect + 1..jev].iter().all(|a| *a == "skipped"), "{actions:?}");

    // The files have Python's shape, and name themselves.
    let (_, report) = get(&base, &format!("/api/jev/cache?scenario={pid}")).await;
    assert_eq!(report["files"], asks as u64);
    assert_eq!(report["orphans"], json!([]));
    let first = &report["signals"][0];
    assert_eq!((first["cached"].as_u64().unwrap() as usize, first["live"].as_u64().unwrap()), (asks, 0));
    assert_eq!(first["records"][0]["cached"]["noul"], 0.9);
    assert_eq!(first["records"][0]["cached"]["model"], "jev-fake-1");
    assert_eq!(first["records"][0]["cached"]["older_version"], false);
    assert_eq!(first["records"][0]["cached"]["legacy"], false);
    let path = PathBuf::from(first["records"][0]["path"].as_str().unwrap());
    let stored = elessar_core::jevcache::load_cached(&path).unwrap();
    assert_eq!(stored.as_object().unwrap().keys().collect::<Vec<_>>(), ["payload", "answers", "model", "ms", "cost", "at"]);
    assert_eq!(elessar_core::jevcache::cache_path(&cache, &stored["payload"]), path);

    // The Jev toggle now reads answers.
    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "engine", "engine": "jev"})).await;
    assert_eq!(reply["state"]["engine"], "jev");
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), asks, "answers are known: nothing to ask");
    let node = reply["state"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == signal["id"]).unwrap();
    assert_eq!(node["detail"]["engine_used"], "jev");

    // A second session answers from the cache, for free.
    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "reset"})).await;
    assert_eq!(reply["state"]["jev"]["calls"], 0);
    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "connect", "source": source})).await;
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), asks);
    assert_eq!((reply["state"]["jev"]["cached"].as_u64(), reply["state"]["jev"]["calls"].as_u64()), (Some(asks as u64), Some(0)));

    // forget: the files go, the answers go, nothing asked.
    let (status, reply) = post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "forget", "signal": signal["id"]})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["report"]["files"], 0);
    assert_eq!(reply["report"]["signals"][0]["live"].as_u64().unwrap() as usize, asks);
    assert!(reply["state"]["audit"].as_array().unwrap().iter().any(|a| a["action"] == "jev-cache" && a["detail"].as_str().unwrap().starts_with(&format!("forgot {asks} cached answer(s) for "))));
    let node = reply["state"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == signal["id"]).unwrap();
    assert_eq!(node["detail"]["jev"]["asked"], false);
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), asks);

    // reask: asked again right away for the connected source.
    let (status, reply) = post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "reask"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), asks * 2);
    let node = reply["state"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == signal["id"]).unwrap();
    assert_eq!(node["detail"]["jev"]["asked"], true, "the session holds the new answers");
    assert_eq!(reply["report"]["files"], asks as u64);

    // Orphans: a file no pack asks for, with a payload, and a legacy file without one.
    std::fs::write(cache.join("00orphan.json"), r#"{"payload": {"model": "m", "state": {}, "questions": {"q": {"type": "noul", "instructions": "An old question"}}}, "answers": {}, "model": "m", "ms": 1, "cost": 0.0, "at": "2026-01-01T00:00:00Z"}"#).unwrap();
    std::fs::write(cache.join("00legacy.json"), r#"{"answers": {}, "model": "m", "ms": 1, "cost": 0.0}"#).unwrap();
    let (_, report) = get(&base, &format!("/api/jev/cache?scenario={pid}")).await;
    assert_eq!(report["orphans"].as_array().unwrap().len(), 2);
    assert_eq!(report["orphan_questions"], json!([{"question": "An old question", "model": "m", "at": "2026-01-01T00:00:00Z"}]));
    let (_, reply) = post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "forget_orphans"})).await;
    assert_eq!(reply["report"]["orphans"], json!([]));
    assert_eq!(reply["report"]["files"], asks as u64);

    // forget_all: everything, in every session.
    let (_, reply) = post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "forget_all"})).await;
    assert_eq!(reply["report"]["files"], 0);
    assert!(app.jev.list_cache().is_empty());
    std::fs::remove_dir_all(&cache).unwrap();
}

/// A source that cannot be fetched asks nothing, and a good fetch still reaches Jev.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_fetch_asks_jev_nothing() {
    let cache = temp_cache("jev-nofetch");
    let fake = fake_openrouter().await;
    let (base, _app) = start(jev_on(cache.clone(), &fake)).await;
    let (_, report) = get(&base, "/api/jev/cache?scenario=fraud-ring").await;
    let source = report["signals"][0]["source"].as_str().unwrap().to_string();
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": "fraud-ring", "action": "connect", "source": "nope"})).await;
    assert_eq!(status, 200);
    assert_eq!(reply["state"]["jev"]["calls"], 0);
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), 0);
    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": "fraud-ring", "action": "connect", "source": source})).await;
    assert!(fake.jev_calls.load(Ordering::SeqCst) > 0);
    assert!(reply["state"]["jev"]["calls"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_dir_all(&cache);
}

/// A failed ask is recorded per record, and is not cached.
#[tokio::test(flavor = "multi_thread")]
async fn a_jev_failure_lands_on_the_record() {
    let cache = temp_cache("jev-fail");
    let fake = fake_openrouter().await;
    let mut cfg = jev_on(cache.clone(), &fake);
    cfg.jev_endpoint = format!("{}/nothing-here", fake.url);
    let (base, _app) = start(cfg).await;
    let (_, report) = get(&base, "/api/jev/cache?scenario=fraud-ring").await;
    let signal = report["signals"][0].clone();
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": "fraud-ring", "action": "connect", "source": signal["source"]})).await;
    assert_eq!(status, 200);
    assert_eq!(reply["state"]["jev"]["errors"].as_u64().unwrap() as usize, signal["records"].as_array().unwrap().len());
    let node = reply["state"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == signal["id"]).unwrap();
    assert!(node["detail"]["jev"]["records"][0]["error"].as_str().unwrap().starts_with("HTTP 404"));
    assert!(!cache.exists() || std::fs::read_dir(&cache).unwrap().next().is_none());
}

/// The agent's model call reaches the configured endpoint, and its turn comes back with the state.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_talks_through_the_backends() {
    let cache = temp_cache("agent");
    let fake = fake_openrouter().await;
    let (base, _app) = start(jev_on(cache.clone(), &fake)).await;

    let (status, reply) = post_json(&base, "/api/agent", json!({"message": "   "})).await;
    assert_eq!((status, reply["error"].as_str().unwrap()), (400, "empty message"));

    let (status, reply) = post_json(&base, "/api/agent", json!({"message": "What now?", "selected": null})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(fake.model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(reply["turn"]["text"], "Fake reply.");
    assert_eq!(reply["state"]["agent"]["available"], true);
    assert_eq!(reply["state"]["agent"]["model"], "test-model");
    assert_eq!(reply["state"]["agent"]["transcript"].as_array().unwrap().len(), 2);
    assert!(reply["state"]["audit"].as_array().unwrap().iter().any(|a| a["action"] == "agent"));

    let (status, reply) = post_json(&base, "/api/action", json!({"action": "agent_reset"})).await;
    assert_eq!(status, 200);
    assert_eq!(reply["state"]["agent"]["transcript"], json!([]));
    assert_eq!(reply["events"], json!([]));

    // A 200 whose body is an error is a failure, not an empty reply.
    *fake.reply.lock().unwrap() = Some(json!({"error": {"message": "Provider returned error", "code": 502}}));
    let (status, reply) = post_json(&base, "/api/agent", json!({"message": "Again?"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["turn"]["text"], "The agent couldn't reach the model: Provider returned error");
}

/// A what-if may connect a source the session has never fetched: the agent
/// fetches it through the server, and the real session stays as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_what_if_fetches_a_source_never_connected() {
    let cache = temp_cache("what-if");
    let fake = fake_openrouter().await;
    let (base, _app) = start(jev_on(cache.clone(), &fake)).await;
    let change = json!({"changes": [{"action": "connect", "source": "advisory"}]});
    *fake.reply.lock().unwrap() = Some(tool_call("what_if", change));
    let (status, reply) = post_json(&base, "/api/agent", json!({"message": "Which source next?"})).await;
    assert_eq!(status, 200, "{reply}");
    let tools = reply["turn"]["tools"].as_array().unwrap();
    assert_eq!((tools[0]["name"].as_str(), tools[0]["error"].as_bool()), (Some("what_if"), Some(false)), "{tools:?}");
    assert_eq!(fake.model_calls.load(Ordering::SeqCst), 2, "the tool result went back to the model");
    assert!(reply["state"]["sources"].as_array().unwrap().iter().all(|s| s["connected"] == false), "only a copy was connected");
    let _ = std::fs::remove_dir_all(&cache);
}

/// Approving and declining what the agent proposes: the action applies, its
/// insights come back, and what `Session::act` cannot do (fetching, asking Jev) happens around it.
#[tokio::test(flavor = "multi_thread")]
async fn agent_proposals_are_decided_over_http() {
    let cache = temp_cache("agent-decide");
    let fake = fake_openrouter().await;
    let (base, _app) = start(jev_on(cache.clone(), &fake)).await;
    let pid = "fraud-ring";
    let (_, report) = get(&base, &format!("/api/jev/cache?scenario={pid}")).await;
    let source = report["signals"][0]["source"].as_str().unwrap().to_string();

    // What the proposal's id is, once the agent has made it.
    async fn propose(base: &str, fake: &Fake, input: Value) -> String {
        *fake.reply.lock().unwrap() = Some(tool_call("propose_action", input));
        let (status, reply) = post_json(base, "/api/agent", json!({"scenario": "fraud-ring", "message": "Well?"})).await;
        assert_eq!(status, 200, "{reply}");
        reply["turn"]["proposals"][0].as_str().unwrap().to_string()
    }

    // A connect with no payload yet: the server fetches it and asks Jev.
    let id = propose(&base, &fake, json!({"action": "connect", "source": source, "reason": "It would help."})).await;
    assert_eq!(fake.jev_calls.load(Ordering::SeqCst), 0, "nothing happens until the user approves");
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "approve"})).await;
    assert_eq!(status, 200, "{reply}");
    let card = reply["state"]["sources"].as_array().unwrap().iter().find(|s| s["id"] == source.as_str()).unwrap();
    assert_eq!(card["connected"], true);
    assert_eq!(reply["state"]["agent"]["proposals"][&id]["status"], "approved");
    assert!(fake.jev_calls.load(Ordering::SeqCst) > 0, "Jev was asked about the fetched source");
    assert!(reply["events"].is_array());
    let lines: Vec<String> = reply["state"]["audit"].as_array().unwrap().iter().map(|a| format!("{}: {}", a["action"].as_str().unwrap(), a["detail"].as_str().unwrap())).collect();
    let (said, done) = (lines.iter().position(|l| *l == format!("agent: you approved the agent's proposal to connect {source}")), lines.iter().position(|l| l.starts_with("connect: ")));
    assert!(said.is_some() && done.is_some() && said < done, "the decision is logged first: {lines:?}");

    // Decided already: nothing left to decide.
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "approve"})).await;
    assert_eq!((status, reply["error"].as_str().unwrap()), (400, "no pending proposal with that id"));

    // Declined: logged, nothing applied.
    let id = propose(&base, &fake, json!({"action": "crown", "hypothesis": "H2", "reason": "Looks right."})).await;
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "decline"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["state"]["crown"], Value::Null);
    assert_eq!(reply["events"], json!([]));
    assert_eq!(reply["state"]["agent"]["proposals"][&id]["status"], "declined");
    assert!(reply["state"]["audit"].as_array().unwrap().iter().any(|a| a["action"] == "agent" && a["detail"].as_str().unwrap().starts_with("you declined the agent's proposal to ")));

    // Approved: the action's insights come back for the toast.
    let id = propose(&base, &fake, json!({"action": "crown", "hypothesis": "H2", "reason": "Looks right."})).await;
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "approve"})).await;
    assert_eq!(status, 200, "{reply}");
    assert!(!reply["state"]["crown"].is_null());
    assert!(reply["events"].as_array().unwrap().iter().any(|e| e["kind"].as_str().unwrap().starts_with("crowned")), "{}", reply["events"]);

    // An approved engine switch asks Jev what is still unanswered, here after a forget.
    let (status, reply) = post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "forget"})).await;
    assert_eq!(status, 200, "{reply}");
    let before = fake.jev_calls.load(Ordering::SeqCst);
    let id = propose(&base, &fake, json!({"action": "engine", "engine": "jev", "reason": "Compare."})).await;
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "approve"})).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["state"]["engine"], "jev");
    assert!(fake.jev_calls.load(Ordering::SeqCst) > before, "Jev was asked after the approved switch");

    // An approved connect of a source with a kept payload fetches nothing and still asks Jev.
    post_json(&base, "/api/action", json!({"scenario": pid, "action": "disconnect", "source": source})).await;
    post_json(&base, "/api/jev/cache", json!({"scenario": pid, "op": "forget"})).await;
    let before = fake.jev_calls.load(Ordering::SeqCst);
    let id = propose(&base, &fake, json!({"action": "connect", "source": source, "reason": "Back."})).await;
    let (status, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "agent_decide", "proposal": id, "decision": "approve"})).await;
    assert_eq!(status, 200, "{reply}");
    assert!(fake.jev_calls.load(Ordering::SeqCst) > before, "Jev was asked after the approved connect");
    let card = reply["state"]["sources"].as_array().unwrap().iter().find(|s| s["id"] == source.as_str()).unwrap();
    assert_eq!(card["connected"], true);

    // A full reset forgets the conversation, as Python's does.
    let (_, reply) = post_json(&base, "/api/action", json!({"scenario": pid, "action": "reset"})).await;
    assert_eq!(reply["state"]["agent"]["transcript"], json!([]));
    assert_eq!(reply["state"]["agent"]["proposals"], json!({}));
    let _ = std::fs::remove_dir_all(&cache);
}
