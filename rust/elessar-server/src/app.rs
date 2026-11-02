//! One session per pack, the actions, and the Jev cache inspector.
//!
//! The engine is synchronous and fetching is not, so one action holds two locks
//! at different scales. `gate` (async) serialises the actions of a pack from
//! start to finish, as the Python session's lock does. `slot` (blocking) is taken
//! only around engine work on the blocking pool, never across a fetch or a Jev
//! request, so reads stay answerable while a connect waits for the network.
//! The agent's tool loop is the exception: its signature takes the session for
//! the whole turn, so `thinking` keeps the last state for readers until it ends.

use crate::agent::{Agent, Backends};
use crate::config::Config;
use crate::jev::JevClient;
use crate::packcheck;
use elessar_core::pack::Pack;
use elessar_core::pipeline;
use elessar_core::session::{JevJob, JevResult, Session};
use elessar_core::timeline::timeline_ref;
use elessar_core::types::{Insight, State};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const MESSAGES_TIMEOUT: Duration = Duration::from_secs(120);

/// A failed request, with the status and message `server/app.py` would send.
#[derive(Debug, PartialEq)]
pub enum ApiError {
    /// 400 `{"error": ...}`
    BadRequest(String),
    /// 404 `{"error": ...}`
    NotFound(String),
    /// 500 `{"error": ...}`: a panic in the engine or a failed serialisation.
    Internal(String),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in the engine fails its own request. The next one recomputes from
    // decisions, so the poison carries no information worth refusing service for.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a pack's session owns.
pub struct Slot {
    pub session: Session,
    pub agent: Agent,
}

/// `state` plus the part the engine does not know about.
#[derive(Serialize)]
struct PublicState<'a> {
    #[serde(flatten)]
    state: &'a State,
    agent: Value,
}

fn public(slot: &Slot) -> PublicState<'_> {
    PublicState { state: slot.session.state(), agent: slot.agent.view() }
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|e| ApiError::Internal(e.to_string()))
}

/// One pack's session and its locks (see the module comment).
pub struct PackEntry {
    gate: tokio::sync::Mutex<()>,
    slot: Arc<Mutex<Slot>>,
    thinking: Mutex<Option<Arc<Vec<u8>>>>,
}

impl PackEntry {
    /// Run engine work on the blocking pool with the session locked.
    async fn with<R: Send + 'static>(&self, work: impl FnOnce(&mut Slot) -> R + Send + 'static) -> Result<R, ApiError> {
        let slot = self.slot.clone();
        tokio::task::spawn_blocking(move || work(&mut lock(&slot))).await.map_err(|e| ApiError::Internal(format!("the engine failed: {e}")))
    }

    async fn respond(&self, events: Vec<Insight>) -> Result<Vec<u8>, ApiError> {
        #[derive(Serialize)]
        struct Response<'a> {
            state: PublicState<'a>,
            events: &'a [Insight],
        }
        self.with(move |slot| to_json(&Response { state: public(slot), events: &events })).await?
    }
}

/// The payload of a source over HTTP, with how long it took in ms. File
/// sources come from this server's own `/mock`, reached through `local`.
async fn fetch_payload(http: &reqwest::Client, local: &reqwest::Client, base_url: &str, pack: &Pack, sid: &str) -> Result<(String, u64), String> {
    let source = pack.source(sid).ok_or_else(|| format!("unknown source '{sid}'"))?;
    let (client, url) = match (source.transport.as_str(), &source.url) {
        ("http", Some(url)) => (http, url.clone()),
        _ => (local, format!("{base_url}/mock/{}/{sid}", pack.id)),
    };
    let started = Instant::now();
    let response = client.get(&url).timeout(FETCH_TIMEOUT).send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP Error {}: {}", status.as_u16(), status.canonical_reason().unwrap_or("")));
    }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    let raw = String::from_utf8(body.to_vec()).map_err(|e| e.to_string())?;
    Ok((raw, started.elapsed().as_millis() as u64))
}

