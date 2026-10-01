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
#   graph graph-hover graph-menu-tab graph-menu-group graph-menu-note
#   graph-filter graph-group json-level
#   tabview tabview-dropdown tabview-find tabview-actions
#   settings shortcuts quickswitch
# The app has no light theme, so there is one set of images.
# Build first: cargo build -p manifold -p manifold-engine-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1

OUT="${OUT:-$repo_root/target/shots}"
SIZE="${SIZE:-1280 800}"
CROP_PX="${CROP_PX:-110}"
ALL="graph graph-hover graph-menu-tab graph-menu-group graph-menu-note graph-filter graph-group json-level tabview tabview-dropdown tabview-find tabview-actions settings shortcuts quickswitch"
VIEWS="${VIEWS:-$ALL}"
mkdir -p "$OUT/crop"
work="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
    rm -rf "$work"
}
trap cleanup EXIT

w() { cygpath -w "$1"; }
profile="$work/appdata"
mkdir -p "$profile/manifold"
echo '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' >"$profile/manifold/settings.json"
cat >"$work/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "kind": "tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]},
 {"id": "g", "kind": "group", "name": "Research", "pos": [300.0, 0.0]},
 {"id": "t2", "kind": "tab", "url": "about:blank", "engine": "webview2", "group": "g", "pos": [0.0, 0.0]},
 {"id": "n1", "kind": "note", "text": "a note", "pos": [-300.0, 0.0]}
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
APPDATA="$(w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$work/log" 2>&1 &
inst=$!

send() { printf '%s\n' "$@" >>"$cmd"; }
wait_file() { for _ in $(seq 120); do [ -s "$1" ] && return 0; sleep 0.25; done; echo "timed out waiting for $1" >&2; return 1; }
# The state file, fresh.
state() {
    rm -f "$work/state.json"
    send "EXPORTSTATE $(w "$work/state.json")"
    wait_file "$work/state.json"
    sleep 0.2
}
# "x y" of the first card of kind $1 (group $2: "top" or "any"): the centre of the
# part of its rect inside the window, since a card can hang off the edge.
center() {
    state
    python -P -c '
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
kind, where, W, H = sys.argv[2], sys.argv[3], float(sys.argv[4]), float(sys.argv[5])
for c in d["cards"]:
    if c["kind"] == kind and (where == "any" or c["group"] is None):
        x0, y0, x1, y1 = c["rect"]
        print(int((max(x0, 0) + min(x1, W)) / 2), int((max(y0, 36) + min(y1, H)) / 2))
        break
' "$(w "$work/state.json")" "$1" "${2:-top}" "${SIZE% *}" "${SIZE#* }" </dev/null
}
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
has() { case " $VIEWS " in *" $1 "*) return 0 ;; esac; return 1; }

send "RESIZE $SIZE" WAITTABS "SLEEP 3"
if [ -s "$work/pic.png" ]; then send "ADDFILE $(w "$work/pic.png") 160 640"; fi
send "ADDFILE $(w "$work/data.json") 640 700" "SLEEP 2"

if has graph; then shot graph; fi
if has graph-hover; then
    read -r x y < <(center Tab); send "MOUSEMOVE $x $y" "SLEEP 1"; shot graph-hover; send "MOUSEMOVE 5 790"
fi
for v in tab:Tab group:Group note:Note; do
    name="graph-menu-${v%%:*}"
    if has "$name"; then
        read -r x y < <(center "${v##*:}"); send "MOUSEMOVE $x $y" "MOUSEDOWN $x $y right" "MOUSEUP $x $y right" "SLEEP 1"; shot "$name"; reset
    fi
done
if has graph-filter; then send "GRAPHFILTERTEXT res" "SLEEP 1"; shot graph-filter; send "GRAPHFILTERTEXT "; fi
if has graph-group; then
    read -r x y < <(center Group); send "DOUBLECLICK $x $y" "SLEEP 3"; shot graph-group; reset
fi
if has json-level; then
    read -r x y < <(center Json); send "DOUBLECLICK $x $y" "SLEEP 3"; shot json-level; reset
fi
if has tabview || has tabview-dropdown || has tabview-find || has tabview-actions; then
    read -r x y < <(center Tab); send "DOUBLECLICK $x $y" "SLEEP 5"
    has tabview && shot tabview
    if has tabview-dropdown; then send "ADDRESSBAR exam" "SLEEP 2"; shot tabview-dropdown; send ESCAPE "SLEEP 1"; fi
    if has tabview-find; then send OPENFINDBAR "FINDTEXT abc" "SLEEP 1"; shot tabview-find; send CLOSEFINDBAR; fi
    if has tabview-actions; then send "CLICK $((${SIZE% *} - 145)) 16" "SLEEP 1"; shot tabview-actions; send ESCAPE; fi
    reset
fi
if has settings; then send TOGGLESETTINGSPAGE "SLEEP 2"; shot settings; send TOGGLESETTINGSPAGE "SLEEP 1"; fi
if has shortcuts; then send TOGGLESHORTCUTSHELP "SLEEP 1"; shot shortcuts; send TOGGLESHORTCUTSHELP "SLEEP 1"; fi
if has quickswitch; then send QUICKSWITCHOPEN "SLEEP 1"; shot quickswitch; send QUICKSWITCHESCAPE "SLEEP 1"; fi
send QUIT
wait "$inst" 2>/dev/null
inst=""

python -P - "$OUT" "$CROP_PX" "$VIEWS" </dev/null <<'EOF' 2>&1 | tail -2
import os, sys
out, crop_px, views = sys.argv[1], int(sys.argv[2]), sys.argv[3].split()
try:
    from PIL import Image
except ImportError:
    print("Pillow not installed: no crops or sheet")
    sys.exit(0)
imgs = []
for v in views:
    p = os.path.join(out, v + ".png")
    if not os.path.exists(p):
        continue
    im = Image.open(p).convert("RGB")
    im.crop((0, 0, im.width, min(crop_px, im.height))).save(os.path.join(out, "crop", v + ".png"))
    imgs.append(im)
if imgs:
    cols = 3
    tw = 640
    th = int(imgs[0].height * tw / imgs[0].width)
    rows = (len(imgs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * tw, rows * th), (30, 30, 30))
    for i, im in enumerate(imgs):
        sheet.paste(im.resize((tw, th)), ((i % cols) * tw, (i // cols) * th))
    sheet.save(os.path.join(out, "sheet.png"))
    print(f"{len(imgs)} views: {out}")
EOF
