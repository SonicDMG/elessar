#!/bin/sh
# Start ELESSAR on http://127.0.0.1:8777. Extra flags pass through, e.g.
#   ./run.sh --scenario supply-chain
#   ./run.sh --check
cd "$(dirname "$0")" && exec python3 -m server.app "$@"