/// A model response as JSON. OpenRouter can answer 200 with an `error` object
/// in the body, which is a failure: treated as a reply it would read as empty.
fn model_reply(text: &str) -> Result<Value, String> {
    let reply: Value = serde_json::from_str(text).map_err(|e| format!("The agent couldn't reach the model: {e}"))?;
    match reply.get("error").filter(|e| !e.is_null()) {
        Some(error) => {
            let message = error.get("message").and_then(Value::as_str).map_or_else(|| error.to_string(), String::from);
            Err(format!("The agent couldn't reach the model: {message}"))
        }
        None => Ok(reply),
    }
}

/// The agent's window on the network.
struct ServerBackends {
    http: reqwest::Client,
    local: reqwest::Client,
    endpoint: String,
    key: String,
    jev: Arc<JevClient>,
    /// The agent runs on the blocking pool; requests run on the runtime.
    handle: Handle,
}

impl Backends for ServerBackends {
    fn messages(&self, body: &Value) -> Result<Value, String> {
        let request = self.http.post(&self.endpoint).bearer_auth(&self.key).header("Content-Type", "application/json").header("X-Title", "ELESSAR").timeout(MESSAGES_TIMEOUT).body(body.to_string());
        self.handle.block_on(async {
            let response = request.send().await.map_err(|e| format!("The agent couldn't reach the model: {e}"))?;
            let status = response.status();
            let text = response.text().await.map_err(|e| format!("The agent couldn't reach the model: {e}"))?;
            if !status.is_success() {
                let detail: String = text.chars().take(300).collect();
                return Err(format!("The agent couldn't reach the model (HTTP {}): {detail}", status.as_u16()));
            }
            model_reply(&text)
        })
    }

    fn jev(&self, state: &Value, questions: &Value) -> Result<JevResult, String> {
        if !self.jev.available() {
            return Err("Jev is off: no OPENROUTER_API_KEY".into());
        }
        Ok(self.handle.block_on(self.jev.ask(state, questions)))
    }

    fn jev_available(&self) -> bool {
        self.jev.available()
    }

    fn jev_model(&self) -> String {
        self.jev.model.clone()
    }

    fn fetch_source(&self, session: &Session, source: &str) -> Result<(String, u64), String> {
        self.handle.block_on(fetch_payload(&self.http, &self.local, &session.base_url, &session.pack, source))
    }
}

pub struct App {
    pub config: Config,
    pub packs: IndexMap<String, Pack>,
    pub failures: IndexMap<String, Vec<String>>,
    pub default: String,
    pub jev: Arc<JevClient>,
    /// For the outside world: honours the proxy settings.
    http: reqwest::Client,
    /// For this server's own `/mock`, which no proxy can reach.
    local: reqwest::Client,
    base_url: OnceLock<String>,
    entries: Mutex<HashMap<String, Arc<PackEntry>>>,
}

impl App {
    /// `Err` when the default scenario is not among the packs, in Python's words.
    pub fn new(config: Config) -> Result<Arc<App>, String> {
        let (packs, failures) = packcheck::discover(&config.scenarios_dir);
        if !packs.contains_key(&config.default_scenario) {
            let mut ids: Vec<String> = packs.keys().map(|id| format!("'{id}'")).collect();
            ids.sort_unstable();
            return Err(format!("scenario '{}' not found or invalid. Available: [{}]", config.default_scenario, ids.join(", ")));
        }
        let http = reqwest::Client::builder().build().map_err(|e| e.to_string())?;
        let local = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
        let jev = Arc::new(JevClient::new(http.clone(), config.jev_endpoint.clone(), config.api_key.clone(), config.decision_model.clone(), config.jev_cache_dir.clone()));
        Ok(Arc::new(App { default: config.default_scenario.clone(), config, packs, failures, jev, http, local, base_url: OnceLock::new(), entries: Mutex::new(HashMap::new()) }))
    }

    /// Where this server listens, for its own `/mock` URLs. Set once, before serving.
    pub fn set_base_url(&self, url: String) {
        let _ = self.base_url.set(url);
    }

    pub fn base_url(&self) -> &str {
        self.base_url.get().map_or("", String::as_str)
    }

