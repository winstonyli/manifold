#!/usr/bin/env bash
# Usage: scripts/cef-pin.sh <cef version prefix>   e.g. scripts/cef-pin.sh 152.0.6
# Regenerates crates/manifold/src/cef_pin.rs. Downloads ~172 MB once; see scripts/cef_pin.py.
set -euo pipefail
cd "$(dirname "$0")/.."
out=$(mktemp)
python -P scripts/cef_pin.py "$1" </dev/null >"$out"
mv "$out" crates/manifold/src/cef_pin.rs
