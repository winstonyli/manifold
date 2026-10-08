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

exit $fails