    /// The pack's session, created on first use. `None` for an unknown pack.
    pub fn entry(&self, pid: &str) -> Option<Arc<PackEntry>> {
        let pack = self.packs.get(pid)?;
        let mut entries = lock(&self.entries);
        if let Some(entry) = entries.get(pid) {
            return Some(entry.clone());
        }
        let mut session = Session::new(pack.clone());
        session.base_url = self.base_url().to_string();
        session.set_jev_status(self.jev.available(), &self.jev.model);
        let agent = Agent::new(self.config.api_key.as_ref().map(|_| self.config.inference_model.clone()));
        let entry = Arc::new(PackEntry { gate: tokio::sync::Mutex::new(()), slot: Arc::new(Mutex::new(Slot { session, agent })), thinking: Mutex::new(None) });
        entries.insert(pid.to_string(), entry.clone());
        Some(entry)
    }

    fn scenario_of<'a>(&'a self, payload: &'a Value) -> Result<&'a str, ApiError> {
        match payload.get("scenario") {
            None => Ok(&self.default),
            Some(Value::String(pid)) => Ok(pid),
            Some(other) => Err(ApiError::BadRequest(format!("missing or unknown field {other}"))),
        }
    }

    /// The entry a POST names. An unknown one is 400, in `KeyError`'s words.
    fn posted_entry(&self, payload: &Value) -> Result<Arc<PackEntry>, ApiError> {
        let pid = self.scenario_of(payload)?;
        self.entry(pid).ok_or_else(|| ApiError::BadRequest(format!("missing or unknown field '{pid}'")))
    }

    /// The entry a GET names. An unknown one is 404.
    fn queried_entry(&self, pid: &str) -> Result<Arc<PackEntry>, ApiError> {
        self.entry(pid).ok_or_else(|| unknown(pid))
    }

    // ------------------------------------------------------------ reads

    /// `GET /api/scenarios`
    pub fn scenarios(&self) -> Value {
        let scenarios: Vec<Value> = self
            .packs
            .values()
            .map(|p| json!({"id": p.id, "title": p.title, "premise": p.premise, "sources": p.sources.len(), "pieces": p.layout.nodes.len(), "warnings": p.warnings}))
            .collect();
        json!({"default": self.default, "scenarios": scenarios, "invalid": self.failures})
    }

    /// `GET /api/state`
    pub async fn state(&self, pid: &str) -> Result<Vec<u8>, ApiError> {
        let entry = self.queried_entry(pid)?;
        let thinking = lock(&entry.thinking).clone();
        if let Some(snapshot) = thinking {
            return Ok(snapshot.as_ref().clone());
        }
        entry.with(|slot| to_json(&public(slot))).await?
    }

    /// `GET /api/timeline`
    pub async fn timeline(&self, pid: &str) -> Result<Vec<u8>, ApiError> {
        self.queried_entry(pid)?.with(|slot| to_json(timeline_ref(&mut slot.session))).await?
    }

    /// `GET /mock/<pack>/<source>`: the source, or a 404.
    pub fn mock_source(&self, pid: &str, sid: &str) -> Result<&elessar_core::pack::Source, ApiError> {
        let pack = self.packs.get(pid).ok_or_else(|| unknown(pid))?;
        let source = pack.source(sid).ok_or_else(|| unknown(sid))?;
        if source.transport != "file" {
            return Err(ApiError::NotFound(format!("source '{sid}' is not served locally")));
        }
        Ok(source)
    }

    // ------------------------------------------------------------ fetching and connecting

    async fn fetch(&self, pid: &str, sid: &str) -> Result<(String, u64), String> {
        let pack = self.packs.get(pid).ok_or_else(|| format!("unknown source '{sid}'"))?;
        fetch_payload(&self.http, &self.local, self.base_url(), pack, sid).await
    }

