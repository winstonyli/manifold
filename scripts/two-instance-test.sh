#!/usr/bin/env bash
# Usage: [ENGINE=cef] scripts/two-instance-test.sh
#
# Two headless instances on one isolated APPDATA (spec "Testing",
# Headless). Never touches the real %APPDATA%\manifold. Build first:
# cargo build -j 12 -p manifold.
# ENGINE (default webview2) is the engine new tabs start in, which
# scenarios 3 and 4 navigate.
#   1. A imports a group, B adds a note: both EXPORTSESSION node sets agree.
#   2. A imports enough notes to compact while B stays open; B edits
#      after: both agree and nothing is lost.
#   3. A navigates a tab to a site that redirects to a fresh url on every
#      load (scripts/nonce_server.py): B follows once, and the two don't
#      keep navigating each other (the site's request count settles).
#   4. A clicks a link on that site: B follows. A's page then navigates
#      by script: B doesn't (the engine reports how a navigation started).
#      Likewise a single-page app's url change: from a click, B follows;
#      from a timer, it doesn't.
#   5. A adds a note and is killed; a fresh instance still has it.
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
echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"${ENGINE:-webview2}\"}" >"$profile/manifold/settings.json"
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
# B edits only once A has compacted (the fresh profile had no `.prev`), so
# the edit lands on the rewritten file.
wait_file "$profile/manifold/session.mnf.prev"
sb NEWNOTE
wait_agree "scenario 2" "$out/a2.json" "$out/b2.json"
# >=201: the 200 imported notes plus B's own from this scenario (B's note
# from scenario 1 is a separate node again already counted at the group
# level, not as "note" text necessarily, so this is a floor, not exact).
assert_py has_kinds "$out/a2.json" "note:201"
echo "scenario 2: ok"

echo "=== scenario 3: a url that changes on every load ==="
python -P "$repo_root/scripts/nonce_server.py" "$out/hits" 240 >"$out/port" 2>"$out/server.err" </dev/null &
wait_file "$out/port"
port="$(tr -d '\r\n' <"$out/port")"
hits() { cat "$out/hits" 2>/dev/null || echo 0; }
sa NEWTAB
sa WAITTABS
# The new tab is focused but shown in Graph view headless.
sa TOGGLEVIEW
# Let the new tab's own first load settle, or its address-bar update
# overwrites the typed url before ENTER.
sa "SLEEP 3"
sa "ADDRESSBAR http://127.0.0.1:$port/r"
sa "SLEEP 1"
sa ENTER
# A's load (a redirect, then the page), then B's (the same, as its copy
# follows): at least 4 requests.
for _ in $(seq 150); do [ "$(hits)" -ge 4 ] && break; sleep 0.2; done
[ "$(hits)" -ge 4 ] || {
    echo "FAIL: scenario 3: only $(hits) requests; B never followed A's navigation"
    exit 1
}
# Past HOST_LOAD_GRACE (5 s), then a window as long again: a bounce is a
# round trip every second or two, so the count would keep climbing.
sleep 6
settled="$(hits)"
sleep 10
[ "$(hits)" -eq "$settled" ] || {
    echo "FAIL: scenario 3: the instances keep navigating each other ($settled -> $(hits) requests in 10 s)"
    exit 1
}
echo "scenario 3: ok ($settled requests)"

echo "=== scenario 4: a click is shared, a script's navigation isn't ==="
# "<r-repeats> <dest-repeats> <dests> <spa-clicks> <spa-timers>", see
# nonce_server.py.
repeats() { cat "$out/hits.repeats" 2>/dev/null || echo "0 0 0 0 0"; }
field() { repeats | tr -d '\r\n' | cut -d' ' -f"$1"; }
before="$(field 1)"
sa "ADDRESSBAR http://127.0.0.1:$port/link"
sa "SLEEP 1"
sa ENTER
sa "SLEEP 3"
# The page is one window-sized link, so any click below the toolbar hits it.
sa "CLICK 400 300"
# A lands on a served-once /r?n= and shares it; B asks for it again.
for _ in $(seq 75); do [ "$(field 1)" -gt "$before" ] && break; sleep 0.2; done
[ "$(field 1)" -gt "$before" ] || {
    echo "FAIL: scenario 4: B never followed A's link click"
    exit 1
}
sa "ADDRESSBAR http://127.0.0.1:$port/js"
sa "SLEEP 1"
sa ENTER
# A's page and then B's (it follows the typed url) each navigate by script
# to their own /dest; had A's been shared, B would ask for A's too.
for _ in $(seq 75); do [ "$(field 3)" -ge 2 ] && break; sleep 0.2; done
[ "$(field 3)" -ge 2 ] || {
    echo "FAIL: scenario 4: only $(field 3) script navigations ran; expected A's and B's"
    exit 1
}
sleep 5
[ "$(field 2)" -eq 0 ] || {
    echo "FAIL: scenario 4: B followed A's script navigation"
    cat "$out/hits.log"
    sa "EXPORTSESSION $(win "$out/a4.json")"; sb "EXPORTSESSION $(win "$out/b4.json")"
    wait_file "$out/a4.json"; wait_file "$out/b4.json"
    grep -o '"url": *"[^"]*"' "$out/a4.json" "$out/b4.json"
    exit 1
}
# A single-page app: a click's pushState is shared, a timer's isn't. Only
# B's load of a shared url asks the server for /spa/c or /spa/t.
sa "ADDRESSBAR http://127.0.0.1:$port/spa"
sa "SLEEP 1"
sa ENTER
sa "SLEEP 3"
sa "CLICK 400 300"
for _ in $(seq 75); do [ "$(field 4)" -ge 1 ] && break; sleep 0.2; done
[ "$(field 4)" -ge 1 ] || {
    echo "FAIL: scenario 4: B never followed A's pushState from a click"
    exit 1
}
sleep 5
[ "$(field 5)" -eq 0 ] || {
    echo "FAIL: scenario 4: B followed A's pushState from a timer"
    exit 1
}
echo "scenario 4: ok"

echo "=== scenario 5: kill A ==="
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
echo "scenario 5: ok"

echo "two-instance test: ok"
