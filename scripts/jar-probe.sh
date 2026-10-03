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
# Per-tab jars (spec 2026-10-03-per-tab-jars-design.md): `perjar` signs two tabs on the named jars Alice and Bob in
# as two probe users, and each login must land in its own jar's file with the default jar untouched; `jarswitch`
# restarts on that profile, moves Alice's tab to Bob with SETJAR and SWITCHJARNOW, and its next request must be Bob's.
# Uses a throwaway APPDATA (never the real profile) and prints counts, flags, the test server's domain and probe
# usernames only, no cookie values. Build first (`cargo build`); this runs the existing target/debug exes, each at
# below-normal priority.
# Usage: scripts/jar-probe.sh   (JAR_PROBE_HOST=127.0.0.1 to use an IP host instead; RUNS="perjar jarswitch" runs
# only the named runs; jarswitch needs perjar's profile, so it is skipped without it)
set -u
repo="$(cd "$(dirname "$0")/.." && pwd)"
sp="$repo/scripts"
exe="$repo/target/debug/manifold.exe"
host="${JAR_PROBE_HOST:-jar.localhost}"
out="$(mktemp -d)"
python -P "$sp/cookie_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/req.log")" 1200 </dev/null >/dev/null 2>&1 &
srv=$!
trap 'kill "$srv" 2>/dev/null' EXIT
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"
base="http://$host:$port"
profile="$out/appdata"; mkdir -p "$profile/manifold"
# The home page is what NEWTAB opens: tab B of the two-tab run below.
echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"webview2\", \"home_page\": \"$base/whoami?b\", \"cef_directory\": \"$(cygpath -m "$repo/target/debug/cef-runtime")\"}" >"$profile/manifold/settings.json"

# wanted <run>: RUNS is empty or names it.
wanted() { [ -z "${RUNS:-}" ] || [[ " $RUNS " == *" $1 "* ]]; }

# seed <json>: the profile's session becomes a fresh import of <json>.
seed() {
  printf '%s\n' "$1" >"$profile/seed.json"
  rm -f "$profile"/manifold/session.mnf*
  "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
}

# start <name> [debug commands...]: one headless app run on the profile's session as it stands; the commands run in
# order, then QUIT. The app runs at below-normal priority, set before its hosts spawn so they inherit it.
start() {
  local name="$1"
  shift
  local cmd="$profile/cmd-$name"; : >"$cmd"
  APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log-$name" 2>&1 &
  local app=$!
  local winpid; winpid="$(cat "/proc/$app/winpid" 2>/dev/null)"
  [ -n "$winpid" ] && powershell.exe -NoProfile -Command "(Get-Process -Id $winpid).PriorityClass = 'BelowNormal'" >/dev/null 2>&1
  echo "    app pid ${winpid:-?}, started $(date +%T), log $(cygpath -w "$profile/log-$name")"
  { printf '%s\n' "$@"; echo QUIT; } >>"$cmd"
  wait "$app" 2>/dev/null
  # Hosts log to %TEMP%/manifold-<tab id>.log (process_manager::per_tab_log_path), not to the app's stderr.
  find /tmp -maxdepth 1 -name 'manifold-*.log' -newer "$cmd" -exec grep -ah "^jar:" {} + 2>/dev/null | sort | uniq -c | cut -c1-200 | tee "$out/jar-$name"
}

# run <name> <tab engine> <path> <seconds to stay> [debug commands...]: one headless app run with a single
# tab; the extra commands run after the tab opens, before the stay.
run() {
  local name="$1" engine="$2" path="$3" stay="$4"
  shift 4
  seed "{\"nodes\": [{\"id\": \"t\", \"kind\": \"tab\", \"url\": \"$base$path\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}"
  echo "--- run $name ($engine $path)"
  start "$name" WAITTABS GRAPHRIGHT GRAPHOPEN "$@" "SLEEP $stay"
}
cat >"$out/dpapi.py" <<'PY'
import ctypes, json
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
def load_jar(path):
    """The jar file's cookies; raises when it is missing, unreadable or not MJR1-protected."""
    raw = open(path, "rb").read()
    if not raw.startswith(b"MJR1"):
        raise ValueError("NOT PROTECTED (no MJR1 header)")
    j = json.loads(unprotect(raw[4:]))
    return j if isinstance(j, list) else j.get("cookies", [])
PY
cat >"$out/jar_summary.py" <<'PY'
import sys
sys.path.insert(0, sys.argv[2])
from dpapi import load_jar
try:
    cs = load_jar(sys.argv[1])