    /// Fetch a source, ask Jev about it, connect it: answers and all in one
    /// step of the session, so one recompute and one insight diff follow, as in Python.
    async fn connect(&self, entry: &PackEntry, pid: &str, sid: &str, events: &mut Vec<Insight>) -> Result<(), ApiError> {
        let source = sid.to_string();
        let insights = match self.fetch(pid, sid).await {
            Ok((raw, ms)) => {
                let results = self.ask_jev_about(entry, pid, sid, raw.clone()).await?;
                entry.with(move |slot| slot.session.connect_with_jev(&source, raw, ms, &results)).await?
            }
            Err(e) => entry.with(move |slot| slot.session.connect_failed(&source, &e)).await?,
        };
        events.extend(insights);
        Ok(())
    }

    /// What Jev says about a payload about to be connected: every opted-in
    /// question not yet answered. Empty when nothing is to be asked, Jev is off,
    /// or the payload does not parse (the connect will report that on the card).
    async fn ask_jev_about(&self, entry: &PackEntry, pid: &str, sid: &str, raw: String) -> Result<Vec<(u32, JevResult)>, ApiError> {
        let Some(pack) = self.packs.get(pid).filter(|_| self.jev.available()) else { return Ok(Vec::new()) };
        let Some(src) = pack.source(sid).filter(|_| pack.signals.iter().any(|s| s.jev.is_some() && s.source.as_deref() == Some(sid))).cloned() else { return Ok(Vec::new()) };
        let source = sid.to_string();
        let jobs: Vec<JevJob> = entry
            .with(move |slot| {
                let session = &slot.session;
                match session.table(&source).filter(|table| table.raw == raw) {
                    Some(table) => session.jev_jobs(&source, table, true),
                    None => pipeline::parse(&src, raw).map_or_else(|_| Vec::new(), |table| session.jev_jobs(&source, &table, true)),
                }
            })
            .await?;
        Ok(if jobs.is_empty() { Vec::new() } else { self.jev.ask_many(jobs).await })
    }

    /// Ask Jev every opted-in question not yet answered about one connected source.
    async fn ask_jev(&self, entry: &PackEntry, sid: &str, events: &mut Vec<Insight>) -> Result<(), ApiError> {
        if !self.jev.available() {
            return Ok(());
        }
        let source = sid.to_string();
        let jobs: Vec<JevJob> = entry
            .with(move |slot| {
                let session = &slot.session;
                session.table(&source).filter(|_| session.is_connected(&source)).map_or_else(Vec::new, |table| session.jev_jobs(&source, table, true))
            })
            .await?;
        if jobs.is_empty() {
            return Ok(());
        }
        let results = self.jev.ask_many(jobs).await;
        let source = sid.to_string();
        events.extend(entry.with(move |slot| slot.session.apply_jev_results(&source, &results)).await?);
        Ok(())
    }

