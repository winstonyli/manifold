#!/usr/bin/env bash
# Usage: [VIEWS="graph tabview ..."] [SIZE="1280 800"] [OUT=target/shots] scripts/chrome-shots.sh
#
# Screenshots of the app's chrome in every view, for iterating on the UI.
# One headless instance on a fresh isolated profile with a small seeded
# session (a tab, a group with a tab in it, a note, an image, a JSON file);
# every tab is `about:blank`, so no page content competes with the chrome.
# Writes, under $OUT (default target/shots):
#   <view>.png          the whole window, as `SCREENSHOT` composites it
#   crop/<view>.png     the top CROP_PX rows (toolbar and address bar)
#   sheet.png           every view in one grid (needs Pillow, which is optional)
# Each view starts from Graph view at the top level and leaves the app
# there, so VIEWS picks any subset and a run is ~40 s. Views:
#   graph graph-hover graph-menu-tab graph-menu-group graph-menu-note graph-menu-jars graph-menu-chip graph-menu-color graph-menu-group-jars
#   graph-link graph-menu-links graph-link-hover graph-link-handles graph-link-drag
#   graph-filter graph-group json-level json-menu
#   tabview tabview-dropdown tabview-find tabview-actions
#   tabview-split settings popup-bookmarks popup-history popup-downloads
#   shortcuts quickswitch graph-closed graph-newnode-menu newnode-zoom graph-tooltip image-view cards-f note-edit missing-file download-real download-progress download-failed json-zoom
#   settings-engine settings-engine-tab settings-combo toast tabview-engine json-tooltip
#   engine-states-picker engine-states-settings engine-install-confirm (the CEF install states: any of the
#   three runs one second instance, a copy of the exe on its own profile, and writes engine-states-* for
#   Not installed, Failed, Installing, Installed, Update needed and Custom (valid and refused) in the
#   Settings page and the default-engine picker, engine-states-failed-tab (the card of a CEF tab with no
#   runtime) and engine-install-confirm; see the comment above that block for how each state is reached)
# Opt-in (not in the default set): hover-graph hover-json hover-json-sel hover-tabview hover-find hover-settings,
# and Tab-key scans tab-graph graph-enter-group graph-enter-json graph-menu-key graph-select-key note-key digit-jump json-tab-offscreen tabview-actions-click json-keys tab-tabview tab-settings tab-find tab-quickswitch tab-dropdown tab-popup tab-popup-history tab-popup-downloads tab-engine tab-combo, which
# sweep the mouse along the toolbar and report where a hover moves anything
# other than the hovered widget (see scripts/hover_diff.py).
# mouse-regress is asserted instead of shot: it prints PASS/FAIL for drag, swatch click and middle-click close.
# The app has no light theme, so there is one set of images.
# Build first: cargo build -p manifold -p manifold-view-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
WAIT_FILE_SECS=30 # chrome-shots' own wait, from before lib.sh
require_fresh_exe "$exe" || exit 1
# The inline python below reads EXPORTSTATE cards through scripts/export_state.py.
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"

OUT="${OUT:-$repo_root/target/shots}"
SIZE="${SIZE:-1280 800}"
CROP_PX="${CROP_PX:-110}"
# The "Recently Closed (n)" toolbar button, which appears once a card is closed.
CLOSED_BUTTON_X="${CLOSED_BUTTON_X:-515}"
# A JSON bar button with a tooltip (Copy value).
JSON_TIP_X="${JSON_TIP_X:-120}"
ALL="graph graph-hover graph-menu-tab graph-menu-group graph-menu-note graph-link graph-menu-links graph-link-hover graph-link-handles graph-link-drag graph-filter graph-group json-level json-menu tabview tabview-dropdown tabview-find tabview-actions tabview-split settings popup-bookmarks popup-history popup-downloads shortcuts quickswitch graph-closed graph-newnode-menu newnode-zoom graph-tooltip image-view cards-f note-edit missing-file download-real download-progress download-failed json-zoom settings-engine settings-engine-tab settings-combo toast tabview-engine json-tooltip engine-states-picker engine-states-settings engine-install-confirm"
VIEWS="${VIEWS:-$ALL}"
mkdir -p "$OUT/crop"
work="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    rm -rf "$work"
}
trap cleanup EXIT

w() { cygpath -w "$1"; }
profile="$work/appdata"
seed_settings "$profile"
# Rows for the Bookmarks and History popups.
echo '[{"title": "Example Domain", "url": "https://example.com/", "folder": null}, {"title": "Rust docs", "url": "https://doc.rust-lang.org/", "folder": "Reading"}, {"title": "A rather long page title that has to be truncated by the popup", "url": "https://example.org/long", "folder": "Reading"}]' >"$profile/manifold/bookmarks.json"
echo '[{"title": "Example Domain", "url": "https://example.com/"}, {"title": "Wikipedia", "url": "https://en.wikipedia.org/"}, {"title": "Hacker News", "url": "https://news.ycombinator.com/"}]' >"$profile/manifold/history.json"
cat >"$work/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]},
 {"id": "g", "type": "application/vnd.manifold.group", "name": "Research", "pos": [300.0, 0.0]},
 {"id": "t2", "type": "application/vnd.manifold.tab", "url": "about:blank", "view_kind": "webview2", "group": "g", "pos": [0.0, 0.0]},
 {"id": "n1", "type": "text/markdown", "text": "a note", "pos": [-300.0, 0.0]}
]}
EOF
"$exe" --import-json "$(w "$work/seed.json")" --out "$(w "$profile/manifold/session.mnf")" >/dev/null
# An image and a JSON file, added by ADDFILE once the app is up.
python -P -c '
import json, sys
from PIL import Image
Image.new("RGB", (240, 160), (70, 110, 160)).save(sys.argv[1])
json.dump({"name": "demo", "items": [1, 2, 3], "nested": {"a": True, "b": None}}, open(sys.argv[2], "w"))
' "$(w "$work/pic.png")" "$(w "$work/data.json")" </dev/null 2>/dev/null || {
    python -P -c 'import json,sys; json.dump({"name": "demo", "items": [1, 2, 3], "nested": {"a": True, "b": None}}, open(sys.argv[1], "w"))' "$(w "$work/data.json")" </dev/null
}