except Exception as e:
    print("jar file: missing or unreadable:", type(e).__name__, e if isinstance(e, ValueError) else ""); sys.exit()
sid = [c for c in cs if c.get("name") == "sid"]
hp = [c for c in cs if c.get("name") == "__Host-hp"]
print(f"jar file: protected (MJR1); {len(cs)} cookies; sid present={bool(sid)} http_only={[c.get('http_only') for c in sid]}"
      f" sid domains={[c.get('domain') for c in sid]} __Host-hp domains={[c.get('domain') for c in hp]}")
PY
fail=0
# jar_summary [jar file]: the default jar unless named.
jar_summary() {
  local line
  line="$(python -P "$(cygpath -w "$out/jar_summary.py")" "$(cygpath -w "${1:-$profile/manifold/jars/default.json}")" "$(cygpath -w "$out")" </dev/null)"
  echo "$line"
  case "$line" in *"sid present=True http_only=[True]"*) ;; *) echo "FAIL: jar lacks one HttpOnly sid"; fail=1 ;; esac
  case "$line" in *"sid domains=['$host']"*) ;; *) echo "FAIL: sid is not exactly one host-only cookie on $host"; fail=1 ;; esac
}

if wanted login || wanted fresh || wanted cef || wanted webview2; then
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
echo "=== requests (engine path auth user cookie-names)"; awk '{print $2, $3, $4, $5, $6}' "$out/req.log"
echo "=== first /whoami per engine"
for e in cef webview2; do
  first="$(awk -v e="$e" '$2 == e && $3 == "/whoami" {print $4; exit}' "$out/req.log")"
  echo "$e: $first"
  [ "$first" = "auth=True" ] || { echo "FAIL: $e's first /whoami was not authenticated"; fail=1; }
done
jar_summary
fi

# Two WebView2 tabs in one run, from an empty jar: A logs in, then B (NEWTAB opens the home page, in a second
# host process seeded with the still-empty jar) asks /whoami?b. Nothing pulls in between (the first 30 s pull
# ran before A existed), so B is authenticated only if the two InPrivate tabs share one cookie store.
if wanted twotab; then
rm -rf "$profile/manifold/jars"
n0=$(wc -l <"$out/req.log")
run twotab webview2 /login 6 "SLEEP 4" NEWTAB WAITTABS
echo "=== two-tab requests"; tail -n +"$((n0 + 1))" "$out/req.log" | awk '{print $2, $3, $4, $5, $6}'
b="$(grep " webview2 /whoami?b" "$out/req.log" | head -1 | awk '{print $4}')"
echo "=== two-tab: tab B's /whoami?b: ${b:-no request}"
case "$b" in
  auth=True) echo "two-tab: WebView2 InPrivate tabs SHARE cookies across host processes" ;;
  auth=False) echo "two-tab: WebView2 InPrivate tabs do NOT share cookies" ;;
  *) echo "two-tab: inconclusive (tab B made no request)" ;;
esac
fi

# Slot reuse while the browser lives: tab C (the first host, whose job holds the browser process; started alone
# first) stays open; tab A logs in and closes; tab B then takes the lowest free profile name, A's if its folder could
# be deleted. All within the first 30 s, so no pull carries A's login into the jar (B's `seeded 0 cookies` confirms
# it). NEWTAB leaves focus on C, so NEXTTAB (tabs cycle in creation order) moves it to the new tab before
# ADDRESSBAR and CLOSETAB; closing a tab leaves nothing focused.
if wanted reuse; then
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
fi

# Ten WebView2 tabs opened and closed in turn: Local State may gain no profile names outside the pool.
if wanted ten; then
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
fi

# Per-tab jars: named jars Alice and Bob, a WebView2 tab on each signing in as its own probe user. One 30 s pull
# (the 38 s stay) and the quit pull carry each login into its tab's jar file; the default jar must get none.
# GRAPHOPEN opens the first tab and NEXTTAB the second, so both are live and both log in.
cat >"$out/perjar.py" <<'PY'
import json, os, sys
sys.path.insert(0, sys.argv[3])
from dpapi import load_jar
def user(path):
    """The probe user whose sid the jar holds ("none" without one); never a cookie value."""
    try:
        cs = load_jar(path)
    except FileNotFoundError:
        return "none(no file)"
    except Exception as e:
        return f"unreadable({type(e).__name__})"
    sids = [c.get("value", "") for c in cs if c.get("name") == "sid"]
    users = [v[len("PROBE-SESSION-"):] or "default" for v in sids if v.startswith("PROBE-SESSION")]
    return ",".join(users) if users else ("none" if not sids else "other")
