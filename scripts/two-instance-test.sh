#!/usr/bin/env bash
# Usage: [ENGINE=cef] scripts/two-instance-test.sh
# Prints a TIMING line per scenario (scripts/timing.sh); gate.sh collects them.
#
# Two headless instances on one isolated APPDATA (spec "Testing",
# Headless). Never touches the real %APPDATA%\manifold. Build first:
# cargo build -j 12 -p manifold.
# ENGINE (default webview2) is the engine new tabs start in, which
# scenarios 3, 4, 5 and 7 navigate.
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
#   5. A navigates that tab to a local JSON file and B follows: the
#      document gains exactly one Json node, not one per instance.
#   6. A adds a note and is killed; a fresh instance still has it.
#   7. That instance, now alone, types a url whose page navigates by
#      script, and quits: the next instance reopens the tab where the
#      script took it (the personal-state live_url), not at the typed url.
# Scenarios 8-10 run between 2 and 3. Moves use MOVETOGROUP (the node
# menu's "Move into", which saves); "groups agree" means each node's group
# is the same in what A shows, what B shows (EXPORTSTATE mirror_groups) and
# the document.
#   8. A creates a group and a note and moves the note into the group: the
#      groups agree.
#   9. A and B move one note into two different groups back to back: the
#      groups agree on one of the two, no node is lost, no cycle.
#  10. A moves group G into H while B moves H into G: the groups agree,
#      no node is lost, no cycle, nothing hangs.
# Scenarios 11-12 follow them, with RENAMEGROUP <id> <name>, SETNOTETEXT
# (type into a note and blur it) and MOVENODE (drag and release), each of
# which saves; "agree" is as above for the name, text or document pos
# (EXPORTSTATE mirror_names, mirror_texts, mirror_pos).
#  11. A renames a group: the names agree on A's. Then A and B rename it
#      back to back: the names agree on one of the two.
#  12. A edits a note's text: the texts agree on A's. Then A and B edit it
#      back to back: the texts agree on one of the two. Then A moves the
#      note: the positions agree on where A dropped it.
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
# Refuses an exe older than its sources, and a cef-runtime CEF host older
# than the built one (ENGINE=cef runs the copy).
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=timing.sh
. "$repo_root/scripts/timing.sh"
require_fresh_exe "$exe" || exit 1
timing_mark setup
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
d_winpid=""
nonce_pid="" # scenario 3's page server
cleanup() {
    local status=$?
    timing_mark
    for wp in "$a_winpid" "$b_winpid" "$c_winpid" "$d_winpid"; do
        [ -n "$wp" ] && taskkill //F //PID "$wp" >/dev/null 2>&1
    done
    [ -n "$nonce_pid" ] && kill_tree "$nonce_pid"
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
# `taskkill` needs the real one: winpid_of_child (winpid.sh) finds the child
# of the coproc's PID whose command line is manifold, retrying while the
# coprocess has yet to exec manifold.exe. Both before either check, so a
# failure still leaves cleanup the other's pid.
a_winpid="$(winpid_of_child "$A_PID")" || true
b_winpid="$(winpid_of_child "$B_PID")" || true
[ -n "$a_winpid" ] || {
    echo "FAIL: couldn't find A's manifold.exe Windows PID"
    exit 1
}
[ -n "$b_winpid" ] || {
    echo "FAIL: couldn't find B's manifold.exe Windows PID"
    exit 1
}

wait_file() { for _ in $(seq 300); do [ -s "$1" ] && return 0; sleep 0.2; done; echo "FAIL: $1 never appeared"; exit 1; }

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
    for i in $(seq 1 150); do
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
    echo "FAIL: $label: A and B never agreed after ~30s"
    cat "$log" || true
    exit 1
}