cmd="$work/cmd"
: >"$cmd"
launch_headless "$exe" "$profile" "$cmd" "$work/log"
inst=$!

# The state file, fresh.
state() {
    rm -f "$work/state.json"
    send "EXPORTSTATE $(w "$work/state.json")"
    wait_file "$work/state.json"
}
# "x y" of the first card of type $1 (group $2: "top" or "any"; $3 "loaded" or
# "asleep" narrows a tab by its engine state): the centre of the part of its
# rect inside the window, since a card can hang off the edge. Without the
# python's "\r", so the y can go into shell arithmetic.
center() {
    state
    python -P -c '
import json, sys
from export_state import card_is
d = json.load(open(sys.argv[1], encoding="utf-8"))
ty, where, W, H = sys.argv[2], sys.argv[3], float(sys.argv[4]), float(sys.argv[5])
loaded = {"loaded": True, "asleep": False, "": None}[sys.argv[6]]
for c in d["cards"]:
    if card_is(c, ty, loaded) and (where in ("any", "chip") or c["group"] is None):
        x0, y0, x1, y1 = c["rect"]
        if where == "chip":
            # The jar chip hangs a fixed number of screen points above the card'"'"'s bottom-left corner.
            print(int(x0 + 18), int(y1 - 33))
        else:
            print(int((max(x0, 0) + min(x1, W)) / 2), int((max(y0, 36) + min(y1, H)) / 2))
        break
' "$(w "$work/state.json")" "$1" "${2:-top}" "${SIZE% *}" "${SIZE#* }" "${3:-}" </dev/null | tr -d '\r'
}
TAB=application/vnd.manifold.tab GROUP=application/vnd.manifold.group NOTE=text/markdown
IMAGE=image/png JSON=application/json
field() { python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get(sys.argv[2]))' "$(w "$work/state.json")" "$1" </dev/null; }
# Waits until two exports agree on the cards: SLEEP draws no frames, and the
# camera eases after entering a level, so a fixed sleep can shoot mid-ease.
settle() {
    local prev="" cur
    for _ in $(seq 20); do
        state
        cur="$(python -P -c 'import json,sys; print([(c["id"], c["rect"]) for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]])' "$(w "$work/state.json")" </dev/null)"
        [ "$cur" = "$prev" ] && return 0
        prev="$cur"
    done
}
shot() {
    settle
    local path="$OUT/$1.png"
    rm -f "$path"
    send "SCREENSHOT $(w "$path")"
    wait_file "$path" && echo "shot: $1"
}
# Back to Graph view at the top level, nothing open. Escape closes popups
# and leaves Tab view; the toolbar's back button (top left) leaves a group
# or a JSON level.
reset() {
    for _ in 1 2 3 4 5; do
        send ESCAPE
        settle
        if [ "$(field tab_view)" = False ]; then
            [ "$(field current_group)" = None ] && [ "$(field json_level)" = None ] && return 0
            send "CLICK 14 16"
        fi
    done
    echo "reset: not back at the top level" >&2
}
# hover_scan NAME: with the app in the view NAME, shoot it at rest and with the
# mouse at every HOVER_STEP px along the top bar, into $OUT/hover/NAME/.
hover_scan() {
    local d="$OUT/hover/$1" x
    rm -rf "$d"; mkdir -p "$d"
    send "MOUSEMOVE 5 790" "SLEEP 1"; settle
    send "SCREENSHOT $(w "$d/rest.png")"; wait_file "$d/rest.png"
    for x in $(seq 4 "${HOVER_STEP:-14}" "${SIZE% *}"); do
        send "MOUSEMOVE $x ${HOVER_Y:-16}" "SLEEP 0.3"
        send "SCREENSHOT $(w "$d/x$(printf %04d "$x").png")"; wait_file "$d/x$(printf %04d "$x").png"
    done
    send "MOUSEMOVE 5 790"
    python -P "$repo_root/scripts/hover_diff.py" "$d" "${HOVER_BAND:-34}" </dev/null
}
# hover_column NAME X0 [Y0 Y1]: the same down the column at X0 (the settings page's widgets).
hover_column() {
    local d="$OUT/hover/$1" y
    rm -rf "$d"; mkdir -p "$d"
    send "MOUSEMOVE 1275 795" "SLEEP 1"; settle
    send "SCREENSHOT $(w "$d/rest.png")"; wait_file "$d/rest.png"
    for y in $(seq "${3:-44}" "${HOVER_STEP:-14}" "${4:-960}"); do
        send "MOUSEMOVE $2 $y" "SLEEP 0.3"
        send "SCREENSHOT $(w "$d/y$(printf %04d "$y").png")"; wait_file "$d/y$(printf %04d "$y").png"
    done
    python -P "$repo_root/scripts/hover_diff.py" "$d" "${HOVER_BAND:-520}" y "${HOVER_X0:-0}" </dev/null
}
# tab_scan NAME N: rest, then N Tab key presses (egui keyboard focus moving on), a shot after each.
tab_scan() {
    local d="$OUT/hover/$1" i
    rm -rf "$d"; mkdir -p "$d"
    send "MOUSEMOVE 1275 795" "SLEEP 1"; settle
    send "SCREENSHOT $(w "$d/rest.png")"; wait_file "$d/rest.png"
    for i in $(seq "$2"); do
        send TABKEY "SLEEP 0.5"
        send "SCREENSHOT $(w "$d/x$(printf %04d "$i").png")"; wait_file "$d/x$(printf %04d "$i").png"
    done
    python -P "$repo_root/scripts/hover_diff.py" "$d" "${HOVER_BAND:-34}" </dev/null
}
# focus_scan NAME X Y...: rest, then a click at each X Y (focusing whatever is
# there), into $OUT/hover/NAME/ as x<N>.png; Escape between. Same report as hover_scan.
focus_scan() {
    local d="$OUT/hover/$1" i=0; shift
    rm -rf "$d"; mkdir -p "$d"
    send "MOUSEMOVE 5 790" "SLEEP 1"; settle
    send "SCREENSHOT $(w "$d/rest.png")"; wait_file "$d/rest.png"
    while [ $# -ge 2 ]; do
        i=$((i + 1)); send "CLICK $1 $2" "SLEEP 0.5" "MOUSEMOVE 5 790" "SLEEP 0.5"
        send "SCREENSHOT $(w "$d/x$(printf %04d "$i").png")"; wait_file "$d/x$(printf %04d "$i").png"
        send ESCAPE "SLEEP 0.5"; shift 2
    done
    python -P "$repo_root/scripts/hover_diff.py" "$d" "${HOVER_BAND:-34}" </dev/null
}
has() { case " $VIEWS " in *" $1 "*) return 0 ;; esac; return 1; }

