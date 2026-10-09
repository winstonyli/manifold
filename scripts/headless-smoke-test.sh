#!/usr/bin/env bash
# Smoke test for manifold's --headless mode (see README.md's "Headless
# mode" section for the stdin text-command protocol this drives). Not a
# full test framework and not pixel comparison: it checks each screenshot
# was produced and isn't suspiciously small, and asserts on app state
# (view mode, the focused tab's URL, the suggestion rows and which one is
# highlighted) read back through EXPORTSTATE, so
# it catches wrong behavior as well as crashes. Exercises the address-bar
# autocomplete flow end to end: launch (into Graph view), click the root
# node to enter Tab view, type a query, screenshot the suggestion dropdown,
# click a suggestion, screenshot the result; arrow-key highlight +
# Enter-accept; Escape dismissing the dropdown without leaving Tab view;
# entering and leaving a group in Graph view; tab drops inside a group
# (reparent, the cycle refusal) and ejecting a rectangle-selected tab;
# the overloaded and empty groups' border colors (needs Pillow);
# closing a group holding an image and a nested group, then reopening
# it (REOPENTAB) nested as it was; `ADDFILE` for each content kind; a tab
# at a local JSON or PDF converting into its node; entering a dropped
# JSON's level via JSONSELECT/JSONSEARCH and leaving it, NEWNOTE from that level; clicking a moved JSON window's "+ N earlier" row; quit. The drop checks
# click cards where EXPORTSTATE's `cards` says they are, after the layout
# stops moving (`settle`), not at fixed coordinates.
#
# Deliberately NOT covered: clicking an open-tab suggestion (accepting it
# focuses another tab), left to manual verification (see project memory
# for the one-off manual pass that confirmed it works). EXPORTSTATE's
# `cards` could now find the second tab's node to click.
#
# Run after `cargo build -p manifold`. Exits 0 if every check passed.
set -uo pipefail # not -e: a failed check should still let later checks run

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
out_dir="${HEADLESS_TEST_OUT:-$repo_root/target/headless-test-output}"

if [ ! -x "$exe" ]; then
    echo "headless-smoke-test: $exe not found — build first (cargo build -p manifold)" >&2
    exit 1
fi
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
require_fresh_exe "$exe" || exit 1
# scripts/smoke_eval.py's checks read EXPORTSTATE cards through scripts/export_state.py.
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"

