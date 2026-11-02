//! The ELESSAR HTTP server: the unchanged `web/` frontend against the Rust engine.
//!
//! Module map:
//!
//! - [`config`]  command line, `.env`, where the repo is
//! - [`packcheck`] the pack checks that need the source files, `--check`
//! - [`jev`]     the Jev client and its on-disk cache, shared with the Python engine
//! - [`agent`]   the in-app agent (no I/O of its own)
//! - [`app`]     one session per pack, the actions, the Jev cache inspector
//! - [`routes`]  the axum routes, mirroring `server/app.py`

pub mod agent;
pub mod app;
pub mod config;
pub mod jev;
pub mod packcheck;
pub mod routes;
