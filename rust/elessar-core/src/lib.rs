//! ELESSAR engine: packs, records, resolution, rules, inference, timeline.
//!
//! The contract is `docs/engine-spec.md`; the packs' `expect.json` files are
//! the acceptance tests. Module map (data flows top to bottom):
//!
//! - [`pack`]      load and validate a scenario pack
//! - [`store`]     parsed records of a source, with pointers and excerpts
//! - [`pipeline`]  the six parsers, conditions, transforms, fragments
//! - [`time`]      record and document timestamps, undated policy
//! - [`resolve`]   entity resolution: clusters, provenance, fuzzy proposals
//! - [`rules`]     the four signal kinds and strength
//! - [`infer`]     noisy-OR with inhibitors and competition
//! - [`session`]   decisions, the recompute, actions, insights, API v1 state
//! - [`timeline`]  the board re-inferred at every event time
//! - [`jevcache`]  Jev cache key canonicalisation and cache reading
//! - [`types`]     the JSON shapes the frontend reads (API v1)

pub mod infer;
pub mod jevcache;
pub mod pack;
pub mod pipeline;
pub mod resolve;
pub mod rules;
pub mod session;
pub mod store;
pub mod time;
pub mod timeline;
pub mod types;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
