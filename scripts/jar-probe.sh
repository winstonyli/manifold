#!/usr/bin/env bash
# End-to-end probe of the cookie jar: a WebView2 tab logs in, the 30 s pull writes the jar file, and a
# fresh run's CEF tab, then WebView2 tab, must be authenticated on their first request. The host is a name
# (`jar.localhost`, which Chromium resolves to loopback), not an IP, so a host-only cookie can be told apart
# from a domain cookie: after the CEF round trip the jar's sid must still be host-only (no dotted copy).
# Then a two-tab run reports whether two WebView2 InPrivate tabs (separate host processes) share cookies:
# tab B, seeded before tab A logs in, asks /whoami?b after A's login. Informational, not pass/fail.
# WebView2 hosts take turns with a pool of profile names (manifold_0..7), so three runs check reuse: a fresh jar
# after the login run (slot 0 again, new browser), a tab closed and replaced while another keeps the browser alive
# (the replacement may reuse its slot), and ten tabs opened and closed in turn: each reused profile must start
# empty, and WebView2's Local State must gain no profile names outside the pool.
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
  find /tmp -maxdepth 1 -name 'manifold-*.log' -newer "$cmd" -exec grep -ah "^jar:" {} + | sort | uniq -c | cut -c1-200 | tee "$out/jar-$name"
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
# Fresh jar, same user-data folder: this run's tab reuses the login run's profile name and must not be signed in.
mv "$profile/manifold/jars" "$profile/manifold/jars.kept"
n0=$(wc -l <"$out/req.log")
run fresh webview2 /whoami?fresh 6
f="$(tail -n +"$((n0 + 1))" "$out/req.log" | grep " webview2 /whoami?fresh" | head -1 | awk '{print $4}')"
echo "fresh jar, reused profile: /whoami?fresh ${f:-no request}"
[ "$f" = "auth=False" ] || { echo "FAIL: a reused profile name carried cookies across runs"; fail=1; }
rm -rf "$profile/manifold/jars"; mv "$profile/manifold/jars.kept" "$profile/manifold/jars"
run cef cef /whoami 12
jar_summary
run webview2 webview2 /whoami 12
echo "=== requests (engine path auth cookie-names)"; awk '{print $2, $3, $4, $5}' "$out/req.log"
echo "=== first /whoami per engine"
for e in cef webview2; do
  first="$(awk -v e="$e" '$2 == e && $3 == "/whoami" {print $4; exit}' "$out/req.log")"
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

# Slot reuse while the browser lives: tab C (the first host, whose job holds the browser process; started alone
# first) stays open; tab A logs in and closes; tab B then takes the lowest free profile name, A's if its folder could
# be deleted. All within the first 30 s, so no pull carries A's login into the jar (B's `seeded 0 cookies` confirms
# it). NEWTAB leaves focus on C, so NEXTTAB (tabs cycle in creation order) moves it to the new tab before
# ADDRESSBAR and CLOSETAB; closing a tab leaves nothing focused.
rm -rf "$profile/manifold/jars"
n0=$(wc -l <"$out/req.log")
run reuse webview2 /whoami?c 4 "SLEEP 4" NEWTAB WAITTABS NEXTTAB "ADDRESSBAR $base/login" "SLEEP 1" ENTER "SLEEP 3" \
  "ADDRESSBAR $base/whoami?a" "SLEEP 1" ENTER "SLEEP 3" CLOSETAB "SLEEP 7" NEWTAB WAITTABS "SLEEP 3"
reuse_log="$(tail -n +"$((n0 + 1))" "$out/req.log")"
a="$(grep " webview2 /whoami?a" <<<"$reuse_log" | head -1 | awk '{print $4}')"
r="$(grep " webview2 /whoami?b" <<<"$reuse_log" | sed -n 2p | awk '{print $4}')"
echo "reuse: tab A after its login: ${a:-no request}; tab B's /whoami?b: ${r:-no request}"
[ "$a" = "auth=True" ] || { echo "FAIL: tab A did not log in, so the reuse check proves nothing"; fail=1; }
[ "$r" = "auth=False" ] || { echo "FAIL: tab B carried cookies (a reused profile was not empty)"; fail=1; }
# A and B must share a profile name (C keeps manifold_0, so both get manifold_1), or B's check proves nothing.
grep -q " 2 jar: InPrivate profile manifold_1$" "$out/jar-reuse" ||
  { echo "FAIL: tab B did not reuse tab A's profile name manifold_1"; fail=1; }

# Ten WebView2 tabs opened and closed in turn: Local State may gain no profile names outside the pool.
cat >"$out/profiles.py" <<'PY'
import json, re, sys
try:
    st = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception as e:
    print("unreadable:", type(e).__name__); sys.exit()
names = (re.fullmatch(r"(?:WV2Profile_)?(manifold_\d+)", k) for k in st.get("profile", {}).get("info_cache", {}))
print(" ".join(sorted(m.group(1) for m in names if m)))
PY
ls="$(cygpath -w "$repo/target/debug/manifold-engine-webview2.exe.WebView2/EBWebView/Local State")"
before="$(python -P "$(cygpath -w "$out/profiles.py")" "$ls" </dev/null)"
# Focus is on C at first and on nothing after a close; NEXTTAB from nothing lands on C (the first tab), so later
# rounds need two to reach the new tab.
cycle=("SLEEP 4")
for i in $(seq 10); do
  cycle+=(NEWTAB WAITTABS NEXTTAB); [ "$i" = 1 ] || cycle+=(NEXTTAB)
  cycle+=("ADDRESSBAR $base/whoami?t$i" "SLEEP 1" ENTER "SLEEP 2" CLOSETAB "SLEEP 6")
done
rm -rf "$profile/manifold/jars"
run ten webview2 /whoami?c 2 "${cycle[@]}"
after="$(python -P "$(cygpath -w "$out/profiles.py")" "$ls" </dev/null)"
added="$(comm -13 <(tr ' ' '\n' <<<"$before" | sort) <(tr ' ' '\n' <<<"$after" | sort) | grep -v '^$' | tr '\n' ' ')"
pool="$(tr ' ' '\n' <<<"$after" | grep -cE '^manifold_[0-7]$')"
echo "ten tabs: Local State profile names added this run: [${added% }]; pool names present: $pool of 8"
names="$(grep -c "InPrivate profile" "$out/jar-ten")"
tn="$(grep -cE " webview2 /whoami\?t[0-9]+ " "$out/req.log")"
echo "ten tabs: $names distinct profile names used; $tn of 10 tabs navigated"
[ "$tn" = 10 ] || { echo "FAIL: not every tab navigated, so not every tab was opened and closed as meant"; fail=1; }
for p in $added; do
  case "$p" in manifold_[0-7]) ;; *) echo "FAIL: Local State gained $p, outside the pool"; fail=1 ;; esac
done
echo "out=$out"
[ "$fail" = 0 ] && echo "PROBE PASS" || echo "PROBE FAIL"
exit "$fail"
