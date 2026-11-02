//! Jev cache keys and reading (spec §11). Phase 1b (small, self-contained).

use crate::pipeline::py;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::path::{Path, PathBuf};

/// Python's `json.dumps(payload, sort_keys=True)`: sorted keys, `", "` and
/// `": "` separators, shortest round-trip floats, non-ASCII as `\uXXXX`.
pub fn canonical(payload: &Value) -> String {
    py::dumps(payload, true)
}

/// `sha256(canonical)[:32] + ".json"` under the cache directory.
pub fn cache_path(dir: &Path, payload: &Value) -> PathBuf {
    let digest = Sha256::digest(canonical(payload).as_bytes());
    let mut name = String::with_capacity(37);
    for byte in &digest[..16] {
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(".json");
    dir.join(name)
}

/// The cached entry, if any: `{payload?, answers, model, ms, cost, at?}`.
/// Read with the exact JSON scanner: a payload's floats must survive to
/// reproduce its key. A missing or unreadable file is `None`.
pub fn load_cached(path: &Path) -> Option<Value> {
    py::json_loads(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_matches_python_dumps() {
        let payload = py::json_loads(r#"{"b": [1, 2.50, 1e16, 1.0, true, null], "a": {"z": "\u00e9\ud83d\ude00\n\"", "y": -0.0}, "é": 1e-05, "Z": "\u007f"}"#).unwrap();
        assert_eq!(
            canonical(&payload),
            r#"{"Z": "\u007f", "a": {"y": -0.0, "z": "\u00e9\ud83d\ude00\n\""}, "b": [1, 2.5, 1e+16, 1.0, true, null], "\u00e9": 1e-05}"#
        );
    }

    #[test]
    fn cache_path_is_a_truncated_digest() {
        // python3 -c 'import hashlib,json; print(hashlib.sha256(json.dumps({"a": 1}, sort_keys=True).encode()).hexdigest()[:32])'
        let p = cache_path(Path::new("/c"), &json!({"a": 1}));
        assert_eq!(p, Path::new("/c/f9d86028c6e0d64e225186f96acb6933.json"));
    }

    #[test]
    fn missing_cache_entry_is_none() {
        assert!(load_cached(Path::new("/nonexistent/elessar.json")).is_none());
    }
}
