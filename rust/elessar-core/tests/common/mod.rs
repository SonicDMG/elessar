//! Shared helpers for the fixture tests: locate the repo, load packs and goldens.
#![allow(dead_code)]

use elessar_core::pack::{load_pack, Pack};
use serde_json::Value;
use std::path::PathBuf;

pub fn repo_root() -> PathBuf {
    // elessar-core/ -> rust/ -> repo
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

pub fn pack_ids() -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(repo_root().join("scenarios"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("manifest.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

pub fn pack(id: &str) -> Pack {
    load_pack(&repo_root().join("scenarios").join(id)).unwrap_or_else(|e| panic!("{e}: {:?}", e.errors))
}

pub fn golden(kind: &str, name: &str) -> Value {
    let path = repo_root().join("golden").join(kind).join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden {}", path.display()))).unwrap()
}

pub fn read_source(p: &Pack, sid: &str) -> String {
    std::fs::read_to_string(p.sources.get(sid).unwrap().path.as_ref().unwrap()).unwrap()
}

/// Compare two floats as the Python engine printed them: exact after shortest round-trip, with a tiny slack.
pub fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}
