#!/usr/bin/env bash
# End-to-end probe of the cookie jar: a WebView2 tab logs in, the 30 s pull writes the jar file, and a
# fresh run's CEF tab, then WebView2 tab, must be authenticated on their first request. The host is a name
# (`jar.localhost`, which Chromium resolves to loopback), not an IP, so a host-only cookie can be told apart
# from a domain cookie: after the CEF round trip the jar's sid must still be host-only (no dotted copy).
# Last, a two-tab run reports whether two WebView2 InPrivate tabs (separate host processes) share cookies:
# tab B, seeded before tab A logs in, asks /whoami?b after A's login. Informational, not pass/fail.
# Uses a throwaway APPDATA (never the real profile) and prints counts, flags and the test server's domain
# only, no cookie values. Build first (`cargo build`); this runs the existing target/debug exes.
# Usage: scripts/jar-probe.sh   (JAR_PROBE_HOST=127.0.0.1 to use an IP host instead)
set -u
repo="$(cd "$(dirname "$0")/.." && pwd)"
sp="$repo/scripts"
exe="$repo/target/debug/manifold.exe"
host="${JAR_PROBE_HOST:-jar.localhost}"
out="$(mktemp -d)"
python -P "$sp/cookie_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/req.log")" 300 </dev/null >/dev/null 2>&1 &
srv=$!
trap 'kill "$srv" 2>/dev/null' EXIT
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"
base="http://$host:$port"
profile="$out/appdata"; mkdir -p "$profile/manifold"
# The home page is what NEWTAB opens: tab B of the two-tab run below.
echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"webview2\", \"home_page\": \"$base/whoami?b\", \"cef_directory\": \"$(cygpath -m "$repo/target/debug/cef-runtime")\"}" >"$profile/manifold/settings.json"

# run <name> <tab engine> <path> <seconds to stay> [debug commands...]: one headless app run with a single
# tab; the extra commands run after the tab opens, before the stay.
run() {
  local name="$1" engine="$2" path="$3" stay="$4"
  shift 4
  cat >"$profile/seed.json" <<JSON
{"nodes": [{"id": "t", "kind": "tab", "url": "$base$path", "engine": "$engine", "pos": [0.0, 0.0]}]}
JSON
  rm -f "$profile"/manifold/session.mnf*
  "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
  local cmd="$profile/cmd-$name"; : >"$cmd"
  echo "--- run $name ($engine $path)"
  APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log-$name" 2>&1 &
  local app=$!
  { echo WAITTABS; echo GRAPHRIGHT; echo GRAPHOPEN; printf '%s\n' "$@"; echo "SLEEP $stay"; echo QUIT; } >>"$cmd"
  wait "$app" 2>/dev/null
  # Hosts log to %TEMP%/manifold-<tab id>.log (process_manager::per_tab_log_path), not to the app's stderr.
  find /tmp -maxdepth 1 -name 'manifold-*.log' -newer "$cmd" -exec grep -ah "^jar:" {} + | sort | uniq -c | cut -c1-200
}
cat >"$out/jar_summary.py" <<'PY'
import ctypes, json, sys
from ctypes import wintypes
class Blob(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]
def unprotect(data):
    src = Blob(len(data), ctypes.cast(ctypes.c_char_p(data), ctypes.POINTER(ctypes.c_char)))
    out = Blob()
    if not ctypes.windll.crypt32.CryptUnprotectData(ctypes.byref(src), None, None, None, None, 0, ctypes.byref(out)):
        raise OSError("DPAPI unprotect failed")
    res = ctypes.string_at(out.pbData, out.cbData)
    ctypes.windll.kernel32.LocalFree(out.pbData)
    return res
try:
    raw = open(sys.argv[1], "rb").read()
    if not raw.startswith(b"MJR1"):
        print("jar file: NOT PROTECTED (no MJR1 header)"); sys.exit()
    j = json.loads(unprotect(raw[4:]))
except Exception as e:
    print("jar file: missing or unreadable:", type(e).__name__); sys.exit()
cs = j if isinstance(j, list) else j.get("cookies", [])
sid = [c for c in cs if c.get("name") == "sid"]
hp = [c for c in cs if c.get("name") == "__Host-hp"]
print(f"jar file: protected (MJR1); {len(cs)} cookies; sid present={bool(sid)} http_only={[c.get('http_only') for c in sid]}"
      f" sid domains={[c.get('domain') for c in sid]} __Host-hp domains={[c.get('domain') for c in hp]}")
PY
fail=0
jar_summary() {
  local line
  line="$(python -P "$(cygpath -w "$out/jar_summary.py")" "$(cygpath -w "$profile/manifold/jars/default.json")" </dev/null)"
  echo "$line"
  case "$line" in *"sid present=True http_only=[True]"*) ;; *) echo "FAIL: jar lacks one HttpOnly sid"; fail=1 ;; esac
  case "$line" in *"sid domains=['$host']"*) ;; *) echo "FAIL: sid is not exactly one host-only cookie on $host"; fail=1 ;; esac
}

run login webview2 /login 38
jar_summary
run cef cef /whoami 12
jar_summary
run webview2 webview2 /whoami 12
echo "=== requests (engine path auth cookie-names)"; awk '{print $2, $3, $4, $5}' "$out/req.log"
echo "=== first /whoami per engine"
for e in cef webview2; do
  first="$(grep " $e /whoami" "$out/req.log" | head -1 | awk '{print $4}')"
  echo "$e: $first"
  [ "$first" = "auth=True" ] || { echo "FAIL: $e's first /whoami was not authenticated"; fail=1; }
done
jar_summary

# Two WebView2 tabs in one run, from an empty jar: A logs in, then B (NEWTAB opens the home page, in a second
# host process seeded with the still-empty jar) asks /whoami?b. Nothing pulls in between (the first 30 s pull
# ran before A existed), so B is authenticated only if the two InPrivate tabs share one cookie store.
rm -rf "$profile/manifold/jars"
n0=$(wc -l <"$out/req.log")
run twotab webview2 /login 6 "SLEEP 4" NEWTAB WAITTABS
echo "=== two-tab requests"; tail -n +"$((n0 + 1))" "$out/req.log" | awk '{print $2, $3, $4, $5}'
b="$(grep " webview2 /whoami?b" "$out/req.log" | head -1 | awk '{print $4}')"
echo "=== two-tab: tab B's /whoami?b: ${b:-no request}"
case "$b" in
  auth=True) echo "two-tab: WebView2 InPrivate tabs SHARE cookies across host processes" ;;
  auth=False) echo "two-tab: WebView2 InPrivate tabs do NOT share cookies" ;;
  *) echo "two-tab: inconclusive (tab B made no request)" ;;
esac
echo "out=$out"
[ "$fail" = 0 ] && echo "PROBE PASS" || echo "PROBE FAIL"
exit "$fail"
