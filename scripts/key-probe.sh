#!/usr/bin/env bash
# Usage: [ENGINES="webview2 cef"] scripts/key-probe.sh
#
# Checks what a real page sees when the host sends it keys (`KEYPAGE`): for
# each engine, one headless instance on a fresh isolated APPDATA loads a
# local page (scripts/key_probe_server.py) that reports each keydown's
# code/key/keyCode, and this script compares them with what
# `ipc_protocol::KeyCode` (`key_codes!`) says each engine should send.
# Guards the shared key table against an edit that changes one engine's
# behaviour. Expected differences by engine, both long-standing:
#   - CEF sends no DOM `code` (its KeyEvent has no such field), so code is
#     not compared there;
#   - modifiers' keyCode is the generic VK_SHIFT (16) on WebView2 and the
#     side-specific VK_LSHIFT (160) on CEF.
# Build first: cargo build -p manifold -p manifold-engine-cef -p manifold-engine-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
mkdir -p "$out"
srv=""
inst=""
cleanup() {
    [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

# The keys sent, in order, and the lines the page should log per engine.
keys="Enter ArrowLeft ShiftLeft Digit7 PageDown KeyA"
expected() {
    case "$1" in
    webview2) cat <<'EOF'
code=Enter key=Enter kc=13
code=ArrowLeft key=ArrowLeft kc=37
code=ShiftLeft key=Shift kc=16
code=Digit7 key=7 kc=55
code=PageDown key=PageDown kc=34
code=KeyA key=a kc=65
EOF
        ;;
    cef) cat <<'EOF'
key=Enter kc=13
key=ArrowLeft kc=37
key=Shift kc=160
key=7 kc=55
key=PageDown kc=34
key=a kc=65
EOF
        ;;
    esac
}

status=0
for engine in ${ENGINES:-webview2 cef}; do
    profile="$out/appdata-$engine"
    mkdir -p "$profile/manifold"
    log="$out/keys-$engine.log"
    : >"$log"
    python -P "$repo_root/scripts/key_probe_server.py" "$(cygpath -w "$out/port-$engine")" "$(cygpath -w "$log")" 300 </dev/null >/dev/null 2>&1 &
    srv=$!
    for _ in $(seq 50); do [ -s "$out/port-$engine" ] && break; sleep 0.1; done
    port="$(cat "$out/port-$engine")"
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"$engine\"}" >"$profile/manifold/settings.json"
    echo "{\"nodes\": [{\"id\": \"t1\", \"kind\": \"tab\", \"url\": \"http://127.0.0.1:$port/\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    {
        echo WAITTABS
        # The first command after WAITTABS needs a drawn frame to land; without this one the app quit early.
        echo "EXPORTSTATE $(cygpath -w "$out/early-$engine.json")"
        echo "SLEEP 3"
        echo GRAPHDOWN
        echo GRAPHOPEN
        echo "SLEEP 8"
        # Each command draws a frame, and the view change needs one.
        echo "EXPORTSTATE $(cygpath -w "$out/state-$engine.json")"
        echo "CLICK 300 200"
        echo "SLEEP 1"
        for k in $keys; do echo "KEYPAGE $k"; done
        echo "SLEEP 3"
        echo QUIT
    } >>"$cmd"
    wait "$inst" 2>/dev/null
    inst=""
    kill "$srv" 2>/dev/null
    srv=""

    # CEF sends no `code`: drop it from what the page logged.
    got="$(cat "$log")"
    [ "$engine" = cef ] && got="$(sed 's/^code= //' "$log")"
    if [ "$got" = "$(expected "$engine")" ]; then
        echo "PASS: $engine: $(wc -l <"$log") keys as expected"
    else
        echo "FAIL: $engine: the page logged"
        echo "$got" | sed 's/^/    /'
        echo "  expected"
        expected "$engine" | sed 's/^/    /'
        status=1
    fi
done
exit $status
