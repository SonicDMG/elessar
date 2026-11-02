//! `elessar check [--scenarios-dir DIR]`: validate every pack, as
//! `python3 -m server.app --check` does.
//!
//! `elessar diff <pack> [--python-dump DIR] [--scenarios-dir DIR] [--show-allowed]`:
//! play the pack offline (every source in rack order, merges per `expect.json`)
//! and compare state and timeline with the Python engine's, from a dump written
//! by `python3 tools/py_dump.py . DIR`. Without `--python-dump` it writes a fresh
//! one to a temporary directory first.

use elessar_core::pack::{self, Pack};
use elessar_core::session::Session;
use elessar_core::store::string_form;
use elessar_core::timeline::timeline;
use elessar_server::packcheck;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: elessar check [--scenarios-dir DIR]\n       elessar diff <pack> [--python-dump DIR] [--scenarios-dir DIR] [--show-allowed]";

const TOLERANCE: f64 = 1e-9;

/// Paths that may differ when decisions are the same, and why:
/// - `merges.pending`: which fuzzy pairs reach the threshold is the similarity's call (spec §13).
/// - `object_types`: the Rust loader fills defaults the frontend does not read (only keys are used).
const ALLOWED: &[&str] = &["merges.pending", "object_types"];

/// The repo holding `scenarios/`: upward from the working directory, else
/// upward from the binary, else where this crate was built.
fn find_root() -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    for start in cwd.as_deref().into_iter().chain(exe.as_deref()) {
        if let Some(found) = start.ancestors().find(|d| d.join("scenarios").is_dir() && d.join("tools").is_dir()) {
            return found.to_path_buf();
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap_or(Path::new(".")).to_path_buf()
}

fn clock() -> i64 {
    // 2026-11-02T00:00:00Z, the dump's frozen clock.
    1_793_577_600_000
}

/// Every difference between two JSON values, as `path: python vs rust`.
fn diff(path: &str, py: &Value, rs: &Value, out: &mut Vec<String>) {
    match (py, rs) {
        (Value::Number(a), Value::Number(b)) => {
            let (x, y) = (a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN));
            if (x - y).abs() > TOLERANCE {
                out.push(format!("{path}: {x} vs {y}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{path}: length {} vs {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, out);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, x) in a {
                match b.get(k) {
                    Some(y) => diff(&format!("{path}.{k}"), x, y, out),
                    None => out.push(format!("{path}.{k}: only in python ({x})")),
                }
            }
            for (k, y) in b {
                if !a.contains_key(k) {
                    out.push(format!("{path}.{k}: only in rust ({y})"));
                }
            }
        }
        _ if py != rs => out.push(format!("{path}: {py} vs {rs}")),
        _ => {}
    }
}

fn is_allowed(d: &str) -> bool {
    ALLOWED.iter().any(|a| d.contains(a))
}

/// Play the pack as `tools/py_dump.py` does for `decided`: every source in rack
/// order, then the first pending proposal per `expect.json` until none is left.
fn play(pack: Pack) -> Result<Session, String> {
    let spec_path = pack.dir.join("expect.json");
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(&spec_path).map_err(|e| format!("{}: {e}", spec_path.display()))?).map_err(|e| format!("{}: {e}", spec_path.display()))?;
    let mut s = Session::new(pack);
    s.clock = clock;
    s.base_url = "offline".into();
    for sid in s.pack.source_order.clone() {
        let path = s.pack.source(&sid).and_then(|src| src.path.clone()).ok_or_else(|| format!("source '{sid}' has no file to play offline"))?;
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        s.connect(&sid, raw, 0);
    }
    let rejected = spec["merges"]["reject"].as_array().cloned().unwrap_or_default();
    while let Some(proposal) = s.state().merges.pending.first().cloned() {
        let (a, b) = (string_form(&proposal.a.value), string_form(&proposal.b.value));
        let listed = rejected.iter().any(|pair| (pair[0] == a.as_str() && pair[1] == b.as_str()) || (pair[0] == b.as_str() && pair[1] == a.as_str()));
        let decision = if listed || spec["merges"]["default"] != "commit" { "reject" } else { "merge" };
        s.act(&json!({"action": decision, "proposal": proposal.id}))?;
    }
    Ok(s)
}

/// A fresh Python dump in a temporary directory.
fn write_dump(root: &Path) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("elessar-py-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let status = Command::new("python3").arg(root.join("tools/py_dump.py")).arg(root).arg(&dir).status().map_err(|e| format!("cannot run python3: {e}"))?;
    if status.success() {
        Ok(dir)
    } else {
        Err(format!("tools/py_dump.py failed ({status})"))
    }
}

/// Prints the differing paths or `identical`. `Ok(true)` when only allowed paths differ.
fn diff_pack(scenarios_dir: &Path, id: &str, dump_dir: &Path, show_allowed: bool) -> Result<bool, String> {
    let pack = pack::load_pack(&scenarios_dir.join(id)).map_err(|e| format!("{id}: {:?}", e.errors))?;
    let dump_path = dump_dir.join(format!("{id}.json"));
    let dump: Value = serde_json::from_str(&std::fs::read_to_string(&dump_path).map_err(|e| format!("{}: {e}", dump_path.display()))?).map_err(|e| format!("{}: {e}", dump_path.display()))?;
    let mut s = play(pack)?;
    let state = serde_json::to_value(s.state()).map_err(|e| e.to_string())?;
    let tl = timeline(&mut s);

    let mut differences = Vec::new();
    diff("state", &dump["decided"], &state, &mut differences);
    diff("timeline", &dump["timeline"], &tl, &mut differences);
    let (allowed, real): (Vec<&String>, Vec<&String>) = differences.iter().partition(|d| is_allowed(d));
    if show_allowed {
        for d in &allowed {
            println!("allowed: {d}");
        }
    }
    for d in &real {
        println!("{d}");
    }
    if real.is_empty() {
        println!("{id}: identical ({} allowed difference(s) at {})", allowed.len(), ALLOWED.join(", "));
    } else {
        println!("{id}: {} difference(s), {} allowed", real.len(), allowed.len());
    }
    Ok(real.is_empty())
}

fn run() -> Result<ExitCode, String> {
    let root = find_root();
    let mut scenarios_dir = root.join("scenarios");
    let (mut dump, mut show_allowed) = (None, false);
    let mut positional = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scenarios-dir" => scenarios_dir = PathBuf::from(args.next().ok_or(USAGE)?),
            "--python-dump" => dump = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--show-allowed" => show_allowed = true,
            flag if flag.starts_with("--") => return Err(format!("unknown argument '{flag}'\n{USAGE}")),
            _ => positional.push(arg),
        }
    }
    match positional.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["check"] => Ok(if packcheck::print_check(&scenarios_dir) { ExitCode::SUCCESS } else { ExitCode::FAILURE }),
        ["diff", id] => {
            let dir = match dump {
                Some(dir) => dir,
                None => write_dump(&root)?,
            };
            let same = diff_pack(&scenarios_dir, id, &dir, show_allowed)?;
            Ok(if same { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
