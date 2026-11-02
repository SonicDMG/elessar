#!/bin/sh
# Start the Rust engine on http://127.0.0.1:8778. Extra flags pass through, e.g.
#   ./run-rust.sh --scenario supply-chain
#   ./run-rust.sh --check
# rustup's cargo may not be on PATH in a shell that did not source its profile.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
cd "$(dirname "$0")" && exec cargo run --release --manifest-path rust/Cargo.toml -p elessar-server -- "$@"
