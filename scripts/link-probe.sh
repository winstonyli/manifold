#!/usr/bin/env bash
# Usage: scripts/link-probe.sh
#
# Checks links end to end (spec 2026-10-06-links) through the real exe:
#   1. LINK a b between two tabs creates one link, a -> b, in EXPORTSTATE.
#   2. After a restart on the same profile the link is still there, same id.
#   3. UNLINK b a (the other order) removes it, and a second restart still
#      has none (the removal was saved).
# One headless instance per run, on a fresh isolated profile seeded with two
# about:blank tabs (import gives them fresh ids, so the first run reads them
# from EXPORTSTATE), state read back through EXPORTSTATE. Runs BelowNormal.
# Build first: cargo build -p manifold.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

profile="$out/appdata"
mkdir -p "$profile/manifold"
echo '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' >"$profile/manifold/settings.json"
cat >"$profile/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]},
 {"id": "t2", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [400.0, 0.0]}
]}
EOF
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null || exit 1

cmd="$profile/cmd"
send() { printf '%s\n' "$@" >>"$cmd"; }
# start <log>: one instance on the profile, at BelowNormal, its tabs up.
start() {
    : >"$cmd"
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$out/$1" 2>&1 &
    inst=$!
    winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"
    [ -n "$winpid" ] && powershell -NoProfile -Command "try { (Get-Process -Id $winpid -ErrorAction Stop).PriorityClass = 'BelowNormal' } catch {}" </dev/null >/dev/null 2>&1
    send WAITTABS "SLEEP 1"
}
stop() { send QUIT; wait "$inst" 2>/dev/null; inst=""; }
x() { echo "EXPORTSTATE $(cygpath -w "$out/$1.json")"; }
wait_file() { for _ in $(seq 240); do [ -s "$1" ] && return 0; sleep 0.25; done; echo "timed out waiting for $1" >&2; return 1; }
py() { python -P -c "$1" "$(cygpath -w "$out/$2.json")" </dev/null 2>&1 | tr -d '\r'; }

start log1
send "$(x s0)"
wait_file "$out/s0.json" && sleep 0.2
read -r ncards a b <<<"$(py 'import json,sys; c=json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(len(c), *[x["id"] for x in c])' s0)"
send "LINK $a $b" "SLEEP 1" "$(x s1)"
stop
start log2
send "$(x s2)" "UNLINK $b $a" "SLEEP 1" "$(x s3)"
stop
start log3
send "$(x s4)"
stop

# Prints "<link count> <first link's id> <its from> <its to>" for one EXPORTSTATE file.
summary() {
    py 'import json,sys; l=json.load(open(sys.argv[1], encoding="utf-8"))["links"]; print(len(l), *((l[0]["id"], l[0]["from"], l[0]["to"]) if l else ("-", "-", "-")))' "$1"
}
status=0
check() { # name want got
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
read -r n1 id1 from1 to1 <<<"$(summary s1)"
read -r n2 id2 from2 to2 <<<"$(summary s2)"
read -r n3 _ <<<"$(summary s3)"
read -r n4 _ <<<"$(summary s4)"
check "the session has the two seeded tabs" "2" "$ncards"
check "LINK creates one link" "1" "$n1"
check "it runs a -> b" "$a $b" "$from1 $to1"
check "the link persists across a restart" "1" "$n2"
check "with the same id and ends" "$id1 $a $b" "$id2 $from2 $to2"
check "UNLINK (ends in the other order) removes it" "0" "$n3"
check "the removal persists across a restart" "0" "$n4"
if [ $status -ne 0 ]; then
    for l in log1 log2 log3; do echo "--- $l"; tail -n 20 "$out/$l"; done
fi
exit $status
