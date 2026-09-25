#!/usr/bin/env bash
# Usage: scripts/drive.sh <commands-file>
#
# Runs one manifold session driven by a file of debug commands (see HELP,
# or README.md's "Headless mode" section), in a fresh isolated APPDATA so
# the real %APPDATA%\manifold is never read or written.
#
#   SEED=<dir>      copy <dir>/*.json (session.json, settings.json, ...) into
#                   the isolated profile first; without it the first-launch
#                   demo scene loads
#   VISIBLE=1       a real window (--debug-commands) instead of --headless
#   KEEP_APPDATA=1  keep the isolated profile afterwards (e.g. to inspect a
#                   moved-aside file); otherwise it's deleted on exit
#
# Command-file lines:
#   - `{repo}` is replaced by the repo root as a Windows path, so a line
#     can be `SCREENSHOT {repo}\target\shots\a.png` (the app needs Windows
#     paths, and building them in bash is error-prone: printf and heredocs
#     turn `\a` into a BEL character).
#   - `sleep N` is sent as the app's SLEEP, which pauses in order with the
#     other commands. A bash sleep would not: while the app waits on a
#     WAITTABS or SCREENSHOT, this script would keep writing.
#   - a line starting with `!` runs here in bash, when the writer reaches
#     it, which is not in step with the app (the writer finishes almost at
#     once). To act after some command has run, wait for a file it writes,
#     e.g. `!(while [ ! -f x ]; do sleep 0.2; done; cp a b) &` after an
#     `EXPORTSESSION` to x.
#   - a `!` line runs in the writer loop itself, so without `&` it blocks
#     the writer, and whatever it prints goes to the app as commands. A
#     probe can wait for an EXPORTSTATE file, then print gestures computed
#     from the live layout. Poll until two exports agree before trusting
#     positions: after entering a group the camera eases, and SLEEP draws
#     no frames (each command draws one), so a fixed sleep can export
#     mid-ease.
#   - blank lines are skipped.
#
# Exit status: a failed command's own status, or the app's. Git Bash
# reports a death by signal N as N<<8 to a Windows parent, so 3840
# means this script got SIGTERM from outside (README, Known limitations).
#
# Build first: cargo build -p manifold (a stale exe is refused; see
# fresh-exe.sh).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
. scripts/fresh-exe.sh
require_fresh_exe target/debug/manifold.exe
CMDS="$1"
REPO_WIN=$(cygpath -w "$PWD")
D="target/drive-appdata-$$"
rm -rf "$D"; mkdir -p "$D/appdata/manifold"
[ -n "${KEEP_APPDATA:-}" ] || trap 'rm -rf "$D"' EXIT
export APPDATA="$(cygpath -w "$PWD/$D")\\appdata"
[ -z "${SEED:-}" ] || cp "$SEED"/*.json "$D/appdata/manifold/"
MODE=--headless; [ -z "${VISIBLE:-}" ] || MODE=--debug-commands
while IFS= read -r line; do
  line=${line//\{repo\}/"$REPO_WIN"}
  case "$line" in
    '!'*) eval "${line#!}" ;;
    "") ;;
    *) printf '%s\n' "$line" ;;
  esac
done < "$CMDS" | target/debug/manifold.exe "$MODE"