    async fn ask_jev_connected(&self, entry: &PackEntry, events: &mut Vec<Insight>) -> Result<(), ApiError> {
        for sid in entry.with(|slot| slot.session.connected_sources()).await? {
            self.ask_jev(entry, &sid, events).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ actions

    /// `POST /api/action` → `{state, events}`
    pub async fn action(&self, payload: Value) -> Result<Vec<u8>, ApiError> {
        let entry = self.posted_entry(&payload)?;
        let pid = self.scenario_of(&payload)?.to_string();
        let _gate = entry.gate.lock().await;
        let mut events = Vec::new();
        match payload.get("action").and_then(Value::as_str) {
            Some("connect") => {
                let sid = required(&payload, "source")?;
                self.connect(&entry, &pid, sid, &mut events).await?;
            }
            Some("reconnect") => {
                let list = payload.get("sources").and_then(Value::as_array).ok_or_else(|| missing("sources"))?;
                let sources: Vec<String> = list.iter().filter_map(|s| s.as_str().map(String::from)).collect();
                for sid in sources {
                    let source = sid.clone();
                    if !entry.with(move |slot| slot.session.is_connected(&source)).await? {
                        self.connect(&entry, &pid, &sid, &mut events).await?;
                    }
                }
            }
            Some("engine") => {
                let call = payload.clone();
                events.extend(entry.with(move |slot| slot.session.act(&call)).await?.map_err(ApiError::BadRequest)?);
                if payload.get("engine").and_then(Value::as_str) == Some("jev") {
                    self.ask_jev_connected(&entry, &mut events).await?;
                }
            }
            Some("agent_reset") => entry.with(|slot| slot.agent.reset()).await?,
            Some("agent_decide") => self.agent_decide(&entry, &pid, &payload, &mut events).await?,
            Some("reset") => {
                let call = payload.clone();
                events.extend(
                    entry
                        .with(move |slot| {
                            slot.agent.reset();
                            slot.session.act(&call)
                        })
                        .await?
                        .map_err(ApiError::BadRequest)?,
                );
            }
            None => return Err(ApiError::BadRequest("unknown action 'None'".into())),
            Some(_) => {
                let call = payload.clone();
                events.extend(entry.with(move |slot| slot.session.act(&call)).await?.map_err(ApiError::BadRequest)?);
            }
        }
        entry.respond(events).await
    }

    /// `agent_decide`. `Agent::decide` applies an approved action through
    /// `Session::act`, which neither fetches nor asks Jev, so those two steps
    /// follow it: `fetch` in its answer names a source it could not bring back.
    /// The audit line comes first, as Python writes it.
    async fn agent_decide(&self, entry: &PackEntry, pid: &str, payload: &Value, events: &mut Vec<Insight>) -> Result<(), ApiError> {
        let id = required(payload, "proposal")?.to_string();
        let decision = required(payload, "decision")?.to_string();
        let prop = entry
            .with(move |slot| {
                let Slot { session, agent } = slot;
                let pending = agent.view()["proposals"].get(&id).filter(|p| p["status"] == "pending").cloned();
                if let Some(p) = pending {
                    let status = if decision == "approve" { "approved" } else { "declined" };
                    session.log("agent", &format!("you {status} the agent's proposal to {}", p["label"].as_str().unwrap_or("")));
                }
                agent.decide(session, &id, &decision)
            })
            .await?
            .map_err(ApiError::BadRequest)?;
        if let Ok(earned) = serde_json::from_value::<Vec<InsightIn>>(prop["events"].clone()) {
            events.extend(earned.into_iter().map(Insight::from));
        }
        if prop["status"] != "approved" {
            return Ok(());
        }
        let applied = &prop["payload"];
        match (applied["action"].as_str(), prop["fetch"].as_str(), applied["source"].as_str()) {
            (_, Some(sid), _) => self.connect(entry, pid, sid, events).await?,
            (Some("connect"), None, Some(sid)) => self.ask_jev(entry, sid, events).await?,
            (Some("engine"), _, _) if applied["engine"] == "jev" => self.ask_jev_connected(entry, events).await?,
            _ => {}
        }
        Ok(())
    }

    /// `POST /api/agent {scenario, message, selected}` → `{turn, state}`
    pub async fn ask_agent(&self, payload: Value) -> Result<Vec<u8>, ApiError> {
        let entry = self.posted_entry(&payload)?;
        let Some(key) = self.config.api_key.clone() else {
            return Err(ApiError::BadRequest("The agent is off: add OPENROUTER_API_KEY to .env".into()));
        };
        let message = match payload.get("message") {
            Some(Value::String(text)) => text.trim().to_string(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        if message.is_empty() {
            return Err(ApiError::BadRequest("empty message".into()));
        }
        let selected = payload.get("selected").and_then(Value::as_str).map(String::from);
        let _gate = entry.gate.lock().await;
        let snapshot = entry.with(|slot| to_json(&public(slot))).await??;
        *lock(&entry.thinking) = Some(Arc::new(snapshot));
        let backends = ServerBackends { http: self.http.clone(), local: self.local.clone(), endpoint: self.config.messages_endpoint.clone(), key, jev: self.jev.clone(), handle: Handle::current() };
        let answered = entry
            .with(move |slot| {
                let Slot { session, agent } = &mut *slot;
                let turn = agent.ask(session, &message, selected.as_deref(), &backends);
                #[derive(Serialize)]
                struct Response<'a> {
                    turn: Value,
                    state: PublicState<'a>,
                }
                to_json(&Response { turn, state: public(slot) })
            })
            .await;
        *lock(&entry.thinking) = None;
        answered?
    }

    // ------------------------------------------------------------ the Jev cache inspector

    /// `GET /api/jev/cache`
    pub async fn jev_cache(&self, app: Arc<App>, pid: &str) -> Result<Vec<u8>, ApiError> {
        self.queried_entry(pid)?;
        to_json(&report_of(app, pid.to_string()).await?)
    }

    /// `POST /api/jev/cache {scenario, op, signal?}` → `{report, state}`
    pub async fn jev_cache_op(&self, app: Arc<App>, payload: Value) -> Result<Vec<u8>, ApiError> {
        let entry = self.posted_entry(&payload)?;
        let pid = self.scenario_of(&payload)?.to_string();
        let _gate = entry.gate.lock().await;
        let op = payload.get("op").and_then(Value::as_str);
        match op {
            Some("forget") | Some("reask") => {
                let signal = payload.get("signal").and_then(Value::as_str).map(String::from);
                self.forget_jev(&entry, signal, op == Some("reask")).await?;
            }
            Some("forget_orphans") => {
                let report = report_of(app.clone(), pid.clone()).await?;
                let orphans: Vec<PathBuf> = report["orphans"].as_array().map_or_else(Vec::new, |o| o.iter().filter_map(Value::as_str).map(PathBuf::from).collect());
                self.jev.forget(orphans.iter().map(PathBuf::as_path));
            }
            Some("forget_all") => {
                let every: Vec<PathBuf> = self.jev.list_cache().into_iter().map(|e| e.path).collect();
                self.jev.forget(every.iter().map(PathBuf::as_path));
                let entries: Vec<Arc<PackEntry>> = lock(&self.entries).values().cloned().collect();
                for each in entries {
                    each.with(|slot| {
                        slot.session.forget_jev_answers(|_| true);
                        slot.session.refresh();
                    })
                    .await?;
                }
            }
            other => return Err(ApiError::BadRequest(format!("unknown op '{}'", other.unwrap_or("None")))),
        }
        let report = report_of(app, pid).await?;
        #[derive(Serialize)]
        struct Response<'a> {
            report: Value,
            state: PublicState<'a>,
        }
        entry.with(move |slot| to_json(&Response { report, state: public(slot) })).await?
    }

    /// Drop cached answers (for one signal's asks, or all of this pack's), and
    /// optionally ask again right away for connected sources.
    async fn forget_jev(&self, entry: &PackEntry, signal: Option<String>, reask: bool) -> Result<(), ApiError> {
        let jev = self.jev.clone();
        let again: Vec<String> = entry
            .with(move |slot| -> Result<Vec<String>, String> {
                let inv = inventory(&slot.session, &jev);
                let signals = inv.json["signals"].as_array().map_or(&[][..], Vec::as_slice);
                let targets: Vec<&Value> = signals.iter().filter(|s| signal.as_deref().is_none_or(|id| s["id"] == id)).collect();
                if let Some(id) = signal.as_deref().filter(|id| !id.is_empty() && targets.is_empty()) {
                    return Err(format!("no Jev signal {id}"));
                }
                let target_ids: HashSet<&str> = targets.iter().filter_map(|s| s["id"].as_str()).collect();
                let paths: HashSet<&Path> = inv.asks.iter().filter(|a| target_ids.contains(a.signal.as_str())).map(|a| a.path.as_path()).collect();
                let removed = jev.forget(paths.iter().copied());
                // A cache file holds every question asked about that record, so the
                // answers of all signals that share those asks go with it.
                let dropped: HashSet<String> = inv.asks.iter().filter(|a| paths.contains(a.path.as_path())).map(|a| format!("|{}|{}", a.source, a.index)).collect();
                slot.session.forget_jev_answers(|key| {
                    let cut = key.rfind('|').and_then(|end| key[..end].rfind('|'));
                    cut.is_some_and(|at| dropped.contains(&key[at..]))
                });
                let what = match targets.first().filter(|_| signal.is_some()) {
                    Some(t) => format!("{} {}", t["code"].as_str().unwrap_or(""), t["label"].as_str().unwrap_or("")),
                    None => "every Jev ask in this pack".to_string(),
                };
                slot.session.log("jev-cache", &format!("forgot {removed} cached answer(s) for {what}"));
                let mut sources: BTreeSet<String> = BTreeSet::new();
                for t in &targets {
                    if t["connected"] == true {
                        sources.extend(t["source"].as_str().map(String::from));
                    }
                }
                Ok(sources.into_iter().collect())
            })
            .await?
            .map_err(ApiError::BadRequest)?;
        if reask {
            let mut earned = Vec::new();
            for sid in again {
                self.ask_jev(entry, &sid, &mut earned).await?;
            }
        }
        entry.with(|slot| slot.session.refresh()).await
    }
}

/// `Insight` has no `Deserialize`: this reads one back from a proposal's `events`.
#[derive(serde::Deserialize)]
struct InsightIn {
    kind: String,
    title: String,
    text: String,
    points: u32,
}

impl From<InsightIn> for Insight {
    fn from(i: InsightIn) -> Insight {
        Insight { kind: i.kind, title: i.title, text: i.text, points: i.points }
    }
}

fn unknown(what: &str) -> ApiError {
    ApiError::NotFound(format!("unknown scenario or source '{what}'"))
}

fn missing(field: &str) -> ApiError {
    ApiError::BadRequest(format!("missing or unknown field '{field}'"))
}

fn required<'a>(payload: &'a Value, field: &str) -> Result<&'a str, ApiError> {
    payload.get(field).and_then(Value::as_str).ok_or_else(|| missing(field))
}