send "RESIZE $SIZE" WAITTABS "SLEEP 3"
if [ -s "$work/pic.png" ]; then send "ADDFILE $(w "$work/pic.png") 160 640"; fi
send "ADDFILE $(w "$work/data.json") 640 700" "SLEEP 2"

if has graph; then shot graph; fi
if has graph-hover; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "SLEEP 1"; shot graph-hover; send "MOUSEMOVE 5 790"
fi
for v in "tab $TAB loaded" "group $GROUP" "note $NOTE"; do
    set -- $v
    name="graph-menu-$1"
    if has "$name"; then
        read -r x y < <(center "$2" top "${3:-}"); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1"; shot "$name"; reset
    fi
done
# The Jar page: two named jars exist and the focused tab uses the second, so it lists three rows (Default and
# both jars) with the current one selected.
if has graph-menu-jars; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" ESCAPE "SETJAR Alpha" "SETJAR Beta" "SLEEP 1"         "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" "MENUPAGE jar" "SLEEP 1"; shot graph-menu-jars; reset
fi
# A right-click on the jar chip: the menu opens straight on the Jar page, with no MENUPAGE.
if has graph-menu-chip; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" ESCAPE "SETJAR Alpha" "SETJAR Beta" "SLEEP 1"
    read -r x y < <(center "$TAB" chip loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1"; shot graph-menu-chip; reset
fi
# The same menu's Color page.
if has graph-menu-color; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" "MENUPAGE color" "SLEEP 1"; shot graph-menu-color; reset
fi
# The group's Default jar page with those jars: none chosen (so Default is the selected row).
if has graph-menu-group-jars; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" ESCAPE "SETJAR Alpha" "SETJAR Beta" "SLEEP 1"
    read -r x y < <(center "$GROUP" top); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1" "MENUPAGE jar" "SLEEP 1"; shot graph-menu-group-jars; reset
fi
# The id of the first top-level card of type $1 (import gives the seeded nodes fresh ids).
cardid() {
    state
    python -P -c 'import json,sys; from export_state import card_is; print([c["id"] for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if card_is(c, sys.argv[2]) and c["group"] is None][0])' "$(w "$work/state.json")" "$1" </dev/null | tr -d '\r'
}
# Links: the top-level tab linked to the note (a dashed line between the cards), the line hovered, then the tab's
# menu with its Links section (the verb rows and "Disconnect from" the note). UNLINK after, so later views have no link.
if has graph-link || has graph-link-hover || has graph-menu-links; then
    tab_id=$(cardid "$TAB"); note_id=$(cardid "$NOTE")
    send "LINK $tab_id $note_id" "SLEEP 1"
    if has graph-link; then shot graph-link; fi
    # The pointer on the link line: the line drawn thicker (the cursor, a pointing hand, is not in a screenshot).
    if has graph-link-hover; then
        state
        read -r lx ly < <(python -P -c 'import json,sys; e=[e for e in json.load(open(sys.argv[1], encoding="utf-8"))["edges"] if e["kind"]=="link"][0]["screen"]; print(round(e[0]), round(e[1]))' "$(w "$work/state.json")" </dev/null | tr -d '\r')
        send "MOUSEMOVE $lx $ly" "SLEEP 1"; shot graph-link-hover; reset
    fi
    if has graph-menu-links; then
        read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1"; shot graph-menu-links; reset
    fi
    send "UNLINK $tab_id $note_id" "SLEEP 1"
fi
# Connect-drag: the top-level tab hovered shows its four edge handles; then a drag from its left handle 4/5 of the
# way to the note's centre (the rubber band, and the note's target ring bright under the pointer), shot mid-drag.
# Esc cancels it before the button comes up, so nothing is linked.
if has graph-link-handles || has graph-link-drag; then
    tab_id=$(cardid "$TAB")
    read -r tx ty < <(center "$TAB" top loaded)
    send "MOUSEMOVE $tx $ty" "SLEEP 1"
    state
    read -r hx hy < <(python -P -c 'import json,sys; r=json.load(open(sys.argv[1], encoding="utf-8"))["link_handles"][sys.argv[2]]["left"]; print(round((r[0]+r[2])/2), round((r[1]+r[3])/2))' "$(w "$work/state.json")" "$tab_id" </dev/null | tr -d '\r')
    if has graph-link-handles; then send "MOUSEMOVE $hx $hy" "SLEEP 1"; shot graph-link-handles; fi
    if has graph-link-drag; then
        read -r nx ny < <(center "$NOTE" top)
        send "MOUSEMOVE $tx $ty" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
        for i in 1 2 3 4 5 6 7 8; do send "MOUSEMOVE $((hx + (nx - hx) * i * 4 / 40)) $((hy + (ny - hy) * i * 4 / 40))"; done
        send "SLEEP 1"; shot graph-link-drag
        send ESCAPE "MOUSEUP $((hx + (nx - hx) * 4 / 5)) $((hy + (ny - hy) * 4 / 5))" "SLEEP 1"
    fi
    reset
fi
if has hover-graph; then hover_scan graph; fi
if has tab-graph; then tab_scan graph "${TAB_STOPS:-10}"; fi
# A click focuses the group card without entering it; Enter then does what a double-click does.
if has graph-enter-group; then
    for _ in $(seq "${GROUP_STOP:-12}"); do send TABKEY "SLEEP 0.5"; done; send "SLEEP 1"; shot graph-enter-group-click; send ENTER "SLEEP 0.4"; shot graph-enter-group-early; send "SLEEP 3"; shot graph-enter-group; reset
fi
# Tab to the JSON card and Enter: the level opens with its root card focused.
if has graph-enter-json; then
    for _ in $(seq "${JSON_ENTRY_STOP:-16}"); do send TABKEY "SLEEP 0.5"; done; send ENTER "SLEEP 3"; shot graph-enter-json; reset
fi
if has graph-menu-key; then
    for _ in $(seq "${MENU_STOP:-13}"); do send TABKEY "SLEEP 0.2"; done
    send SHIFTF10 "SLEEP 1"; shot graph-menu-key-open
    send TABKEY "SLEEP 0.5"; shot graph-menu-key-tab
    send ESCAPE "SLEEP 1"; shot graph-menu-key-closed; reset
fi
# Space on a Tab-focused tab card selects it (and Space again clears it).
if has graph-select-key; then
    for _ in $(seq "${SELECT_STOP:-12}"); do send TABKEY "SLEEP 0.5"; done
    send SPACEKEY "SLEEP 1"; shot graph-select-key-on
    send SPACEKEY "SLEEP 1"; shot graph-select-key-off; reset
fi
# Tab to the Note card and Enter: the note starts editing; Escape ends it and Enter edits again (focus
# returned to the card). The note's Tab stop is found by trying each stop. Prints PASS/FAIL lines.
if has note-key; then
    noteediting() { state; python -P -c 'import json,sys; print(any(c.get("editing") for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]))' "$(w "$work/state.json")" </dev/null | tr -d '
'; }
    found=0
    for stop in $(seq 20); do
        reset
        for _ in $(seq "$stop"); do send TABKEY "SLEEP 0.2"; done
        send ENTER "SLEEP 1"
        if [ "$(noteediting)" = True ]; then found=$stop; break; fi
    done
    if [ "$found" = 0 ]; then echo "FAIL: Enter on a focused Note card starts editing (no Tab stop did)"; else
        echo "PASS: Enter on the Note card (Tab stop $found) starts editing"; shot note-key-editing
        send ESCAPE "SLEEP 1"
        [ "$(noteediting)" = False ] && echo "PASS: Escape ends editing" || echo "FAIL: Escape ends editing"
        send ENTER "SLEEP 1"
        [ "$(noteediting)" = True ] && echo "PASS: Enter edits again, so focus returned to the card" || echo "FAIL: focus did not return to the card"
    fi
    reset
fi
# Enter the JSON level by keyboard in a small window (so some cards are off screen) and Tab through every
# card after the root: each Tab must select a new card, off-screen ones included (zoomed in so they are). Prints PASS/FAIL lines.
if has json-tab-offscreen; then
    sel() { state; python -P -c 'import json,sys; j=json.load(open(sys.argv[1], encoding="utf-8"))["json_level"]; print("%s|%d" % (j["selected"], len(j["cards"])))' "$(w "$work/state.json")" </dev/null | tr -d '
'; }
    for _ in $(seq "${JSON_ENTRY_STOP:-16}"); do send TABKEY "SLEEP 0.3"; done; send ENTER "SLEEP 3"
    # Zoomed in, the cards other than the root fall off screen.
    send ZOOMIN ZOOMIN ZOOMIN "MOUSEMOVE 700 650" "MOUSEDOWN 700 650" "MOUSEMOVE 400 650" "MOUSEMOVE 100 650" "MOUSEUP 100 650" "MOUSEMOVE 100 650" "MOUSEMOVE 5 790" "SLEEP 2"; settle; shot json-tab-offscreen-before
    total=$(sel); total=${total#*|}
    seen=""
    for _ in $(seq $((total - 1))); do
        send TABKEY "SLEEP 1.5"; settle
        cur=$(sel); seen="$seen [${cur%|*}]"
    done
    echo "selected after each Tab:$seen (cards: $total)"
    distinct=$(printf '%s
' $seen | sort -u | wc -l)
    [ "$distinct" -ge $((total - 1)) ] && echo "PASS: Tab reached every JSON card after the root" || echo "FAIL: Tab reached $distinct of $((total - 1)) cards after the root"
    shot json-tab-offscreen; reset
fi
# Digits jump to the card with that Tab-order number (the Note is 3 of the 5 seeded cards):
# 3 focuses it, so Enter starts editing. Prints PASS/FAIL lines.
if has digit-jump; then
    ned() { state; python -P -c 'import json,sys; print(any(c.get("editing") for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]))' "$(w "$work/state.json")" </dev/null | tr -d '
'; }
    reset
    send "DIGITKEY 3" "SLEEP 1"; shot digit-jump
    send ENTER "SLEEP 1"
    echo "$([ "$(ned)" = True ] && echo PASS || echo FAIL): digit 3 focuses card 3 (the note), Enter edits it"
    reset
fi
# Mouse behaviour that the Tab-focus model must not disturb, asserted rather than shot: dragging a
# card moves it, clicking its swatch tags it, middle-click closes it. Prints PASS/FAIL lines.
if has mouse-regress; then
    cardrect() {
        state
        python -P -c '
import json, sys
from export_state import card_is
for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]:
    if card_is(c, sys.argv[2]):
        print(*[int(v) for v in c["rect"]]); break
' "$(w "$work/state.json")" "$1" </dev/null | tr -d '\r'
    }
    cardcount() { state; python -P -c 'import json,sys; print(len(json.load(open(sys.argv[1], encoding="utf-8"))["cards"]))' "$(w "$work/state.json")" </dev/null | tr -d '\r'; }
    read -r x0 y0 x1 y1 < <(cardrect "$NOTE")
    cx=$(((x0 + x1) / 2)); cy=$(((y0 + y1) / 2))
    send "MOUSEMOVE $cx $cy" "MOUSEDOWN $cx $cy" "MOUSEMOVE $((cx + 30)) $((cy + 15))" "MOUSEMOVE $((cx + 60)) $((cy + 30))" "MOUSEMOVE $((cx + 90)) $((cy + 45))" "MOUSEUP $((cx + 90)) $((cy + 45))" "SLEEP 1"
    read -r n0 m0 n1 m1 < <(cardrect "$NOTE")
    check "drag moves the card by the drag distance" "90,45" "$((n0 - x0)),$((m0 - y0))"
    # The swatch sits on the bottom-right corner (hover reveals nothing extra; the click is what counts).
    sx=$n1; sy=$m1
    send "MOUSEMOVE $((n0 + 20)) $((m0 + 20))" "SLEEP 0.5" "CLICK $sx $sy" "SLEEP 1"; shot mouse-regress-swatch
    state
    tagged=$(python -P -c 'import json,sys; from export_state import *; print([c["color"] for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if card_is(c, NOTE)][0])' "$(w "$work/state.json")" </dev/null)
    check "swatch click tags the card" "True" "$tagged"
    before=$(cardcount)
    mx=$(((n0 + n1) / 2)); my=$(((m0 + m1) / 2))
    send "MOUSEMOVE $mx $my" "MOUSEDOWN $mx $my middle" "MOUSEUP $mx $my middle" "SLEEP 1"
    check "middle-click closes the card" "$((before - 1))" "$(cardcount)"
    reset
fi
if has hover-menu-tab; then
    read -r x y < <(center "$TAB" top loaded); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1"
    HOVER_X0="${MENU_X0:-640}" HOVER_BAND=190 hover_column menu-tab "${MENU_X:-700}" "${MENU_Y0:-424}" "${MENU_Y1:-646}"; reset
fi
if has focus-graph; then focus_scan graph 385 16 235 16; fi
# Right-click on empty canvas: the New Tab/Note/Group card.
if has graph-newnode-menu; then
    send "MOUSEMOVE 700 150" "MOUSEDOWN 700 150 right" "MOUSEUP 700 150 right" "SLEEP 1"; shot graph-newnode-menu; reset
fi
# The same card zoomed far out: its rows keep a constant screen size (they once scaled with zoom and overflowed).
if has newnode-zoom; then
    send "CLICK 219 19" "SLEEP 1" "CLICK 219 19" "SLEEP 1" "CLICK 219 19" "SLEEP 1" "CLICK 219 19" "SLEEP 1" "CLICK 219 19" "SLEEP 1" "CLICK 219 19" "SLEEP 1" "MOUSEMOVE 700 150" "MOUSEDOWN 700 150 right" "MOUSEUP 700 150 right" "SLEEP 1"; shot newnode-zoom; reset
fi
# A toolbar tooltip (the shortcuts help button).
if has graph-tooltip; then
    send "MOUSEMOVE 467 16" "SLEEP 2"; shot graph-tooltip; send "MOUSEMOVE 5 790"
fi
# A download card and a sleeping tab beside the live cards (header F on cards with no seeded kind of their own).
if has cards-f; then
    send "ADDDOWNLOAD $(w "$work/data.json")" "SLEEPNODE t1" "SLEEP 6" "MOUSEMOVE 327 19" "MOUSEDOWN 327 19 left" "MOUSEUP 327 19 left" "SLEEP 2"; shot cards-f
fi
# A note being edited: a click on the rendered note enters the editor in place.
if has note-edit; then
    read -r x y < <(center "$NOTE"); send "CLICK $x $y" "SLEEP 1" "MOUSEMOVE 5 790" "SLEEP 1"; shot note-edit; send ESCAPE "SLEEP 1"
fi
if has image-view; then
    read -r x y < <(center "$IMAGE"); send "DOUBLECLICK $x $y" "SLEEP 3"; shot image-view; reset
fi
if has graph-filter; then send "GRAPHFILTERTEXT res" "SLEEP 1"; shot graph-filter; send "GRAPHFILTERTEXT "; fi
if has graph-group; then
    read -r x y < <(center "$GROUP"); send "DOUBLECLICK $x $y" "SLEEP 3"; shot graph-group; reset
fi
if has json-level || has json-menu || has json-tooltip || has json-keys || has hover-json || has hover-json-sel; then
    read -r x y < <(center "$JSON"); send "DOUBLECLICK $x $y" "SLEEP 3"
    has json-level && shot json-level
    has hover-json && hover_scan json
    # With a card selected, the bar's Copy buttons are enabled.
    if has hover-json-sel; then send "CLICK 280 380" "SLEEP 1"; hover_scan json-sel; fi
    if has json-tooltip; then send "MOUSEMOVE $JSON_TIP_X 16" "SLEEP 2"; shot json-tooltip; send "MOUSEMOVE 5 790"; fi
    # Right-click the first card the level lays out (data.json at 1280x800).
    # Keys: Down x3 selects the third outline row, Enter toggles it, Right opens it, Up+Left+Left closes then selects the parent, Tab focuses a card.
    if has json-keys; then
        send ARROWDOWN ARROWDOWN ARROWDOWN "SLEEP 1"; shot json-keys-down
        send ENTER "SLEEP 1"; shot json-keys-enter
        send ARROWRIGHT "SLEEP 1"; shot json-keys-right
        send ARROWUP ARROWLEFT ARROWLEFT "SLEEP 1"; shot json-keys-left
        tab_scan json "${JSON_TAB_STOPS:-18}"
    fi
    if has json-menu; then settle; send "MOUSEMOVE 280 380" "MOUSEDOWN 280 380 right" "MOUSEUP 280 380 right" "SLEEP 1"; shot json-menu; send ESCAPE; fi
    reset
fi
if has tabview || has hover-tabview || has tab-tabview || has tabview-dropdown || has tabview-find || has tabview-actions || has tabview-actions-click || has tabview-split || has tabview-engine || has hover-find || has hover-dropdown || has tab-find || has tab-dropdown || has tabview-split-key; then
    read -r x y < <(center "$TAB" top loaded); send "DOUBLECLICK $x $y" "SLEEP 5"
    has tabview && shot tabview
    has tab-tabview && tab_scan tabview 12
    has hover-tabview && hover_scan tabview
    if has tabview-dropdown; then send "ADDRESSBAR exam" "SLEEP 2"; shot tabview-dropdown; send ESCAPE "SLEEP 1"; fi
    if has tab-dropdown; then
        send "ADDRESSBAR exam" "SLEEP 2"; HOVER_BAND=820 tab_scan dropdown 4; send ESCAPE "SLEEP 1"
    fi
    if has hover-dropdown; then
        send "ADDRESSBAR exam" "SLEEP 2"; HOVER_X0=210 HOVER_BAND=820 hover_column dropdown 400 40 250; send ESCAPE "SLEEP 1"
    fi
    if has tabview-find; then send OPENFINDBAR "FINDTEXT abc" "SLEEP 1"; shot tabview-find; send CLOSEFINDBAR; fi
    # Asserted: the popup's Pin row pins the focused tab, its Duplicate row adds a card, its Close Tab row removes one.
    if has tabview-actions-click; then
        npinned() { state; python -P -c 'import json,sys; print(sum(1 for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if c["pinned"]))' "$(w "$work/state.json")" </dev/null | tr -d '
'; }
        ncards() { state; python -P -c 'import json,sys; print(len(json.load(open(sys.argv[1], encoding="utf-8"))["cards"]))' "$(w "$work/state.json")" </dev/null | tr -d '
'; }
        pins0=$(npinned); cards0=$(ncards)
        send "CLICK $((${SIZE% *} - 95)) 16" "SLEEP 1" "CLICK $((${SIZE% *} - 95)) 49" "SLEEP 1"
        echo "$([ "$(npinned)" = "$((pins0 + 1))" ] && echo PASS || echo FAIL): Tab actions > Pin pins the tab"
        send "CLICK $((${SIZE% *} - 95)) 16" "SLEEP 1" "CLICK $((${SIZE% *} - 95)) 76" "SLEEP 2"
        echo "$([ "$(ncards)" = "$((cards0 + 1))" ] && echo PASS || echo FAIL): Tab actions > Duplicate adds a card"
        send "CLICK $((${SIZE% *} - 95)) 16" "SLEEP 1" "CLICK $((${SIZE% *} - 95)) 155" "SLEEP 2"
        echo "$([ "$(ncards)" = "$cards0" ] && echo PASS || echo "FAIL ($(ncards) vs $cards0)"): Tab actions > Close Tab removes the tab"
    fi
    if has tabview-actions; then send "CLICK $((${SIZE% *} - 95)) 16" "SLEEP 1"; shot tabview-actions; send ESCAPE; fi
    if has hover-find; then
        send OPENFINDBAR "FINDTEXT abc" "SLEEP 1"; HOVER_Y=${FIND_Y:-60} HOVER_BAND=${FIND_BAND:-90} hover_scan find; send CLOSEFINDBAR
    fi
    if has tab-find; then
        send OPENFINDBAR "FINDTEXT abc" "SLEEP 1"; HOVER_BAND=${FIND_BAND:-90} tab_scan find 6; send CLOSEFINDBAR
    fi
    # The Split button opens the engine picker.
    if has tabview-engine; then send "CLICK $((${SIZE% *} - 126)) 16" "SLEEP 1"; shot tabview-engine; send ESCAPE "SLEEP 1"; fi
    # Keyboard-only pick from the Split engine picker: it opens on the selected card,
    # Down moves, Enter picks and opens the Split pane.
    if has tabview-split-key; then
        send "CLICK $((${SIZE% *} - 126)) 16" "SLEEP 1"; shot tabview-split-key-open
        send ARROWDOWN "SLEEP 1"; shot tabview-split-key-down
        send ENTER "SLEEP 3"; shot tabview-split-key; send CLOSESPLIT "SLEEP 1"
    fi
    if has tabview-split; then send "SPLIT webview2" "SLEEP 3"; shot tabview-split; send CLOSESPLIT "SLEEP 1"; fi
    reset
fi
if has tab-quickswitch; then
    send QUICKSWITCHOPEN "SLEEP 1"; HOVER_BAND=400 tab_scan quickswitch 4; send QUICKSWITCHESCAPE "SLEEP 1"
fi
if has hover-quickswitch; then
    send QUICKSWITCHOPEN "SLEEP 1"; HOVER_X0=440 HOVER_BAND=400 hover_column quickswitch 640 90 230; send QUICKSWITCHESCAPE "SLEEP 1"
fi
if has shortcuts; then send TOGGLESHORTCUTSHELP "SLEEP 1"; shot shortcuts; send TOGGLESHORTCUTSHELP "SLEEP 1"; fi
if has quickswitch; then send QUICKSWITCHOPEN "SLEEP 1"; shot quickswitch; send QUICKSWITCHESCAPE "SLEEP 1"; fi
# Closes the note (middle-click), so it must come before the settings block
# and after every view that shows the note.
if has graph-closed; then
    read -r x y < <(center "$NOTE"); send "CLICK $x $y middle" "SLEEP 1"; settle
    send "CLICK $CLOSED_BUTTON_X 16" "SLEEP 1"; shot graph-closed; reset
fi
# Last: the download it adds would show in the other views.
if has settings || has hover-settings || has tab-settings || has settings-engine || has settings-engine-tab || has settings-combo || has popup-bookmarks || has popup-history || has popup-downloads || has tab-popup || has tab-popup-history || has tab-popup-downloads || has tab-combo || has tab-engine; then
    # Taller, so a popup opening below the Lists buttons is not pushed up over them.
    send "RESIZE ${SIZE% *} 1000" TOGGLESETTINGSPAGE "SLEEP 2"
    has popup-downloads && send "ADDDOWNLOAD $(w "$work/data.json")" "SLEEP 1"
    send "SESSIONNAMETEXT demo" "SAVESESSIONAS demo" "SLEEP 1"
    has settings && shot settings
    if has tab-settings; then HOVER_BAND=1000 tab_scan settings 24; fi
    if has hover-settings; then hover_column settings 40; fi
    # Engine picker (Default starting engine) and the search-engine combo.
    if has settings-engine; then send "CLICK 63 180" "SLEEP 1"; shot settings-engine; send "CLICK 1100 300" "SLEEP 1"; fi
    if has settings-engine-tab; then send "CLICK 119 513" "SLEEP 1"; shot settings-engine-tab; send "CLICK 1100 300" "SLEEP 1"; fi
    if has settings-combo; then send "CLICK 80 87" "SLEEP 1"; shot settings-combo; send "CLICK 1100 300" "SLEEP 1"; fi
    if has tab-engine; then send "CLICK 63 180" "SLEEP 1"; HOVER_BAND=1000 tab_scan engine 5; send "CLICK 1100 300" "SLEEP 1"; fi
    if has tab-combo; then send "CLICK 80 87" "SLEEP 1"; HOVER_BAND=1000 tab_scan combo 5; send "CLICK 1100 300" "SLEEP 1"; fi
    # The Lists buttons on the settings page (fixed layout at the default size).
    for v in "popup-bookmarks 75" "popup-history 190" "popup-downloads 310"; do
        set -- $v
        if has "$1"; then send "CLICK $2 677" "SLEEP 1"; shot "$1"; send "CLICK 1100 300" "SLEEP 1"; fi
    done
    for v in "tab-popup 75" "tab-popup-history 190" "tab-popup-downloads 310"; do
        set -- $v
        if has "$1"; then send "CLICK $2 677" "SLEEP 1"; HOVER_BAND=1000 tab_scan "${1#tab-}" 6; send "CLICK 1100 300" "SLEEP 1"; fi
    done
    send TOGGLESETTINGSPAGE "RESIZE $SIZE" "SLEEP 1"
fi
# A toast, from opening a session that doesn't exist (which also leaves the settings page).
if has toast; then send "OPENSESSION nosuch" "SLEEP 1"; shot toast; fi
# Restored cards whose files are gone (an image and a JSON file): save a session with them, delete the files,
# open it again. Before json-zoom, which must run last.
if has missing-file; then
    cp "$work/pic.png" "$work/gone.png"; printf x >> "$work/gone.png"; cp "$work/data.json" "$work/gone.json"; printf " " >> "$work/gone.json"
    send "ADDFILE $(w "$work/gone.png") 100 300" "ADDFILE $(w "$work/gone.json") 400 300" "SLEEP 2" "EXPORTSESSION $(w "$work/missing.mnf")" "SLEEP 1"; state
    rm -f "$work/gone.png" "$work/gone.json"
    send "IMPORTSESSION $(w "$work/missing.mnf")" "SLEEP 1" CONFIRMIMPORT "SLEEP 4" "MOUSEMOVE 5 790" "SLEEP 1"; shot missing-file
fi
# Real downloads (`download-failed` kills the server mid-transfer): a local HTTP server answers /real.zip as a small attachment and /slow as a 1 MB one it stalls after
# 300 KB, and the live tab navigates to it, so the engine starts the download (the card keeps the URL as its source
# line). Before json-zoom, which must run last.
if has download-real || has download-progress || has download-failed; then
    cat >"$work/serve.py" <<'PY'
import http.server, time
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        slow = self.path == "/slow"
        self.send_response(200)
        self.send_header("Content-Type", "application/zip")
        self.send_header("Content-Disposition", 'attachment; filename="%s.zip"' % ("slow" if slow else "real"))
        self.send_header("Content-Length", "1000000" if slow else "4")
        self.end_headers()
        if slow:
            self.wfile.write(b"x" * 300000)
            self.wfile.flush()
            time.sleep(120)
        else:
            self.wfile.write(b"data")
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", 8765), H).serve_forever()
PY
    python -P "$work/serve.py" </dev/null >/dev/null 2>&1 &
    srv=$!
    read -r x y < <(center "$TAB" top loaded); send "DOUBLECLICK $x $y" "SLEEP 5"
    if has download-progress || has download-failed; then
        # Still arriving: 300 KB of 1 MB. The fit button frames the new card, which opens above the tab's.
        send "ADDRESSBAR http://127.0.0.1:8765/slow" "SLEEP 1" ENTER "SLEEP 6"
        send TOGGLEVIEW "SLEEP 2" "CLICK 327 19" "SLEEP 2" "MOUSEMOVE 5 790" "SLEEP 1"; has download-progress && shot download-progress
        if has download-failed; then
            # The server dies mid-transfer: the engine reports the download interrupted.
            kill $srv 2>/dev/null; sleep 5
            send "MOUSEMOVE 5 780" "SLEEP 1"; shot download-failed
        fi
        send "DOUBLECLICK $x $y" "SLEEP 2"
    fi
    if has download-real; then
        send "ADDRESSBAR http://127.0.0.1:8765/real.zip" "SLEEP 1" ENTER "SLEEP 6"; shot download-real-tab
        send TOGGLEVIEW "SLEEP 2" "MOUSEMOVE 5 790" "SLEEP 1"; shot download-real
    fi
    kill $srv 2>/dev/null
fi
# Last, because it leaves the graph zoomed (the toolbar's fit button does not restore it). The Json card preview's header stays 40 points while its rows get more or less of the card: the toolbar's
# zoom-out button twice (fewer rows fit under the header), then back in (the graph zoom, not the page zoom `ZOOMIN` sets).
if has json-zoom; then
    click() { send "MOUSEMOVE $1 $2" "MOUSEDOWN $1 $2 left" "MOUSEUP $1 $2 left" "SLEEP 1"; }
    click 219 19; click 219 19; send "MOUSEMOVE 5 790" "SLEEP 2"; shot json-zoom-out
    # Further out: the Note card under 80 points tall, so it falls back to the 18-point name strip.
    for _ in 1 2 3; do click 219 19; done; send "MOUSEMOVE 5 790" "SLEEP 2"; shot note-short
    for _ in 1 2 3; do click 295 19; done
    # In to ~280%, then drag the empty canvas up so the Json card (below the tab) is on screen.
    for _ in 1 2 3 4 5 6; do click 295 19; done
    send "MOUSEMOVE 900 760" "MOUSEDOWN 900 760 left" "MOUSEMOVE 900 660 left" "MOUSEMOVE 900 560 left" "MOUSEUP 900 560 left" "MOUSEMOVE 5 790" "SLEEP 2"; shot json-zoom-in
fi
send QUIT
wait "$inst" 2>/dev/null
inst=""

# Engine states (CEF install) need what the instance above can't give: a manifold.exe with no CEF host or
# `cef-runtime/` beside it (the dev tree has them, so CEF would always read Installed) and an install that changes
# state mid-run. CEF installs two parts (its host exe, marked by `cef.host`, and the runtime), so the block needs
# a build that pins a CEF host (crates/manifold/src/host_pin.rs, written by scripts/release.sh): without one CEF
# reads "Unavailable: this build has no Blink (CEF) download" and the block is skipped. So a second instance (fresh profile, a copy of the exe in $work) walks Not installed ->
# Failed -> Installing -> Installed -> Update needed with nothing mocked in the app: the runtime is files in
# the profile's data dir, and the download goes through HTTPS_PROXY (ureq honours it) to scripts/fake_proxy.py,
# which answers 502 (Failed) or opens the tunnel and goes silent (Installing at 0 bytes; the half-filled
# bar would need a real transfer, so it isn't shot). Custom is two more instances whose settings.json names
# a directory that matches the pin, or doesn't.
cef_pin_lines="$(python -P "$repo_root/scripts/host_pin.py" --list "$repo_root/crates/manifold/src/host_pin.rs" </dev/null | grep '^cef windows-x64 ')"
if { has engine-states-picker || has engine-states-settings || has engine-install-confirm; } && [ -z "$cef_pin_lines" ]; then
    echo "SKIP engine-states-*: host_pin.rs pins no CEF host for windows-x64 (cut a release table first)"
elif has engine-states-picker || has engine-states-settings || has engine-install-confirm; then
    eng="$work/eng"; mkdir -p "$eng"
    cp "$exe" "$eng/manifold.exe"; cp "$(dirname "$exe")/manifold-engine-webview2.exe" "$eng/"
    pin_version="$(grep -o 'version: "[^"]*"' "$repo_root/crates/manifold/src/cef_pin.rs" | head -1 | cut -d'"' -f2)"
    python -P "$repo_root/scripts/fake_proxy.py" "$(w "$work/proxy-port")" "$(w "$work/proxy-mode")" 600 </dev/null >/dev/null 2>&1 &
    proxy=$!
    echo refuse >"$work/proxy-mode"
    wait_file "$work/proxy-port"
    # engine_app NAME [SETTINGS-EXTRA]: restart the instance on a fresh profile "eng-NAME" (the settings
    # page open). `eng_data` is where the runtime lives: etcetera's data dir under this APPDATA.
    engine_app() {
        profile="$work/eng-$1"
        seed_settings "$profile" webview2 "${2:-}"
        eng_data="$profile/manifold/manifold/data/engine-hosts"; mkdir -p "$eng_data"
        cat >"$work/seed-eng.json" <<SEED
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]},
 {"id": "t3", "type": "application/vnd.manifold.tab", "url": "https://example.com/", "view_kind": "cef", "pos": [300.0, 0.0]}
]}
SEED
        "$eng/manifold.exe" --import-json "$(w "$work/seed-eng.json")" --out "$(w "$profile/manifold/session.mnf")" >/dev/null
        cmd="$work/cmd-$1"; : >"$cmd"
        HTTPS_PROXY="http://127.0.0.1:$(cat "$work/proxy-port")" launch_headless "$eng/manifold.exe" "$profile" "$cmd" "$work/log-eng-$1"
        inst=$!
        send "RESIZE ${SIZE% *} 1000" WAITTABS "SLEEP 3"
    }
    engine_stop() { send QUIT; wait "$inst" 2>/dev/null; inst=""; }
    # fake_host current|old: CEF's pinned host files (sparse, at their pinned sizes) and the `cef.host` marker that
    # lists their hashes (`old`: other hashes, so the host reads Update needed).
    fake_host() {
        : >"$eng_data/cef.host"
        while read -r _ _ file size sha _; do
            truncate -s "$size" "$eng_data/$file"
            [ "$1" = old ] && sha="$(printf '%064d' 0)"
            printf '%s %s
' "$sha" "$file" >>"$eng_data/cef.host"
        done <<<"$cef_pin_lines"
    }
    # fake_runtime MARKER: a runtime directory with a (sparse) 423 MB libcef.dll and MARKER in `ready`.
    fake_runtime() { mkdir -p "$eng_data/cef-runtime"; truncate -s 423M "$eng_data/cef-runtime/libcef.dll"; printf '%s' "$1" >"$eng_data/cef-runtime/ready"; }
    engine_app states
    # The seeded CEF tab can't start: its card says so (graph view, then its Tab view).
    shot engine-states-graph
    send GRAPHRIGHT GRAPHOPEN "SLEEP 3"; shot engine-states-failed-tab
    send TOGGLEVIEW "SLEEP 1"
    send TOGGLESETTINGSPAGE "SLEEP 2"
    shot engine-states-settings-notinstalled
    send "CLICK 63 180" "SLEEP 1"; shot engine-states-picker-notinstalled; send "CLICK 1100 300" "SLEEP 1"
    # The Engines section's Download button asks first, with the size.
    send "CLICK 57 672" "SLEEP 1"; shot engine-install-confirm
    # Confirm with the proxy refusing: the install fails and the row says why, with Retry.
    send "CLICK 476 529" "SLEEP 4"; shot engine-states-settings-failed
    send "CLICK 63 180" "SLEEP 1"; shot engine-states-picker-failed; send "CLICK 1100 300" "SLEEP 1"
    # Retry with the proxy silent: Installing, with Cancel.
    echo stall >"$work/proxy-mode"
    send "CLICK 57 672" "SLEEP 1" "CLICK 476 529" "SLEEP 2"; shot engine-states-settings-installing
    send "CLICK 63 180" "SLEEP 1"; shot engine-states-picker-installing; send "CLICK 1100 300" "SLEEP 1"
    engine_stop
    # Files on disk, judged by their marker: this pin's is Installed (size, Remove), another's is Update needed.
    # Fresh instances: a failed install outranks the disk in the state, so they can't follow the one above.
    engine_app installed; fake_host current; fake_runtime "$pin_version"; send TOGGLESETTINGSPAGE "SLEEP 2"; shot engine-states-settings-installed; engine_stop
    engine_app stale; fake_host old; fake_runtime "old-version"; send TOGGLESETTINGSPAGE "SLEEP 2"; shot engine-states-settings-stale
    send "CLICK 63 180" "SLEEP 1"; shot engine-states-picker-stale; send "CLICK 1100 300" "SLEEP 1"
    engine_stop
    # Bring-your-own folders: one whose archive.json names this pin (Custom), one that names another build.
    for variant in "custom $pin_version" "custom-bad 151.0.1+gabcdef+chromium-151.0.0.1"; do
        set -- $variant
        mkdir -p "$work/mycef-$1"; : >"$work/mycef-$1/libcef.dll"
        echo "{\"type\": \"minimal\", \"name\": \"cef_binary_$2_windows64_minimal.tar.bz2\"}" >"$work/mycef-$1/archive.json"
        engine_app "$1" ", \"cef_directory\": \"$(cygpath -m "$work/mycef-$1")\""
        fake_host current  # a bring-your-own runtime still needs the downloaded host
        send TOGGLESETTINGSPAGE "SLEEP 2"; shot "engine-states-settings-$1"
        send "CLICK 63 180" "SLEEP 1"; shot "engine-states-picker-$1"; send "CLICK 1100 300" "SLEEP 1"
        engine_stop
    done
    kill $proxy 2>/dev/null
fi

python -P "$repo_root/scripts/shots_sheet.py" "$OUT" "$CROP_PX" "$VIEWS" </dev/null 2>&1 | tail -2
