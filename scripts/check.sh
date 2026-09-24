#!/usr/bin/env bash
# Every check CI runs, runnable before a push: ./scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test
