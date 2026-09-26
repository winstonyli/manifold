#!/usr/bin/env bash
# Usage: scripts/two-instance-test.sh
#
# Two headless instances on one isolated APPDATA (spec "Testing",
# Headless). Never touches the real %APPDATA%\manifold. Build first:
# cargo build -j 12 -p manifold.
#   1. A imports a group, B adds a note: both EXPORTSESSION node sets agree.
#   2. A imports enough notes to compact while B stays open; B edits
#      after: both agree and nothing is lost.
#   3. A adds a note and is killed; a fresh instance still has it.
#
# A kill is TerminateProcess (`taskkill //F //PID`), not power loss: the
# store's on-disk file is never mid-write at that point (each scenario
# always calls EXPORTSESSION, which saves first, immediately before the
# kill), so this only exercises "a replica vanishes", not crash recovery
# of a torn write.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
if [ ! -x "$exe" ]; then
    echo "two-instance-test: $exe not found — build first (cargo build -j 12 -p manifold)" >&2
    exit 1
fi
profile="$repo_root/target/two-instance-appdata-$$"
out="$repo_root/target/two-instance-out-$$"
mkdir -p "$profile/manifold" "$out"
export APPDATA="$(cygpath -w "$profile")"
echo '{"has_shown_first_launch_demo": true}' >"$profile/manifold/settings.json"
win() { cygpath -w "$1"; }
out_win="$(win "$out")"

# Windows PIDs of instances we started, so cleanup only ever kills our own
# (never by image name -- other sessions run their own manifold.exe).
a_winpid=""
b_winpid=""
c_winpid=""
cleanup() {
    local status=$?
    for wp in "$a_winpid" "$b_winpid" "$c_winpid"; do
        [ -n "$wp" ] && taskkill //F //PID "$wp" >/dev/null 2>&1
    done
    rm -rf "$profile" "$out"
    exit $status
}
trap cleanup EXIT

coproc A { "$exe" --headless; }
coproc B { "$exe" --headless; }
sa() { echo "$1" >&"${A[1]}"; }
sb() { echo "$1" >&"${B[1]}"; }

# Bash/MSYS spawns a native .exe through a small proxy process that stays
# alive to shuttle its pipes and relay its exit status back to `wait`, so
# `$A_PID`/`$B_PID` (the coproc's own PID) name that proxy, not
# manifold.exe -- confirmed live: ps -W showed two distinct WINPIDs, one
# for the coproc PID and a different one for its manifold.exe child.
# `taskkill` needs the real one: find the child of the coproc's PID whose
# command line is manifold, and read its WINPID column.
winpid_of() {
    ps -W | awk -v ppid="$1" '$2 == ppid && $0 ~ /manifold/ { print $4; exit }'
}
# Give each coprocess a moment to actually exec manifold.exe before
# looking for its child in the process table.
sleep 1
a_winpid="$(winpid_of "$A_PID")"
b_winpid="$(winpid_of "$B_PID")"
[ -n "$a_winpid" ] || {
    echo "FAIL: couldn't find A's manifold.exe Windows PID"
    exit 1
}
[ -n "$b_winpid" ] || {
    echo "FAIL: couldn't find B's manifold.exe Windows PID"
    exit 1
}

wait_file() { for _ in $(seq 100); do [ -s "$1" ] && return 0; sleep 0.2; done; echo "FAIL: $1 never appeared"; exit 1; }

# Compares two EXPORTSESSION files' shared nodes (ids, kinds, fields;
# `local` is personal and may differ).
same_nodes() {
    python -P "$repo_root/scripts/compare_sessions.py" "$1" "$2" </dev/null
}
assert_py() {
    python -P "$repo_root/scripts/session_asserts.py" "$@" </dev/null
}

# A no-op that still draws a frame (unlike SLEEP, which draws none), so
# the recipient's per-frame poll_store (event_loop.rs) has a chance to run
# and pick up the other instance's edits.
tick() { "$1" "EXPORTSTATE $out_win\\tick.json"; }

