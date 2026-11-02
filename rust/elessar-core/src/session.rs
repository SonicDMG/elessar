//! One investigation: decisions, the recompute, actions, insights (spec §7, §8, §10). Phase 3.

use crate::pack::Pack;
use crate::types::{Insight, State};
use serde_json::Value;

pub struct Session {
    pub pack: Pack,
}

impl Session {
    pub fn new(pack: Pack) -> Session {
        Session { pack }
    }

    /// Connect a source from an already-fetched payload (the server fetches; tests read files).
    pub fn connect(&mut self, source: &str, raw: String, latency_ms: u64) -> Vec<Insight> {
        let _ = (source, raw, latency_ms);
        todo!("phase 3: connect")
    }

    /// Apply an action payload (spec §10) and return the insights it earned.
    pub fn act(&mut self, payload: &Value) -> Result<Vec<Insight>, String> {
        let _ = payload;
        todo!("phase 3: actions")
    }

    pub fn state(&self) -> &State {
        todo!("phase 3: state")
    }
}