/// One ask of the inventory, for finding its record again.
struct Ask {
    signal: String,
    source: String,
    index: u32,
    path: PathBuf,
}

/// `jev_inventory`: every Jev ask this pack can make with its cache status.
/// Covers unconnected sources too.
struct Inventory {
    json: Value,
    asks: Vec<Ask>,
}

/// The requests one source would make, whether or not it is connected. A file
/// source that has never been fetched is read from disk.
fn all_jobs(session: &Session, sid: &str) -> Vec<JevJob> {
    if let Some(table) = session.table(sid) {
        return session.jev_jobs(sid, table, false);
    }
    let Some(source) = session.pack.source(sid).filter(|s| s.transport == "file") else { return Vec::new() };
    let Some(raw) = source.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    pipeline::parse(source, raw).map_or_else(|_| Vec::new(), |table| session.jev_jobs(sid, &table, false))
}

fn inventory(session: &Session, jev: &JevClient) -> Inventory {
    let pack = &session.pack;
    let codes: HashMap<&str, &str> = pack.layout.nodes.iter().map(|n| (n.id.as_str(), n.code.as_str())).collect();
    let code_of = |id: &str| codes.get(id).copied().unwrap_or(id).to_string();
    let mut jobs_of: HashMap<String, Vec<JevJob>> = HashMap::new();
    let mut files: HashMap<PathBuf, Option<Value>> = HashMap::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut versions: BTreeSet<String> = BTreeSet::new();
    let mut asks = Vec::new();
    let mut signals = Vec::new();
    for sig in &pack.signals {
        let (Some(spec), Some(sid)) = (&sig.jev, &sig.source) else { continue };
        let key = sig.id.replace('-', "_");
        let jobs = jobs_of.entry(sid.clone()).or_insert_with(|| all_jobs(session, sid));
        let mut records = Vec::new();
        for job in jobs.iter().filter(|j| j.signals.contains(&sig.id)) {
            let path = jev.cache_path(&job.state, &job.questions);
            let shown = path.display().to_string();
            paths.insert(shown.clone());
            let file = files.entry(path.clone()).or_insert_with(|| elessar_core::jevcache::load_cached(&path));
            let cached = file.as_ref().map(|c| {
                let model = c.get("model").cloned().unwrap_or(Value::Null);
                if let Some(m) = model.as_str().filter(|m| !m.is_empty()) {
                    versions.insert(m.to_string());
                }
                let noul = c.get("answers").and_then(|a| a.get(&key)).and_then(|a| a.get("noul")).cloned().unwrap_or(Value::Null);
                json!({"noul": noul, "model": model, "at": c.get("at"), "ms": c.get("ms"), "legacy": c.get("payload").is_none()})
            });
            let excerpt: String = job.excerpt.chars().take(200).collect();
            let asked_with: Vec<String> = job.signals.iter().filter(|i| **i != sig.id).map(|i| code_of(i)).collect();
            records.push(json!({"pointer": job.pointer, "excerpt": excerpt, "path": shown, "asked_with": asked_with, "cached": cached}));
            asks.push(Ask { signal: sig.id.clone(), source: sid.clone(), index: job.index, path });
        }
        let n_cached = records.iter().filter(|r| !r["cached"].is_null()).count();
        signals.push(json!({
            "id": sig.id,
            "code": code_of(&sig.id),
            "label": sig.label.as_deref().unwrap_or(&sig.id),
            "source": sid,
            "source_name": pack.source(sid).map_or("", |s| s.name.as_str()),
            "connected": session.is_connected(sid),
            "question": spec.question,
            "criteria": spec.criteria,
            "where": spec.r#where,
            "threshold": spec.threshold,
            "live": records.len() - n_cached,
            "cached": n_cached,
            "records": records,
        }));
    }
    let newest = versions.last().cloned();
    for record in signals.iter_mut().filter_map(|s| s["records"].as_array_mut()).flatten() {
        if let Some(cached) = record["cached"].as_object_mut() {
            let older = cached.get("model").and_then(Value::as_str).is_some_and(|m| !m.is_empty() && newest.as_deref().is_some_and(|n| m < n));
            cached.insert("older_version".into(), json!(older));
        }
    }
    Inventory { json: json!({"model": jev.model, "newest_seen": newest, "signals": signals, "paths": paths}), asks }
}

