//! Jev cache keys and reading (spec §11). Phase 1b (small, self-contained).

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Python's `json.dumps(payload, sort_keys=True)`: sorted keys, `", "` and
/// `": "` separators, shortest round-trip floats, non-ASCII as `\uXXXX`.
pub fn canonical(payload: &Value) -> String {
    let _ = payload;
    todo!("phase 1b: canonical")
}

/// `sha256(canonical)[:32] + ".json"` under the cache directory.
pub fn cache_path(dir: &Path, payload: &Value) -> PathBuf {
    let _ = (dir, payload);
    todo!("phase 1b: cache_path")
}

/// The cached entry, if any: `{payload?, answers, model, ms, cost, at?}`.
pub fn load_cached(path: &Path) -> Option<Value> {
    let _ = path;
    todo!("phase 1b: load_cached")
}
