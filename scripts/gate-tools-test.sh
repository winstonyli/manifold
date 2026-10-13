#!/usr/bin/env bash
# Usage: scripts/gate-tools-test.sh
# Checks gate-tier.sh and fresh-exe.sh (the timing join is a unit test in crates/gate/src/timing.rs) in about a second, without the app or git.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# check NAME WANT GOT: a PASS or FAIL line; a FAIL sets status=1.
check() {
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
status=0

# gate-tier --stdin: the strictest path wins; a ui-listed file that imports a core module is demoted.
mkdir -p "$t/src"
echo 'use egui;' >"$t/src/theme.rs"
echo 'use crate::store::model::X;' >"$t/src/zoom_chip.rs"
s="$t/src"
tier() { TIER_SRC="$s" bash "$repo_root/scripts/gate-tier.sh" --stdin | tail -1; }
check "docs only is ui" "TIER ui" "$(printf '%s\n' README.md docs/x.md | tier)"
check "a ui-listed file with no core import is ui" "TIER ui" "$(printf '%s\n' "$s/theme.rs" | tier)"
check "a ui-listed file that imports store is light" "TIER light" "$(printf '%s\n' "$s/zoom_chip.rs" | tier)"
check "an unlisted source file is light" "TIER light" "$(printf '%s\n' "$s/main.rs" | tier)"
check "the store is full" "TIER full" "$(printf '%s\n' "$s/store/journal.rs" | tier)"
check "view_install/tests.rs is full" "TIER full" "$(printf '%s\n' "$s/view_install/tests.rs" | tier)"
check "process_manager/tests.rs is full" "TIER full" "$(printf '%s\n' "$s/process_manager/tests.rs" | tier)"
check "jar/tests.rs is full" "TIER full" "$(printf '%s\n' "$s/jar/tests.rs" | tier)"
check "an unlisted file's tests.rs is light" "TIER light" "$(printf '%s\n' "$s/link_gestures/tests.rs" | tier)"
check "the strictest path wins" "TIER full" "$(printf '%s\n' README.md "$s/main.rs" crates/ipc-protocol/src/lib.rs | tier)"
check "an empty list is refused" "3" "$(printf '' | TIER_SRC="$s" bash "$repo_root/scripts/gate-tier.sh" --stdin >/dev/null 2>&1; echo $?)"
check "a file in a layer directory is read by its module name (model/jar.rs is full)" "TIER full" "$(printf '%s\n' "$s/model/jar.rs" | tier)"
check "a layer directory's test dir follows its module (model/jar/tests.rs is full)" "TIER full" "$(printf '%s\n' "$s/model/jar/tests.rs" | tier)"
check "a ui-listed file in a layer directory is ui (view/theme.rs)" "TIER ui" "$(printf '%s\n' "$s/view/theme.rs" | tier)"
check "an unlisted file in a layer directory is light (io/profile.rs)" "TIER light" "$(printf '%s\n' "$s/io/profile.rs" | tier)"
# require_fresh_exe: judges manifold.exe, the WebView2 host and the PDF view beside it, each against its own dep-info.
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
mkdir -p "$t/bin"
touch -d '2026-01-01' "$t/bin/src.rs"
src="$(cygpath -m "$t/bin/src.rs")"
for n in manifold manifold-engine-webview2 manifold-view-pdfium; do
    echo "$(cygpath -m "$t/bin/$n.exe"): $src" >"$t/bin/$n.d"
    touch -d '2026-01-02' "$t/bin/$n.exe"
done
fresh() { require_fresh_exe "$t/bin/manifold.exe" 2>/dev/null; echo $?; }
check "all exes fresh passes" "0" "$(fresh)"
touch -d '2026-01-03' "$t/bin/src.rs"
touch -d '2026-01-04' "$t/bin/manifold.exe" "$t/bin/manifold-view-pdfium.exe"
check "only the webview2 host stale fails" "1" "$(fresh)"
check "the failure names the host and says to build both" "2" "$(require_fresh_exe "$t/bin/manifold.exe" 2>&1 >/dev/null | grep -c 'manifold-engine-webview2.exe is older\|-p manifold -p manifold-view-webview2')"
touch -d '2026-01-04' "$t/bin/manifold-engine-webview2.exe"
touch -d '2026-01-02' "$t/bin/manifold-view-pdfium.exe"
check "only the PDF view stale fails" "1" "$(fresh)"
check "the failure names the PDF view" "1" "$(require_fresh_exe "$t/bin/manifold.exe" 2>&1 >/dev/null | grep -c 'manifold-view-pdfium.exe is older')"
rm "$t/bin/manifold-engine-webview2.exe" "$t/bin/manifold-view-pdfium.exe"
check "a missing host is skipped" "0" "$(fresh)"
touch -d '2026-01-02' "$t/bin/manifold.exe"
check "only manifold.exe stale fails" "1" "$(fresh)"
exit $status
