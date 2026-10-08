#!/usr/bin/env bash
# Usage: bash scripts/lib-test.sh
# Checks scripts/lib.sh's helpers without the app, in a few seconds. Exits 0 if every check passed.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
fails=0
expect() { # name want got
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "NOT OK: $1: got [$3], wanted [$2]"; fails=1; fi
}

seed_settings "$t/p1"
expect "seed_settings: webview2 by default" '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' "$(cat "$t/p1/manifold/settings.json")"
expect "seed_settings: ends in a newline" 1 "$(tail -c1 "$t/p1/manifold/settings.json" | wc -l)"
seed_settings "$t/p2" cef
expect "seed_settings: the engine" '{"has_shown_first_launch_demo": true, "default_starting_engine": "cef"}' "$(cat "$t/p2/manifold/settings.json")"
seed_settings "$t/p3" webview2 ', "home_page": "http://x/"'
expect "seed_settings: extra members" '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2", "home_page": "http://x/"}' "$(cat "$t/p3/manifold/settings.json")"

# A fake instance: prints its first argument, APPDATA, HTTPS_PROXY and the first command line, then waits for an
# END line, so it is still running while launch_headless looks up its Windows pid.
cat >"$t/fake-exe" <<'EOF'
#!/usr/bin/env bash
read -r first
printf '%s\n' "$1" "$APPDATA" "${HTTPS_PROXY:-none}" "$first"
while read -r line; do [ "$line" = END ] && break; done
EOF
chmod +x "$t/fake-exe"
cmd="$t/cmd"
: >"$cmd"
send "FIRST LINE" SECOND
expect "send: one line per argument" "FIRST LINE
SECOND" "$(cat "$cmd")"
HTTPS_PROXY=http://proxy.test launch_headless "$t/fake-exe" "$t/prof" "$cmd" "$t/fake.log"
pid=$!
expect "launch_headless: \$! is still the instance after the priority lookup" yes "$(kill -0 "$pid" 2>/dev/null && echo yes)"
expect "launch_headless: launched_winpid is set" yes "$([ -n "$launched_winpid" ] && echo yes)"
send END
wait "$pid"
expect "launch_headless: \$! is the instance, and it exits 0" 0 "$?"
expect "launch_headless: --headless" --headless "$(sed -n 1p "$t/fake.log")"
expect "launch_headless: APPDATA is the profile, Windows-style" "$(cygpath -w "$t/prof")" "$(sed -n 2p "$t/fake.log")"
expect "launch_headless: a prefix assignment reaches the instance" http://proxy.test "$(sed -n 3p "$t/fake.log")"
expect "launch_headless: commands from the file's first line" "FIRST LINE" "$(sed -n 4p "$t/fake.log")"

# The producer waits for a marker, so the file turns up while wait_file is polling (no fixed sleep).
(until [ -e "$t/go" ]; do sleep 0.05; done; echo x >"$t/late") &
: >"$t/go"
wait_file "$t/late"
expect "wait_file: a file that turns up later" 0 "$?"
WAIT_FILE_SECS=1 wait_file "$t/never" 2>"$t/wait.err"
expect "wait_file: gives up after WAIT_FILE_SECS" 1 "$?"
expect "wait_file: says what it waited for" "timed out waiting for $t/never" "$(cat "$t/wait.err")"
: >"$t/empty"
WAIT_FILE_SECS=1 wait_file "$t/empty" 2>/dev/null
expect "wait_file: an empty file is not there yet" 1 "$?"

expect "check: a match passes and leaves status" "PASS: same
status=0" "$(status=0; check same a a; echo "status=$status")"
expect "check: a mismatch says got and wanted, and sets status" "FAIL: differs: got [b], wanted [a]
status=1" "$(status=0; check differs a b; echo "status=$status")"

exit $fails