mkdir -p "$out_dir"
rm -f "$out_dir"/*.png "$out_dir"/*.json

# `SCREENSHOT`'s path travels over stdin, not argv — MSYS/Git-Bash's
# automatic POSIX-to-Windows path translation for a spawned native exe
# only rewrites its command-line *arguments*, not bytes a script writes
# into the child's stdin afterward. `$out_dir` as-is (`/c/Users/...`)
# isn't a valid Windows path and silently fails to save; `cygpath -w`
# converts it once, up front, into the `C:\Users\...` form the Rust side
# actually needs.
out_dir_win="$(cygpath -w "$out_dir")"

# A fresh profile of its own, so the real %APPDATA%\manifold is never
# read or written (and a real session can't move the root tab the click
# below expects). `has_shown_first_launch_demo` skips the demo scene, so
# the app starts with just the root tab. Other running manifold windows
# are left alone: an engine pipe name they hold makes `spawn_tab` fall
# back to a fresh one, and engines never outlive their manifold.exe (its
# Job Object), so there are no orphans to clear first.
profile="$repo_root/target/headless-test-appdata-$$"
mkdir -p "$profile/manifold"
echo '{"has_shown_first_launch_demo": true}' >"$profile/manifold/settings.json"
export APPDATA="$(cygpath -w "$profile")"

# Address-bar suggestions come from a local server (probe_server.py's /ac/), not DuckDuckGo, so those checks
# don't depend on the network. Its lifetime covers the whole run; `cleanup` kills it.
python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$profile/port")" "$(cygpath -w "$profile/page.log")" 1800 </dev/null >/dev/null 2>&1 &
probe_pid=$!
for _ in $(seq 300); do [ -s "$profile/port" ] && break; sleep 0.1; done
export MANIFOLD_SUGGESTIONS_URL="http://127.0.0.1:$(cat "$profile/port")/ac/"

fail=0
pass() { echo "PASS: $1"; }
failed() {
    echo "FAIL: $1"
    fail=1
}

# The app saves a SCREENSHOT on its next drawn frame, which can take
# seconds when the GPU is busy; wait up to the 30 s the app itself gives
# one, then for the size to stop changing (a save in progress; an empty
# file counts as not written yet).
wait_for_file() {
    local path="$1" waited=0 prev=-1 now
    while [ ! -s "$path" ] && [ "$waited" -lt 150 ]; do
        sleep 0.2
        waited=$((waited + 1))
    done
    while [ -f "$path" ] && now=$(stat -c%s "$path") && [ "$now" != "$prev" ]; do
        prev=$now
        sleep 0.2
    done
}

check_screenshot() {
    local path="$1" label="$2" min_bytes="${3:-2000}"
    wait_for_file "$path"
    if [ ! -f "$path" ]; then
        failed "$label: screenshot not created ($path)"
        return
    fi
    local size
    size=$(stat -c%s "$path" 2>/dev/null || stat -f%z "$path" 2>/dev/null || echo 0)
    if [ "$size" -lt "$min_bytes" ]; then
        failed "$label: screenshot suspiciously small ($size bytes)"
    else
        pass "$label: screenshot looks reasonable ($size bytes)"
    fi
}

# EXPORTSTATE writes the app's UI state as JSON (see its doc comment in
# debug_control.rs). `state NAME EXPR` exports it to NAME.json and
# evaluates the Python expression EXPR over `s` (that state), `url` (the
# focused tab's URL), `q` (that URL's `q=` search query, or "") and `row`
# (the ROW environment variable, for comparing against a captured value): it
# prints the value, or the URL with a third argument `url`, and succeeds
# if the value is truthy.
state() {
    local path="$out_dir/$1.json"
    rm -f "$path"
    send "EXPORTSTATE $out_dir_win\\$1.json"
    wait_for_file "$path"
    python -P "$repo_root/scripts/smoke_eval.py" state "$path" "$2" "${3:-}" </dev/null
}

check_state() {
    local name="$1" label="$2" expr="$3" result
    if result=$(state "$name" "$expr" url); then
        pass "$label ($result)"
    else
        failed "$label: \`$expr\` is false (focused tab: ${result:-none})"
    fi
}

# Polls until EXPR holds, for up to ~15 s: suggestions are fetched on a
# background thread after a debounce, so a fixed sleep either wastes time or
# isn't enough.
wait_state() {
    local name="$1" label="$2" expr="$3" i
    for i in $(seq 1 30); do
        if state "$name" "$expr" >/dev/null; then
            pass "$label"
            return
        fi
        sleep 0.5
    done
    failed "$label: \`$expr\` still false after ~15 s"
}

# `coproc` gives a bidirectional pipe to the child — the same shape
# `--headless` is designed for (a driving script writing commands, reading
# nothing back except the exit code), without needing a second process
# just to hold stdin open.
coproc BROWSER { "$exe" --headless; }
browser_pid=$BROWSER_PID

send() { echo "$1" >&"${BROWSER[1]}"; }

# The coproc's pid is the MSYS proxy whose Windows tree holds manifold.exe (two-instance-test.sh says how), so
# kill_tree, not kill: a kill of that pid stops only the proxy, and manifold.exe could outlive an interrupted run
# with its profile locked (ten such profiles were left in target/). The profile's session.mnf.lock goes only once
# manifold.exe is gone, so the rm retries for up to 5 s.
cleanup() {
    kill_tree "$browser_pid"
    kill_tree "$probe_pid"
    wait "$browser_pid" 2>/dev/null
    for _ in 1 2 3 4 5; do
        rm -rf "$profile" 2>/dev/null
        [ -e "$profile" ] || break
        sleep 1
    done
}
trap cleanup EXIT

# Pin a known window size so the click coordinates below (measured against
# this exact size during manual verification) stay valid regardless of
# whatever winit's platform-default window size happens to be.
send "RESIZE 816 639"

# The root tab spawns asynchronously (30 s or more on a loaded machine);
# a fixed sleep here used to race it.
send "WAITTABS"

# The app launches into Graph view; with only the root tab, the layout
# puts it at the window's center (~(407, 332) at 816x639). Clicking it
# enters Tab view, which the address-bar flow below needs. This CLICK is
# the first command after the tab spawns, so it lands only because
# headless mode draws a frame before each pointer command (it missed 3
# runs out of 3 without that). Keep it first: any command in between
# renders a frame of its own and would hide a regression.
send "CLICK 407 332"
send WAITIDLE
check_state tab_view "clicking the just-spawned root node enters Tab view" 's["tab_view"]'

send "ADDRESSBAR weather"
wait_state weather_rows "suggestions for \"weather\" arrive" 's["suggestions"]'
send "SCREENSHOT $out_dir_win\\dropdown.png"
check_screenshot "$out_dir/dropdown.png" "autocomplete dropdown"
first_row=$(state weather_rows 's["suggestions"][0]')

# First suggestion row's position below the address bar, at the 816x639
# size pinned above — re-measure (via a manual SCREENSHOT) if chrome
# layout constants (CHROME_HEIGHT_POINTS in main.rs) ever change. (Already
# had to move this twice — most recently when Tab view's separate tab-strip
# and toolbar bars were merged into one, shrinking chrome height 58->32.)
send "CLICK 283 42"
ROW="$first_row" wait_state after_click "clicking the first suggestion searches it" 'q == row'
send WAITIDLE # first paint
send "SCREENSHOT $out_dir_win\\after_click.png"
check_screenshot "$out_dir/after_click.png" "post-click navigation"

# Arrow-key highlight + Enter-accept: two ARROWDOWNs highlight the 2nd row
# (the first press highlights the 1st), and ENTER searches that row's
# query. No coordinates involved.
send "ADDRESSBAR sourdough bread"
wait_state sourdough_rows "suggestions for \"sourdough bread\" arrive" 'len(s["suggestions"]) >= 2'
send "ARROWDOWN"
send "ARROWDOWN"
check_state highlighted "two ARROWDOWNs highlight the 2nd row" 's["highlighted"] == 1'
second_row=$(state highlighted 's["suggestions"][1]')
send "ENTER"
ROW="$second_row" wait_state arrow_nav "Enter searches the highlighted row" 'q == row'
send WAITIDLE # first paint
send "SCREENSHOT $out_dir_win\\arrow_nav.png"
check_screenshot "$out_dir/arrow_nav.png" "arrow-key navigation"

# Escape dismisses the dropdown without leaving Tab view (as opposed to a
# second Escape / one with no dropdown showing, which leaves to Graph
# view), and navigates nowhere.
send "ADDRESSBAR pancake recipe"
wait_state pancake_rows "suggestions for \"pancake recipe\" arrive" 's["suggestions"]'
send "ESCAPE"
send "SCREENSHOT $out_dir_win\\escape_dismiss.png"
check_screenshot "$out_dir/escape_dismiss.png" "Escape dismiss"
ROW="$second_row" check_state escape_dismiss "Escape closes the dropdown, stays in Tab view, navigates nowhere" \
    's["tab_view"] and not s["suggestions"] and q == row'

# Card helpers. `query NAME EXPR` evaluates EXPR over an
# already-exported NAME.json without exporting again: `s` (the state),
# `cards` (by id), `tabs` (live or asleep) and `groups` (sorted ids),
# `xy(id)` and `lt(id)` (a card's center and top-left corner, where its
# eject button sits, as "x y") and `bbox` (every tab's rects' bounds grown
# by 20 and kept inside the window, as "x0 y0 x1 y1"). `xy` and `lt` say
# on stderr when the point is off the graph canvas (under the toolbar or
# outside the window), where a click can't reach the card. EXPR also sees
# scripts/export_state.py's names: `card_is(c, TYPE[, loaded])` and the
# type constants (`TAB`, `NOTE`, `JSON`, ...).
query() {
    python -P "$repo_root/scripts/smoke_eval.py" query "$out_dir/$1.json" "$2" </dev/null
}

check_query() {
    if [ "$(query "$1" "bool($3)")" = True ]; then
        pass "$2"
    else
        # What the state actually was, so a flake explains itself without a rerun (the next run wipes the JSON).
        failed "$2: \`$3\` is false in $1.json (saw: $(query "$1" '"tab_view=%s group=%s tabs=%s groups=%s" % (s["tab_view"], s["current_group"], tabs, groups)' 2>&1 | tail -1))"
    fi
}

# Exports until two exports in a row agree on every card's rect, leaving
# the last as NAME.json. After entering a group the camera eases, and
# SLEEP draws no frames (each command draws one), so a fixed sleep can
# read positions mid-ease; a drag there lands on empty canvas.
settle() {
    local prev="" now i
    for i in $(seq 1 40); do
        state "$1" 1 >/dev/null
        now=$(query "$1" 'sorted((i, [round(v, 1) for v in c["rect"]]) for i, c in cards.items())')
        [ "$now" = "$prev" ] && return 0
        prev=$now
        sleep 0.3
    done
    failed "$1: the layout never stopped moving"
}

# Group drill-in: a second Escape (no dropdown now) leaves to Graph view,
# NEWGROUP adds a group card, DOUBLECLICK enters it and the toolbar's back
# button leaves it. The card is found by its reported position: spawn
# jitter comes from the id, and ids are random per launch, so it can
# land at the window's edge or under the minimap: Reset View (the
# toolbar's fit button) first brings every card into view, which also
# hides the minimap. DOUBLECLICK rather than two CLICKs, which a slow
# frame can push past egui's double-click delay.
send "ESCAPE"
send "NEWGROUP"
send WAITIDLE # Tab-to-Graph transition, layout settle
send "SCREENSHOT $out_dir_win\group_card.png"
check_screenshot "$out_dir/group_card.png" "group card"
# A "Saved <path>" toast would cover the bottom-left of the window, and a
# card placed there would take the next click on the toast instead.
check_state group_card "a headless screenshot shows no toast" 's["toast"] is None'
send "CLICK 326 17"
settle group_card
first_group=$(query group_card 'groups[0]')
send "DOUBLECLICK $(query group_card "xy('$first_group')")"
check_state group_in "double-clicking the group card enters the group" 'not s["tab_view"] and s["current_group"]'
send "CLICK 22 17"
check_state group_out "the back button leaves the group" 's["current_group"] is None'

# Tab drops inside a group.

# `drag X0 Y0 X1 Y1 [button]`: a press, six moves and a release with no
# sleep between them (see debug_control.rs's advice on drags).
drag() {
    local i
    send "MOUSEDOWN $1 $2${5:+ $5}"
    for i in 1 2 3 4 5 6; do send "MOUSEMOVE $(($1 + ($3 - $1) * i / 6)) $(($2 + ($4 - $2) * i / 6))"; done
    send "MOUSEUP $3 $4${5:+ $5}"
}

# A two-tab group from a file, entered by double-clicking its card.
cat >"$out_dir/pair.group.json" <<'EOF'
{"nodes": [
 {"id": "g", "type": "application/vnd.manifold.group", "name": "Pair"},
 {"id": "a", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
EOF
before=$(query group_out 'groups')
send "IMPORTGROUP $out_dir_win\pair.group.json"
send "WAITTABS"
# The imported card lands wherever the jitter puts it, which can be off
# screen at this zoom: the toolbar's Reset View (fit all) button brings
# every card into view (its spot at 816 wide, as measured from a
# SCREENSHOT; re-measure if the Graph toolbar changes).
send "CLICK 326 17"
settle pair_top
pair=$(BEFORE="$before" query pair_top '[g for g in groups if g not in eval(env["BEFORE"])][0]')
send "DOUBLECLICK $(query pair_top "xy('$pair')")"
settle pair_in
check_query pair_in "double-clicking the imported group's card enters it" "s['current_group'] == '$pair' and len(tabs) == 2"
read -r m1 m2 <<<"$(query pair_in '" ".join(tabs)')"

drag $(query pair_in "xy('$m1')") $(query pair_in "xy('$m2')")
settle reparented
check_query reparented "a tab dropped on a sibling tab inside a group is reparented under it" \
    "cards['$m1']['parent'] == '$m2'"
# The pair now overlaps exactly, and which is on top follows spawn order,
# so move whichever the press picks up onto empty canvas first, toward
# the window's middle so it stays in view.
read -r x y <<<"$(query reparented "xy('$m1')")"
drag "$x" "$y" $((x > 408 ? x - 250 : x + 250)) $((y > 340 ? y - 150 : y + 150))
settle apart
drag $(query apart "xy('$m2')") $(query apart "xy('$m1')")
settle cycle
PREV="$(query apart "cards['$m2']['screen']")" check_query cycle "dropping a tab onto its own child is refused" \
    "cards['$m2']['screen'] != eval(env['PREV']) and cards['$m2']['parent'] is None and cards['$m1']['parent'] == '$m2'"

# A right-drag marquee selects both; ejecting one through its button
# takes it out of the selection too (the pair overlaps again, so either
# may be the one ejected).
drag $(query cycle 'bbox') right
settle selected
check_query selected "a right-drag marquee selects both tabs" "s['selected_tabs'] == tabs"
read -r x y <<<"$(query selected "xy('$m1')")"
read -r ex ey <<<"$(query selected "lt('$m1')")"
send "MOUSEMOVE $x $y"
send "MOUSEMOVE $ex $ey"
send "CLICK $ex $ey"
settle ejected
check_query ejected "ejecting a selected tab also drops it from the selection" \
    "len(tabs) == 1 and s['selected_tabs'] == tabs"

# Ctrl-click toggles the selected tab out and back in without opening it; a plain click on empty canvas (the
# marquee's start corner) then clears the selection.
read -r x y <<<"$(query ejected "xy(tabs[0])")"
read -r bx by _ <<<"$(query ejected 'bbox')"
send "MOUSEMOVE $x $y"
send "CLICK $x $y ctrl"
settle ctrl_out
check_query ctrl_out "a Ctrl-click on a selected tab takes it out of the selection without opening it" \
    "s['selected_tabs'] == [] and not s['tab_view']"
send "CLICK $x $y ctrl"
settle ctrl_in
check_query ctrl_in "a Ctrl-click on an unselected tab selects it without opening it" \
    "s['selected_tabs'] == tabs and not s['tab_view']"
send "MOUSEMOVE $bx $by"
send "CLICK $bx $by"
settle deselected
check_query deselected "a click on empty canvas clears the selection" "s['selected_tabs'] == []"

# Border colors, read off a screenshot: a group of more than 8 members
# rests on DANGER_COLOR (196, 87, 58), and the empty group from above on
# ACCENT (215, 162, 74). `card_border`'s unit test covers which color
# wins; this checks the member count reaches it and the frame paints it.
# The mouse waits in an empty corner so no card eases toward its hover
# color.
send "CLICK 22 17"
python -P "$repo_root/scripts/smoke_eval.py" nine "$out_dir/nine.group.json" </dev/null
settle top
before_nine=$(query top 'groups')
send "IMPORTGROUP $out_dir_win\nine.group.json"
send "WAITTABS"
send "CLICK 326 17"
send "MOUSEMOVE 806 629"
settle nine
nine=$(BEFORE="$before_nine" query nine '[g for g in groups if g not in eval(env["BEFORE"])][0]')
empty=$(BEFORE="$before" query nine 'eval(env["BEFORE"])[0]')
send "SCREENSHOT $out_dir_win\borders.png"
check_screenshot "$out_dir/borders.png" "group borders"
# `border NAME ID RGB` succeeds if a pixel on ID's left edge, at
# mid-height, is within 40 of RGB (the stroke is about 1 px wide).
border() {
    python -P "$repo_root/scripts/smoke_eval.py" border "$out_dir/$1.json" "$out_dir/borders.png" "$2" "$3" </dev/null
}
if ! python -P -c "import PIL" </dev/null 2>/dev/null; then
    echo "SKIP: group border colors (needs Pillow: pip install pillow)"
else
    if border nine "$nine" "(196, 87, 58)"; then pass "a group of 9 has a DANGER_COLOR border"
    else failed "the 9-member group $nine's border isn't DANGER_COLOR (see borders.png)"; fi
    if border nine "$empty" "(215, 162, 74)" && ! border nine "$empty" "(196, 87, 58)"; then pass "an empty group has an ACCENT border"
    else failed "the empty group $empty's border isn't ACCENT (see borders.png)"; fi
fi

# Recently Closed keeps nesting: fill the empty group with an image and
# the Pair group (so `$empty { image, group { tab, tab } }`), close it
# from the top level, and REOPENTAB (Ctrl+Shift+T) brings the whole
# subtree back as one entry, still nested.
send "DOUBLECLICK $(query nine "xy('$empty')")"
settle outer_in
send "ADDFILE $(cygpath -w "$repo_root")\crates\manifold\assets\icon\icon-64.png"
send "IMPORTGROUP $out_dir_win\pair.group.json"
send "WAITTABS"
settle outer_built
check_query outer_built "a group can hold an image and a nested group" \
    "s['current_group'] == '$empty' and len(groups) == 1 and sum(card_is(c, IMAGE) for c in cards.values()) == 1"
send "CLICK 22 17"
settle outer_back
send "CLOSEGROUP $empty"
settle outer_closed
check_query outer_closed "closing the outer group removes it" "'$empty' not in groups"
send "REOPENTAB"
send "WAITTABS"
settle reopened
outer=$(BEFORE="$(query outer_closed 'groups')" query reopened '([g for g in groups if g not in eval(env["BEFORE"])] or ["none"])[0]')
if [ "$outer" = none ]; then
    failed "REOPENTAB brought no group back"
else
    send "DOUBLECLICK $(query reopened "xy('$outer')")"
    settle reopened_in
    check_query reopened_in "the reopened group has its image and nested group back" \
        "s['current_group'] == '$outer' and len(groups) == 1 and sum(card_is(c, IMAGE) for c in cards.values()) == 1"
    inner=$(query reopened_in '(groups or ["none"])[0]')
    send "DOUBLECLICK $(query reopened_in "xy('$inner')")"
    settle reopened_inner
    check_query reopened_inner "the nested group comes back with both tabs" \
        "s['current_group'] == '$inner' and len(tabs) == 2"
fi

# Drag and drop: `ADDFILE <path> X Y` places a file the way a real drop
# does (`drop_file`), in Graph view here, so each card lands near X Y.
# Same-frame fan-out is left to the unit tests.
printf 'a dropped note\n' >"$out_dir/drop.txt"
printf '{"a": 1}\n' >"$out_dir/drop.json"
state before_drop 1 >/dev/null
before_drop_ids=$(query before_drop 'sorted(cards)')
send "ADDFILE $out_dir_win\\drop.txt 400 300"
send "ADDFILE $out_dir_win\\drop.json 400 300"
settle after_drop
note_id=$(BEFORE="$before_drop_ids" query after_drop \
    '([i for i, c in cards.items() if card_is(c, NOTE) and i not in eval(env["BEFORE"])] or ["none"])[0]')
json_id=$(BEFORE="$before_drop_ids" query after_drop \
    '([i for i, c in cards.items() if card_is(c, JSON) and i not in eval(env["BEFORE"])] or ["none"])[0]')
check_query after_drop "a dropped .txt becomes a Note card" "'$note_id' != 'none'"
check_query after_drop "a dropped .json becomes a Json card" "'$json_id' != 'none'"
# Both land on one point, so the layout eases them about a card apart
# (graph_layout's REPULSION_SOFT_DISTANCE): each moves half a card or so
# (~120px at this zoom), not off-canvas (~700px each before that cap).
check_query after_drop "the dropped note's screen center is within 200px of (400, 300)" \
    "abs(cards['$note_id']['screen'][0] - 400) < 200 and abs(cards['$note_id']['screen'][1] - 300) < 200"
check_query after_drop "the dropped json's screen center is within 200px of (400, 300)" \
    "abs(cards['$json_id']['screen'][0] - 400) < 200 and abs(cards['$json_id']['screen'][1] - 300) < 200"
check_query after_drop "the two cards dropped on one point no longer overlap" \
    "(lambda a, b: a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1])(cards['$note_id']['rect'], cards['$json_id']['rect'])"

# Entering the dropped JSON file: its level, driven by the JSON commands,
# then back out. `{"a": 1}` is one root card with one row.
send "DOUBLECLICK $(query after_drop "xy('$json_id')")"
settle json_in
check_query json_in "double-clicking a Json card enters its level" \
    "s['json_level'] and s['json_level']['root'] == '$json_id' and s['current_group'] == '$json_id'"
check_query json_in "the JSON level shows the root card" \
    "[c['pointer'] for c in s['json_level']['cards']] == [''] and cards['$json_id#']['json_card']"
send "JSONSELECT /a"
send "JSONSEARCH a"
check_state json_search "JSONSELECT and JSONSEARCH reach the level" \
    "s['json_level']['selected'] == '/a' and s['json_level']['matches'] == ['/a']"
send "CLICK 22 17"
check_state json_out "the back button leaves the JSON level" \
    "s['json_level'] is None and s['current_group'] != '$json_id'"

# Creating from a JSON level leaves it first (spec place-node-design): a new
# note lands in the level the user then sees, not out of sight beside the
# Json node.
settle json_out2
send "DOUBLECLICK $(query json_out2 "xy('$json_id')")"
settle json_in2
check_query json_in2 "re-entering the JSON level" "s['json_level'] and s['current_group'] == '$json_id'"
notes_before=$(query json_out2 'sorted(i for i, c in cards.items() if card_is(c, NOTE))')
send "NEWNOTE"
settle json_note
check_query json_note "NEWNOTE from a JSON level leaves it" "s['json_level'] is None and s['current_group'] != '$json_id'"
check_query json_note "the new note is a card on the level now shown" \
    "len([i for i, c in cards.items() if card_is(c, NOTE)]) == len(eval('''$notes_before''')) + 1"

# A far reveal moves a card's window (skip > 0, a `+ N earlier` row under the header); a real CLICK on that row,
# aimed at the `earlier` point EXPORTSTATE's json_level gives, grows the window up by one page (json_view::show_earlier).
printf '{"items": [%s]}\n' "$(seq -s, 0 999)" >"$out_dir/big.json"
state before_big 1 >/dev/null
before_big_ids=$(query before_big 'sorted(cards)')
send "ADDFILE $out_dir_win\\big.json 400 300"
settle after_big
big_id=$(BEFORE="$before_big_ids" query after_big \
    '([i for i, c in cards.items() if card_is(c, JSON) and i not in eval(env["BEFORE"])] or ["none"])[0]')
send "DOUBLECLICK $(query after_big "xy('$big_id')")"
settle big_in
ITEMS="next(c for c in s['json_level']['cards'] if c['pointer'] == '/items')" # the /items card, in a query
send "JSONSELECT /items/900"
send WAITIDLE
settle big_far
check_query big_far "a far JSONSELECT moves the card's window (skip 900) and gives the earlier row a click point" \
    "s['json_level']['root'] == '$big_id' and $ITEMS['skip'] == 900 and $ITEMS['earlier'] is not None"
send "CLICK $(query big_far "' '.join('%.0f' % v for v in $ITEMS['earlier'])")"
send WAITIDLE
settle big_earlier
check_query big_earlier "clicking the + N earlier row grows the window up by one page (skip 880)" \
    "$ITEMS['skip'] == 880"
send "CLICK 22 17"
check_state big_out "the back button leaves the big JSON level" "s['json_level'] is None"

# The remaining `ADDFILE` kinds (the image and the .txt/.json are above),
# each checked by the new card's type: a PDF, a `.bin` (a Download card)
# and a `.md` (a Note by its bytes, not its extension). `.html` becomes a
# live Tab, which is clicked into Tab view to check its `file://` url.
printf '%%PDF-1.4\n' >"$out_dir/doc.pdf"
printf '# a markdown note\n' >"$out_dir/note.md"
printf '\xffbinary\xfedata' >"$out_dir/data.bin"
printf '<!doctype html><html><body>hi</body></html>' >"$out_dir/page.html"
state before_kinds 1 >/dev/null
before_kinds_ids=$(query before_kinds 'sorted(cards)')
send "ADDFILE $out_dir_win\\doc.pdf 600 500"
send "ADDFILE $out_dir_win\\note.md 600 500"
send "ADDFILE $out_dir_win\\data.bin 600 500"
send "ADDFILE $out_dir_win\\page.html 600 500"
send "WAITTABS"
settle after_kinds
# new_card TYPE [True|False]: the id of the one new card of that type (and, for a tab, that `loaded` state).
new_card() {
    BEFORE="$before_kinds_ids" query after_kinds \
        "([i for i, c in cards.items() if card_is(c, $1, ${2:-None}) and i not in eval(env[\"BEFORE\"])] or [\"none\"])[0]"
}
pdf_id=$(new_card PDF)
md_id=$(new_card NOTE)
bin_id=$(new_card DOWNLOAD)
html_id=$(new_card TAB True)
check_query after_kinds "a dropped .pdf becomes a Pdf card" "'$pdf_id' != 'none'"
check_query after_kinds "a dropped .md becomes a Note card" "'$md_id' != 'none'"
check_query after_kinds "a dropped .bin becomes a Download card" "'$bin_id' != 'none'"
check_query after_kinds "a dropped .html becomes a Tab card" "'$html_id' != 'none'"
if [ "$html_id" != none ]; then
    send "CLICK $(query after_kinds "xy('$html_id')")"
    wait_state html_tab "the dropped .html tab's url is a file:// URL" 'url.startswith("file://")'
fi

# Engine-side conversion, no network: a tab typed to a local JSON or
# PDF file goes, and a node of that kind takes its place, keeping the
# file:// url. Checked on EXPORTSESSION, which carries each node's url.
printf '{"converted": [1, 2]}\n' >"$out_dir/conv.json"
printf '%%PDF-1.4\n' >"$out_dir/conv.pdf"
# `converted NAME FILE KIND` succeeds once NAME.json (a session export) has
# a KIND node whose url is a file:// one ending in /FILE, and no tab there.
converted() {
    python -P "$repo_root/scripts/smoke_eval.py" converted "$out_dir/$1.json" "$2" "$3" </dev/null
}
# NEWTAB doesn't focus the tab it opens, and a conversion drops to Graph
# view with nothing focused, so this types into whichever tab NEXTTAB
# lands on.
convert_focused_tab() { # $1=file $2=kind (as EXPORTSESSION names it)
    local url name="conv_$2" i
    url="file:///$(cygpath -m "$out_dir/$1" | sed 's/ /%20/g')"
    send "WAITTABS"
    state "focus_$2" 's["tab_view"]' >/dev/null || send "NEXTTAB"
    # Let the tab's own load settle, or its address-bar update overwrites
    # the typed url before ENTER.
    send WAITIDLE
    if ! state "focus_$2" 's["tab_view"] and url' >/dev/null; then
        failed "no tab focused to type $1 into (see focus_$2.json)"
        return
    fi
    send "ADDRESSBAR $url"
    send WAITIDLE
    send "ENTER"
    for i in $(seq 1 150); do
        rm -f "$out_dir/$name.json"
        send "EXPORTSESSION $out_dir_win\\$name.json"
        wait_for_file "$out_dir/$name.json"
        if converted "$name" "$1" "$2"; then
            pass "a tab at a local $1 becomes a $2 node with its file:// url"
            return
        fi
        sleep 0.3
    done
    failed "a tab at a local $1 never became a $2 node (see $name.json)"
}
convert_focused_tab conv.json json
convert_focused_tab conv.pdf pdf

send "QUIT"
if wait "$browser_pid"; then
    pass "process exited cleanly"
else
    failed "process did not exit cleanly (exit code $?)"
fi

echo
if [ "$fail" -eq 0 ]; then
    echo "All checks passed. Screenshots in $out_dir"
else
    echo "Some checks failed. Screenshots in $out_dir"
fi
exit "$fail"