nodes = json.load(open(sys.argv[1], encoding="utf-8"))["nodes"]
ids = {n.get("name"): n["id"] for n in nodes if n["kind"] == "jar"}
jars = sys.argv[2]
a = user(os.path.join(jars, ids["Alice"] + ".json")) if "Alice" in ids else "no-jar-node"
b = user(os.path.join(jars, ids["Bob"] + ".json")) if "Bob" in ids else "no-jar-node"
print(f"perjar: alice jar={a} bob jar={b} default={user(os.path.join(jars, 'default.json'))}")
PY
perjar_ok=0
if wanted perjar; then
rm -rf "$profile/manifold/jars"
seed "{\"nodes\": [
  {\"id\": \"ja\", \"kind\": \"jar\", \"name\": \"Alice\"},
  {\"id\": \"jb\", \"kind\": \"jar\", \"name\": \"Bob\"},
  {\"id\": \"a\", \"kind\": \"tab\", \"url\": \"$base/login?u=alice\", \"engine\": \"webview2\", \"jar\": \"ja\", \"jar_set\": true, \"pos\": [0.0, 0.0]},
  {\"id\": \"b\", \"kind\": \"tab\", \"url\": \"$base/login?u=bob\", \"engine\": \"webview2\", \"jar\": \"jb\", \"jar_set\": true, \"pos\": [400.0, 0.0]}
]}"
echo "--- run perjar (two WebView2 tabs on jars Alice and Bob)"
start perjar WAITTABS GRAPHRIGHT GRAPHOPEN NEXTTAB "SLEEP 38" "EXPORTSESSION $(cygpath -w "$out/s.json")"
grep -E " /login\?u=" "$out/req.log" | awk '{print "    " $2, $3, $4, $5}'
line="$(python -P "$(cygpath -w "$out/perjar.py")" "$(cygpath -w "$out/s.json")" "$(cygpath -w "$profile/manifold/jars")" "$(cygpath -w "$out")" </dev/null)"
echo "$line"
if [ "$line" = "perjar: alice jar=alice bob jar=bob default=none" ] || [ "$line" = "perjar: alice jar=alice bob jar=bob default=none(no file)" ]; then
  perjar_ok=1
  echo "perjar: PASS"
else
  echo "FAIL: each login must land in its own tab's jar, and none in the default jar"; fail=1
fi
fi

# Jar switch on a live tab: restart on perjar's profile (both tabs restored, each seeded from its own jar), focus
# Alice's tab explicitly (the quick switcher, by its url; restored focus is not relied on), ask /whoami?switch, then
# SETJAR Bob and SWITCHJARNOW: the host is replaced by one seeded from Bob's jar, so /whoami?switch2 must be Bob's.
if wanted jarswitch; then
if [ "$perjar_ok" = 1 ]; then
  n0=$(wc -l <"$out/req.log")
  echo "--- run jarswitch (Alice's tab moves to jar Bob)"
  start jarswitch WAITTABS QUICKSWITCHOPEN "QUICKSWITCHTEXT u=alice" QUICKSWITCHENTER WAITTABS "SLEEP 3" \
    "ADDRESSBAR $base/whoami?switch" "SLEEP 1" ENTER "SLEEP 3" "SETJAR Bob" SWITCHJARNOW WAITTABS "SLEEP 8" \
    "ADDRESSBAR $base/whoami?switch2" "SLEEP 1" ENTER "SLEEP 3"
  sw_log="$(tail -n +"$((n0 + 1))" "$out/req.log")"
  echo "=== jarswitch requests (engine path auth user)"; awk '{print "    " $2, $3, $4, $5}' <<<"$sw_log"
  u1="$(awk '$3 == "/whoami?switch" {print $5; exit}' <<<"$sw_log")"
  u2="$(awk '$3 == "/whoami?switch2" {print $5; exit}' <<<"$sw_log")"
  if [ "$u1" = "user=alice" ] && [ "$u2" = "user=bob" ]; then
    echo "jarswitch: whoami alice -> bob PASS"
  else
    echo "jarswitch: whoami ${u1#user=}${u1:+ }-> ${u2#user=} FAIL"; fail=1
  fi
else
  echo "jarswitch: skipped (needs a passing perjar run on this profile)"; wanted perjar || fail=1
fi
fi
echo "out=$out"
[ "$fail" = 0 ] && echo "PROBE PASS" || echo "PROBE FAIL"
exit "$fail"
