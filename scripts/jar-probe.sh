#!/usr/bin/env bash
# End-to-end probe of the cookie jar: a WebView2 tab logs in, the 30 s pull writes the jar file, and a
# fresh run's CEF tab, then WebView2 tab, must be authenticated on their first request.
# Uses a throwaway APPDATA (never the real profile) and prints counts and flags only, no cookie values.
# Build first (`cargo build`); this runs the existing target/debug exes. Usage: scripts/jar-probe.sh
set -u
repo="$(cd "$(dirname "$0")/.." && pwd)"
sp="$repo/scripts"
exe="$repo/target/debug/manifold.exe"
out="$(mktemp -d)"
python -P "$sp/cookie_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/req.log")" 240 </dev/null >/dev/null 2>&1 &
srv=$!
trap 'kill "$srv" 2>/dev/null' EXIT
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"
profile="$out/appdata"; mkdir -p "$profile/manifold"
echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"webview2\", \"cef_directory\": \"$(cygpath -m "$repo/target/debug/cef-runtime")\"}" >"$profile/manifold/settings.json"

# run <name> <tab engine> <path> <seconds to stay>: one headless app run with a single tab.
run() {
  local name="$1" engine="$2" path="$3" stay="$4"
  cat >"$profile/seed.json" <<JSON
{"nodes": [{"id": "t", "kind": "tab", "url": "http://127.0.0.1:$port$path", "engine": "$engine", "pos": [0.0, 0.0]}]}
JSON
  rm -f "$profile"/manifold/session.mnf*
  "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
  local cmd="$profile/cmd-$name"; : >"$cmd"
  echo "--- run $name ($engine $path)"
  APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log-$name" 2>&1 &
  local app=$!
  { echo WAITTABS; echo GRAPHRIGHT; echo GRAPHOPEN; echo "SLEEP $stay"; echo QUIT; } >>"$cmd"
  wait "$app" 2>/dev/null
  grep -a "^jar:" "$profile/log-$name" | cut -c1-200
}
cat >"$out/jar_summary.py" <<'PY'
import json, sys
try:
    j = json.load(open(sys.argv[1]))
except Exception as e:
    print("jar file: missing or unreadable:", type(e).__name__); sys.exit()
cs = j if isinstance(j, list) else j.get("cookies", [])
sid = [c for c in cs if c.get("name") == "sid"]
print(f"jar file: {len(cs)} cookies; sid present={bool(sid)} http_only={[c.get('http_only') for c in sid]}")
PY
fail=0
jar_summary() {
  local line
  line="$(python -P "$(cygpath -w "$out/jar_summary.py")" "$(cygpath -w "$profile/manifold/jars/default.json")" </dev/null)"
  echo "$line"
  case "$line" in *"sid present=True http_only=[True]"*) ;; *) echo "FAIL: jar lacks an HttpOnly sid"; fail=1 ;; esac
}

run login webview2 /login 38
jar_summary
n0=$(wc -l <"$out/req.log")
run cef cef /whoami 12
run webview2 webview2 /whoami 12
echo "=== requests (engine path auth)"; awk '{print $2, $3, $4}' "$out/req.log"
echo "=== first /whoami per engine"
for e in cef webview2; do
  first="$(grep " $e /whoami" "$out/req.log" | head -1 | awk '{print $4}')"
  echo "$e: $first"
  [ "$first" = "auth=True" ] || { echo "FAIL: $e's first /whoami was not authenticated"; fail=1; }
done
jar_summary
echo "out=$out"
[ "$fail" = 0 ] && echo "PROBE PASS" || echo "PROBE FAIL"
exit "$fail"