# Exports both sides and polls, up to ~10s, for their node sets to agree,
# rather than a single fixed-SLEEP-then-compare (the brief's fallback for
# scenario 1's timing, applied to every scenario here since the whole
# script is run three times in a row and must not flake).
wait_agree() { # $1=label $2=a_export_path $3=b_export_path
    local label="$1" af="$2" bf="$3" i log="$out/agree.log"
    for i in $(seq 1 50); do
        rm -f "$af" "$bf"
        tick sa
        tick sb
        sa "EXPORTSESSION $(win "$af")"
        sb "EXPORTSESSION $(win "$bf")"
        wait_file "$af"
        wait_file "$bf"
        if same_nodes "$af" "$bf" >"$log" 2>&1; then
            cat "$log"
            return 0
        fi
        sleep 0.2
    done
    echo "FAIL: $label: A and B never agreed after ~10s"
    cat "$log" || true
    exit 1
}

echo "=== scenario 1: import + note ==="
cat >"$out/pair.json" <<'EOF'
{"nodes": [
 {"id": "g", "kind": "group", "name": "Pair"},
 {"id": "a", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
EOF
sa "IMPORTGROUP $(win "$out/pair.json")"
sa WAITTABS
sb NEWNOTE
wait_agree "scenario 1" "$out/a1.json" "$out/b1.json"
assert_py has_kinds "$out/a1.json" "note:1" "tab:2"
echo "scenario 1: ok"

echo "=== scenario 2: compaction mid-run ==="
# 200 notes of 500 chars each: ~100 KB of ops, past COMPACT_MIN (64 KiB)
# and twice a snapshot this small, so A's save after the import compacts.
big="$out/big.json"
text="$(printf 'x%.0s' $(seq 1 500))"
{
    printf '{"nodes": [{"id": "biggroup", "kind": "group", "name": "Big"}'
    for i in $(seq 1 200); do
        printf ',{"id": "note%d", "kind": "note", "text": "%s", "group": "biggroup"}' "$i" "$text"
    done
    printf ']}'
} >"$big"
sa "IMPORTGROUP $(win "$big")"
sa WAITTABS
sb NEWNOTE
wait_agree "scenario 2" "$out/a2.json" "$out/b2.json"
[ -f "$profile/manifold/session.mnf.prev" ] || {
    echo "FAIL: scenario 2: session.mnf.prev is missing -- A should have compacted"
    exit 1
}
# >=201: the 200 imported notes plus B's own from this scenario (B's note
# from scenario 1 is a separate node again already counted at the group
# level, not as "note" text necessarily, so this is a floor, not exact).
assert_py has_kinds "$out/a2.json" "note:201"
echo "scenario 2: ok"

echo "=== scenario 3: kill A ==="
sa "EXPORTSESSION $(win "$out/before3.json")"
wait_file "$out/before3.json"
sa NEWNOTE
sa "EXPORTSESSION $(win "$out/a3.json")"
wait_file "$out/a3.json"
new_id="$(assert_py new_id "$out/before3.json" "$out/a3.json")"
echo "A's new note id: $new_id"

taskkill //F //PID "$a_winpid"
wait "$A_PID" 2>/dev/null || true
a_winpid="" # already dead; cleanup shouldn't try again

sb QUIT
wait "$B_PID" 2>/dev/null || true
b_winpid=""

coproc C { "$exe" --headless; }
sc() { echo "$1" >&"${C[1]}"; }
sleep 1
c_winpid="$(winpid_of "$C_PID")"
[ -n "$c_winpid" ] || {
    echo "FAIL: couldn't find C's manifold.exe Windows PID"
    exit 1
}
sc "EXPORTSESSION $(win "$out/c.json")"
wait_file "$out/c.json"
assert_py has_id "$out/c.json" "$new_id"
sc QUIT
wait "$C_PID" 2>/dev/null || true
c_winpid=""
echo "scenario 3: ok"

echo "two-instance test: ok"