/// `jev_cache_report`: this pack's asks with their cache status, plus cache
/// files no pack would ask for any more (orphans) and legacy files without a
/// payload. Reads every pack, one lock at a time.
async fn report_of(app: Arc<App>, pid: String) -> Result<Value, ApiError> {
    let worker = app.clone();
    let work = tokio::task::spawn_blocking(move || {
        let mut referenced: HashSet<String> = HashSet::new();
        let mut this = Value::Null;
        for other in worker.packs.keys() {
            let Some(entry) = worker.entry(other) else { continue };
            let inv = inventory(&lock(&entry.slot).session, &worker.jev);
            referenced.extend(inv.json["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from));
            if *other == pid {
                this = inv.json;
            }
        }
        let entries = worker.jev.list_cache();
        let is_orphan = |path: &Path| !referenced.contains(&path.display().to_string());
        let orphans = entries.iter().filter(|e| e.payload.is_some() && is_orphan(&e.path));
        let legacy = entries.iter().filter(|e| e.payload.is_none() && is_orphan(&e.path));
        let names: Vec<String> = orphans.clone().chain(legacy).map(|e| e.path.display().to_string()).collect();
        let questions: Vec<Value> = orphans
            .take(20)
            .map(|e| {
                let first = e.payload.as_ref().and_then(|p| p.get("questions")).and_then(Value::as_object).and_then(|q: &Map<String, Value>| q.values().next());
                let question = first.and_then(|q| q.get("instructions")).cloned().unwrap_or_else(|| json!("?"));
                json!({"question": question, "model": e.model, "at": e.at})
            })
            .collect();
        if let Some(report) = this.as_object_mut() {
            report.insert("available".into(), json!(worker.jev.available()));
            report.insert("files".into(), json!(entries.len()));
            report.insert("orphans".into(), json!(names));
            report.insert("orphan_questions".into(), json!(questions));
        }
        this
    });
    work.await.map_err(|e| ApiError::Internal(format!("the engine failed: {e}")))
}