timing_mark s1-import
echo "=== scenario 1: import + note ==="
cat >"$out/pair.json" <<'EOF'
{"nodes": [
 {"id": "g", "type": "application/vnd.manifold.group", "name": "Pair"},
 {"id": "a", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
EOF
sa "IMPORTGROUP $(win "$out/pair.json")"
sa WAITTABS
sb NEWNOTE
wait_agree "scenario 1" "$out/a1.json" "$out/b1.json"
assert_py has_kinds "$out/a1.json" "text/markdown:1" "application/vnd.manifold.tab:2"
echo "scenario 1: ok"

timing_mark s2-compaction
echo "=== scenario 2: compaction mid-run ==="
# 200 notes of 500 chars each: ~100 KB of ops, past COMPACT_MIN (64 KiB)
# and twice a snapshot this small, so A's save after the import compacts.
big="$out/big.json"
text="$(printf 'x%.0s' $(seq 1 500))"
{
    printf '{"nodes": [{"id": "biggroup", "type": "application/vnd.manifold.group", "name": "Big"}'
    for i in $(seq 1 200); do
        printf ',{"id": "note%d", "type": "text/markdown", "text": "%s", "group": "biggroup"}' "$i" "$text"
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
assert_py has_kinds "$out/a2.json" "text/markdown:201"
echo "scenario 2: ok"

# Scenarios 8-10 (groups) run here, while both instances are in Graph view
# with no tabs navigating.

# Runs `$2` on instance `$1` (sa or sb) and prints the id of the one node it
# added. Nothing else may add a node meanwhile.
new_node() { # $1=sa|sb $2=command $3=file label
    local before="$out/$3-before.json" after="$out/$3-after.json"
    "$1" "EXPORTSESSION $(win "$before")"
    wait_file "$before"
    "$1" "$2"
    "$1" "EXPORTSESSION $(win "$after")"
    wait_file "$after"
    assert_py new_id "$before" "$after" | tr -d '\r'
}

# Polls, up to ~10s, until each listed id's group is the same in what A
# shows, what B shows (EXPORTSTATE's mirror_groups) and the document (A's
# EXPORTSESSION, which saves first). Leaves the last exports at
# $out/<label>-{a,b}-state.json and $out/<label>-doc.json.
wait_mirror() { # $1=label $2...=ids
    wait_field group "$@"
}
# wait_mirror for any field session_asserts.py field_agree knows (group,
# name, text, pos).
wait_field() { # $1=field $2=label $3...=ids
    local field="$1" label="$2" i log="$out/mirror.log"
    shift 2
    local af="$out/$label-a-state.json" bf="$out/$label-b-state.json" df="$out/$label-doc.json"
    for i in $(seq 1 150); do
        rm -f "$af" "$bf" "$df"
        tick sa
        tick sb
        sa "EXPORTSESSION $(win "$df")"
        wait_file "$df"
        tick sb
        sa "EXPORTSTATE $(win "$af")"
        sb "EXPORTSTATE $(win "$bf")"
        wait_file "$af"
        wait_file "$bf"
        if assert_py field_agree "$field" "$af" "$bf" "$df" "$@" >"$log" 2>&1; then
            cat "$log"
            return 0
        fi
        sleep 0.2
    done
    echo "FAIL: $label: A, B and the document never agreed on the ${field}s after ~10s"
    cat "$log" || true
    exit 1
}

# Each instance's own view right after both moved: informational, since
# whether either had already picked up the other's move depends on frame
# timing (headless frames come per command and from sporadic WM_PAINTs).
show_local() { # $1=label $2...=ids
    local label="$1" af="$out/$1-a-local.json" bf="$out/$1-b-local.json"
    shift
    sa "EXPORTSTATE $(win "$af")"
    sb "EXPORTSTATE $(win "$bf")"
    wait_file "$af"
    wait_file "$bf"
    local id
    for id in "$@"; do
        echo "$label: right after the moves, $id: A shows $(json_get "$af" "$id"), B shows $(json_get "$bf" "$id")"
    done
}
json_get() { # $1=EXPORTSTATE file $2=id: that id's mirror group
    grep -o "\"$2\":\(null\|\"[^\"]*\"\)" "$1" | head -1 | cut -d: -f2- | tr -d '"'
}

timing_mark s8-move-into-group
echo "=== scenario 8: a move into a group reaches the other instance ==="
g8="$(new_node sa NEWGROUP s8g)"
n8="$(new_node sa NEWNOTE s8n)"
sa "MOVETOGROUP $n8 $g8"
wait_agree "scenario 8" "$out/a8.json" "$out/b8.json"
[ "$(assert_py group_of "$out/a8.json" "$n8" | tr -d '\r')" = "$g8" ] || {
    echo "FAIL: scenario 8: the document doesn't put $n8 in $g8"
    exit 1
}
wait_mirror s8 "$n8" "$g8"
echo "scenario 8: ok"

timing_mark s9-two-moves
echo "=== scenario 9: both move one node into different groups ==="
g9a="$(new_node sa NEWGROUP s9ga)"
g9b="$(new_node sa NEWGROUP s9gb)"
n9="$(new_node sa NEWNOTE s9n)"
# B needs all three before it can offer the move.
wait_mirror s9-setup "$g9a" "$g9b" "$n9"
cp "$out/s9-setup-doc.json" "$out/a9-before.json"
# Back to back, so each move is likely made before that instance picks up
# the other's.
sa "MOVETOGROUP $n9 $g9a"
sb "MOVETOGROUP $n9 $g9b"
show_local s9 "$n9"
wait_agree "scenario 9" "$out/a9.json" "$out/b9.json"
wait_mirror s9 "$n9" "$g9a" "$g9b"
got9="$(assert_py group_of "$out/s9-doc.json" "$n9" | tr -d '\r')"
[ "$got9" = "$g9a" ] || [ "$got9" = "$g9b" ] || {
    echo "FAIL: scenario 9: $n9 ended in $got9, neither $g9a (A's) nor $g9b (B's)"
    exit 1
}
assert_py kept_ids "$out/a9-before.json" "$out/s9-doc.json"
assert_py no_cycle "$out/s9-doc.json"
echo "scenario 9: ok ($n9 in $([ "$got9" = "$g9a" ] && echo "A's" || echo "B's") group)"

timing_mark s10-cycle
echo "=== scenario 10: A moves G into H while B moves H into G ==="
g10="$(new_node sa NEWGROUP s10g)"
h10="$(new_node sa NEWGROUP s10h)"
wait_mirror s10-setup "$g10" "$h10"
cp "$out/s10-setup-doc.json" "$out/a10-before.json"
sa "MOVETOGROUP $g10 $h10"
sb "MOVETOGROUP $h10 $g10"
show_local s10 "$g10" "$h10"
wait_agree "scenario 10" "$out/a10.json" "$out/b10.json"
wait_mirror s10 "$g10" "$h10"
assert_py kept_ids "$out/a10-before.json" "$out/s10-doc.json"
assert_py no_cycle "$out/s10-doc.json"
gg="$(assert_py group_of "$out/s10-doc.json" "$g10" | tr -d '\r')"
hg="$(assert_py group_of "$out/s10-doc.json" "$h10" | tr -d '\r')"
echo "scenario 10: ok (G in $gg, H in $hg)"

# $1=field $2=label $3=id: the value A showed in the last wait_field for
# that label, as JSON.
shown() { assert_py field_of "$1" "$out/$2-a-state.json" "$3" | tr -d '\r'; }
# Each instance's own value right after both edited: informational, as
# show_local.
show_local_field() { # $1=field $2=label $3=id
    local af="$out/$2-a-local.json" bf="$out/$2-b-local.json"
    sa "EXPORTSTATE $(win "$af")"
    sb "EXPORTSTATE $(win "$bf")"
    wait_file "$af"
    wait_file "$bf"
    echo "$2: right after the edits, $3: A shows $(assert_py field_of "$1" "$af" "$3" | tr -d '\r')," \
        "B shows $(assert_py field_of "$1" "$bf" "$3" | tr -d '\r')"
}

timing_mark s11-rename
echo "=== scenario 11: a group rename reaches the other instance ==="
g11="$(new_node sa NEWGROUP s11g)"
# B needs the group before it can rename it.
wait_field name s11-setup "$g11"
sa "RENAMEGROUP $g11 Renamed by A"
wait_agree "scenario 11" "$out/a11.json" "$out/b11.json"
wait_field name s11 "$g11"
[ "$(shown name s11 "$g11")" = '"Renamed by A"' ] || {
    echo "FAIL: scenario 11: $g11 is named $(shown name s11 "$g11"), not A's rename"
    exit 1
}
# Back to back, so each rename is likely made before that instance picks up
# the other's.
sa "RENAMEGROUP $g11 Both A"
sb "RENAMEGROUP $g11 Both B"
show_local_field name s11c "$g11"
wait_agree "scenario 11" "$out/a11c.json" "$out/b11c.json"
wait_field name s11c "$g11"
got11="$(shown name s11c "$g11")"
[ "$got11" = '"Both A"' ] || [ "$got11" = '"Both B"' ] || {
    echo "FAIL: scenario 11: $g11 ended named $got11, neither A's nor B's rename"
    exit 1
}
echo "scenario 11: ok ($g11 named $got11)"

timing_mark s12-note-text
echo "=== scenario 12: a note's text and a move reach the other instance ==="
n12="$(new_node sa NEWNOTE s12n)"
wait_field text s12-setup "$n12"
sa "SETNOTETEXT $n12 Typed by A"
wait_agree "scenario 12" "$out/a12.json" "$out/b12.json"
wait_field text s12 "$n12"
[ "$(shown text s12 "$n12")" = '"Typed by A"' ] || {
    echo "FAIL: scenario 12: $n12 holds $(shown text s12 "$n12"), not A's text"
    exit 1
}
sa "SETNOTETEXT $n12 Both typed A"
sb "SETNOTETEXT $n12 Both typed B"
show_local_field text s12c "$n12"
wait_agree "scenario 12" "$out/a12c.json" "$out/b12c.json"
wait_field text s12c "$n12"
got12="$(shown text s12c "$n12")"
[ "$got12" = '"Both typed A"' ] || [ "$got12" = '"Both typed B"' ] || {
    echo "FAIL: scenario 12: $n12 ended holding $got12, neither A's nor B's text"
    exit 1
}
# The mirror and the document must stay agreed, not just reach agreement
# once: a later save on either side (B's here, A's in wait_field's
# EXPORTSESSION) mustn't bring a losing text back (`commit_note_buffers`
# commits any box that differs from the mirror).
sb "EXPORTSESSION $(win "$out/s12c2-b-save.json")"
wait_file "$out/s12c2-b-save.json"
wait_field text s12c2 "$n12"
[ "$(shown text s12c2 "$n12")" = "$got12" ] || {
    echo "FAIL: scenario 12: $n12 went from $got12 to $(shown text s12c2 "$n12") with no edit"
    exit 1
}
sa "MOVENODE $n12 321 -123"
wait_agree "scenario 12" "$out/a12m.json" "$out/b12m.json"
wait_field pos s12m "$n12"
[ "$(shown pos s12m "$n12")" = '[321.0, -123.0]' ] || {
    echo "FAIL: scenario 12: $n12 is at $(shown pos s12m "$n12"), not where A dropped it"
    exit 1
}
echo "scenario 12: ok ($n12 holds $got12)"

timing_mark s3-nonce-site
echo "=== scenario 3: a url that changes on every load ==="
python -P "$repo_root/scripts/nonce_server.py" "$out/hits" 240 >"$out/port" 2>"$out/server.err" </dev/null &
nonce_pid=$!
wait_file "$out/port"
port="$(tr -d '\r\n' <"$out/port")"
hits() { cat "$out/hits" 2>/dev/null || echo 0; }
sa NEWTAB
sa WAITTABS
# The new tab is focused but shown in Graph view headless.
sa TOGGLEVIEW
# Let the new tab's own first load settle, or its address-bar update
# overwrites the typed url before ENTER.
sa WAITIDLE
sa "ADDRESSBAR http://127.0.0.1:$port/r"
sa WAITIDLE
sa ENTER
# B's copy of A's new tab arrives asleep (spec "Lazy tabs"): once A's url
# has reached it, open it there, so B loads it and follows from then on
# (scenario 4 too). Its landing is a host-started load, which is what
# keeps B's own redirect local below.
for _ in $(seq 300); do
    rm -f "$out/b3.json"
    tick sb
    sb "EXPORTSESSION $(win "$out/b3.json")"
    wait_file "$out/b3.json"
    grep -q "127.0.0.1:$port" "$out/b3.json" && break
    sleep 0.2
done
grep -q "127.0.0.1:$port" "$out/b3.json" || {
    echo "FAIL: scenario 3: A's url never reached B"
    exit 1
}
sb QUICKSWITCHOPEN
sb "QUICKSWITCHTEXT 127.0.0.1:$port"
sb QUICKSWITCHENTER
# Back to Graph view, where B was: C restores B's personal state, and
# scenario 7 toggles C into Tab view from Graph.
sb TOGGLEVIEW
# A's load (a redirect, then the page), then B's (the same, as its copy
# follows): at least 4 requests.
for _ in $(seq 600); do [ "$(hits)" -ge 4 ] && break; sleep 0.2; done
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

timing_mark s4-click-vs-script
echo "=== scenario 4: a click is shared, a script's navigation isn't ==="
# "<r-repeats> <dest-repeats> <dests> <spa-clicks> <spa-timers>", see
# nonce_server.py.
repeats() { cat "$out/hits.repeats" 2>/dev/null || echo "0 0 0 0 0"; }
field() { repeats | tr -d '\r\n' | cut -d' ' -f"$1"; }
# Requests the page server has logged for the exact path $1.
served() { local n; n="$(grep -c " $1\$" "$out/hits.log" 2>/dev/null || true)"; echo "${n:-0}"; }
# Waits for the server to log one more request for path $1 than $2, then for
# A's tab to finish loading it.
wait_served() {
    for _ in $(seq 300); do
        [ "$(served "$1")" -gt "$2" ] && break
        sleep 0.2
    done
    [ "$(served "$1")" -gt "$2" ] || { echo "FAIL: the server never saw a request for $1"; exit 1; }
    sa WAITIDLE
}
before="$(field 1)"
link_before="$(served /link)"
sa "ADDRESSBAR http://127.0.0.1:$port/link"
sa WAITIDLE
sa ENTER
wait_served /link "$link_before"
# The page is one window-sized link, so any click below the toolbar hits it.
sa "CLICK 400 300"
# A lands on a served-once /r?n= and shares it; B asks for it again.
for _ in $(seq 300); do [ "$(field 1)" -gt "$before" ] && break; sleep 0.2; done
[ "$(field 1)" -gt "$before" ] || {
    echo "FAIL: scenario 4: B never followed A's link click"
    exit 1
}
sa "ADDRESSBAR http://127.0.0.1:$port/js"
sa WAITIDLE
sa ENTER
# A's page and then B's (it follows the typed url) each navigate by script
# to their own /dest; had A's been shared, B would ask for A's too.
for _ in $(seq 300); do [ "$(field 3)" -ge 2 ] && break; sleep 0.2; done
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
spa_before="$(served /spa)"
sa "ADDRESSBAR http://127.0.0.1:$port/spa"
sa WAITIDLE
sa ENTER
wait_served /spa "$spa_before"
sa "CLICK 400 300"
for _ in $(seq 300); do [ "$(field 4)" -ge 1 ] && break; sleep 0.2; done
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

timing_mark s5-json-convert
echo "=== scenario 5: a shared tab converting to a node ==="
# A navigates the shared tab to a local JSON file (no network); B follows.
# Both engines load it and report content, but the document must end up
# with exactly one more Json node, not one per instance.
echo '{"converted": true}' >"$out/c5.json"
c5_url="file:///$(cygpath -m "$out/c5.json" | sed 's/ /%20/g')"
sa "EXPORTSESSION $(win "$out/a5-before.json")"
wait_file "$out/a5-before.json"
json_before="$(assert_py count_kind "$out/a5-before.json" application/json | tr -d '\r')"
sa "ADDRESSBAR $c5_url"
sa WAITIDLE
sa ENTER
for _ in $(seq 300); do
    rm -f "$out/a5.json"
    tick sa
    sa "EXPORTSESSION $(win "$out/a5.json")"
    wait_file "$out/a5.json"
    [ "$(assert_py count_kind "$out/a5.json" application/json | tr -d '\r')" -gt "$json_before" ] && break
    sleep 0.2
done
# Time for B to follow the url, load it and convert too, if it's going to.
sleep 8
wait_agree "scenario 5" "$out/a5.json" "$out/b5.json"
json_after="$(assert_py count_kind "$out/a5.json" application/json | tr -d '\r')"
[ "$json_after" -eq $((json_before + 1)) ] || {
    echo "FAIL: scenario 5: Json nodes went $json_before -> $json_after, want exactly one more"
    exit 1
}
echo "scenario 5: ok"

timing_mark s6-kill
echo "=== scenario 6: kill A ==="
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
c_winpid="$(winpid_of_child "$C_PID")" || {
    echo "FAIL: couldn't find C's manifold.exe Windows PID"
    exit 1
}
sc "EXPORTSESSION $(win "$out/c.json")"
wait_file "$out/c.json"
assert_py has_id "$out/c.json" "$new_id"
echo "scenario 6: ok"

timing_mark s7-restart
echo "=== scenario 7: a restart reopens a tab's live url ==="
# C is the only instance, so its personal state is the one on disk after
# it quits (two instances would each overwrite the other's on save).
js_loads() { grep -c ' /js$' "$out/hits.log" || true; }
dests="$(field 3)"
js_first="$(served /js)"
sc NEWTAB
sc WAITTABS
sc TOGGLEVIEW
# Typed text is dropped if the view changes before the bar takes focus, and a
# new tab's first load can overwrite it before ENTER; under load either loses
# the request, so type again once if the server never sees /js.
for attempt in 1 2; do
    sc WAITIDLE
    sc "ADDRESSBAR http://127.0.0.1:$port/js"
    sc WAITIDLE
    sc ENTER
    for _ in $(seq 50); do [ "$(served /js)" -gt "$js_first" ] && break; sleep 0.2; done
    [ "$(served /js)" -gt "$js_first" ] && break
done
[ "$(served /js)" -gt "$js_first" ] || {
    echo "FAIL: scenario 7: C never requested /js (the address bar text was lost)"
    cat "$out/hits.log" "$out/hits.repeats"
    exit 1
}
for _ in $(seq 300); do [ "$(field 3)" -gt "$dests" ] && break; sleep 0.2; done
[ "$(field 3)" -gt "$dests" ] || {
    echo "FAIL: scenario 7: C's /js page never navigated to /dest"
    cat "$out/hits.log" "$out/hits.repeats"
    exit 1
}
# Let C see its tab settle on /dest before quitting saves it.
sc "SLEEP 2"
js_before="$(js_loads)"
dest_repeats="$(field 2)"
sc QUIT
wait "$C_PID" 2>/dev/null || true
c_winpid=""

coproc D { "$exe" --headless; }
sd() { echo "$1" >&"${D[1]}"; }
d_winpid="$(winpid_of_child "$D_PID")" || {
    echo "FAIL: couldn't find D's manifold.exe Windows PID"
    exit 1
}
sd WAITTABS
# D asks again for the /dest?n= C's script chose (a dest repeat).
for _ in $(seq 300); do [ "$(field 2)" -gt "$dest_repeats" ] && break; sleep 0.2; done
[ "$(field 2)" -gt "$dest_repeats" ] || {
    echo "FAIL: scenario 7: D didn't reopen C's /dest url"
    cat "$out/hits.log"
    exit 1
}
[ "$(js_loads)" -eq "$js_before" ] || {
    echo "FAIL: scenario 7: D loaded the typed /js again"
    exit 1
}
sd QUIT
wait "$D_PID" 2>/dev/null || true
d_winpid=""
echo "scenario 7: ok"

timing_mark
echo "two-instance test: ok"
