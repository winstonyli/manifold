#!/usr/bin/env bash
# Usage: scripts/cef-pin.sh [--platform <cdn id>] <cef version prefix>   e.g. scripts/cef-pin.sh 152.0.6
# Rewrites that platform's pin in crates/manifold/src/cef_pin.rs. Downloads the archive (~172 MB for
# windows64) once; see scripts/cef_pin.py.
set -euo pipefail
cd "$(dirname "$0")/.."
python -P scripts/cef_pin.py "$@" </dev/null
