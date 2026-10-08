#!/usr/bin/env bash
# Usage: [SECONDS_PANNING=20] [MODE=pan|zoom|reveal-numbers|reveal-objects] [REVEAL_N=200000] [TIME_ONLY=1] scripts/json-pan-profile.sh
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
# Keep SECONDS_PANNING small (10 works): samply's kernel trace wrote 7 GB in
# 15 minutes for a 20 s run that takes 46 s untraced, and never finished.
# Without samply, time the same command file (`manifold.exe --headless
# --profile-dir D --script commands.txt`): setup takes ~6 s, then ~33 ms per
# command for both pan and zoom (2026-10-01, ~2k cards, machine quiet).
#
# TIME_ONLY=1: no samply. Runs the copied exe once, in the foreground, and appends one line to
# target/json-time.log: `TIME_ONLY mode=… seconds=… commands=… ms=… exit=…`. The cost per command is the
# difference between two runs of different SECONDS_PANNING, divided by the difference in commands, so the
# setup cancels out. Launch detached at BelowNormal (children inherit it):
#   TIME_ONLY=1 MODE=pan SECONDS_PANNING=10 cmd //c start "" //b //belownormal bash scripts/json-pan-profile.sh
# and wait for the line before starting the next run (each run clears target/json-profile).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo/target/json-profile"
exe="$repo/target/release/manifold.exe"
[ -f "$exe" ] || { echo "build the release exe first (see header)" >&2; exit 1; }
rm -rf "$out"
mkdir -p "$out/appdata"
echo '{"has_shown_first_launch_demo": true}' >"$out/appdata/settings.json"
python -P "$repo/scripts/json-pan-profile.py" fixture "$out/fixture.json" "${MODE:-pan}" </dev/null
python -P "$repo/scripts/json-pan-profile.py" script "$(cygpath -w "$out/fixture.json")" "$out/commands.txt" "${SECONDS_PANNING:-20}" "${MODE:-pan}" </dev/null
cp "$exe" "$out/manifold.exe"
if [ -n "${TIME_ONLY:-}" ]; then
    commands=$(wc -l <"$out/commands.txt")
    start=$(date +%s%N)
    status=0
    "$out/manifold.exe" --headless --profile-dir "$(cygpath -w "$out/appdata")" \
        --script "$(cygpath -w "$out/commands.txt")" >"$out/run.log" 2>&1 || status=$?
    ms=$(( ($(date +%s%N) - start) / 1000000 ))
    # A reveal run's selection, from its EXPORTSTATE: empty means the level never opened and the run timed nothing.
    selected=$(grep -o '"selected": *"[^"]*"' "$out/state.json" 2>/dev/null | sed 's/.*"\([^"]*\)"$/\1/' || true)
    echo "TIME_ONLY mode=${MODE:-pan} seconds=${SECONDS_PANNING:-20} commands=$commands ms=$ms exit=$status selected=$selected" \
        | tee -a "$repo/target/json-time.log"
    exit 0
fi
powershell -NoProfile -File "$(cygpath -w "$HOME/.claude/samply/profile.ps1")" \
    -Exe "$(cygpath -w "$out/manifold.exe")" -Out "$(cygpath -w "$out/profile.json.gz")" \
    -ExeArgs --headless,--profile-dir,"$(cygpath -w "$out/appdata")",--script,"$(cygpath -w "$out/commands.txt")"
