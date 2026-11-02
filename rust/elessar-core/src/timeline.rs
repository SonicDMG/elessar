//! The board re-inferred at every event time (spec §9). Phase 3.

use crate::session::Session;
use serde_json::Value;

/// `GET /api/timeline` as a JSON value (shape in spec §9), cached per board state.
pub fn timeline(session: &mut Session) -> Value {
    let _ = session;
    todo!("phase 3: timeline")
}
