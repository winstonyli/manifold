#!/usr/bin/env bash
# Usage: [SECONDS_PANNING=20] scripts/json-pan-profile.sh
#
# Profiles panning a big JSON level (a few thousand cards) with samply, via
# the elevated `Samply` task (~/.claude/SAMPLY.md). The app runs headless on
# an isolated profile folder, driven by a command file (`--script`,
# `--profile-dir`: the task can set neither stdin nor APPDATA).
# Needs a release build with debug info:
#   CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p manifold
# Output: target/json-profile/profile.json.gz (open with `samply load`, or
# resolve frames with llvm-symbolizer, see SAMPLY.md). Check the machine is
# quiet first (CPU under ~20%) and say so with any numbers.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo/target/json-profile"
exe="$repo/target/release/manifold.exe"
[ -f "$exe" ] || { echo "build the release exe first (see header)" >&2; exit 1; }
rm -rf "$out"
mkdir -p "$out/appdata"
echo '{"has_shown_first_launch_demo": true}' >"$out/appdata/settings.json"
python -P "$repo/scripts/json-pan-profile.py" fixture "$out/fixture.json" </dev/null
python -P "$repo/scripts/json-pan-profile.py" script "$(cygpath -w "$out/fixture.json")" "$out/commands.txt" "${SECONDS_PANNING:-20}" </dev/null
cp "$exe" "$out/manifold.exe"
powershell -NoProfile -File "$(cygpath -w "$HOME/.claude/samply/profile.ps1")" \
    -Exe "$(cygpath -w "$out/manifold.exe")" -Out "$(cygpath -w "$out/profile.json.gz")" \
    -ExeArgs --headless,--profile-dir,"$(cygpath -w "$out/appdata")",--script,"$(cygpath -w "$out/commands.txt")"
