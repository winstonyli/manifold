# Sourced by drive.sh, most probes that run
# target/debug/manifold.exe (`grep -l fresh-exe.sh scripts/*.sh` lists them).
#
# require_fresh_exe <path to manifold.exe>: fails, naming the files, if any
# source the exe was built from (cargo's dep-info, manifold.d beside it) is
# newer than the exe; the same for the WebView2 host and the PDF view beside it
# (their own .d files), which `cargo build -p manifold` does not build, so build
# all. `cargo check`/`test`/`clippy` don't rebuild it, so a probe could
# otherwise screenshot an old build. ALLOW_STALE=1 skips this. The check is
# `gate fresh` (crates/gate/src/fresh.rs): a Git Bash fork costs seconds on this
# machine, so the dep-info walk runs natively.
_fresh_exe_gate="$(dirname "${BASH_SOURCE[0]}")/../target/debug/gate.exe"
require_fresh_exe() {
    [ -n "${ALLOW_STALE:-}" ] && return 0
    if [ ! -x "$_fresh_exe_gate" ]; then
        echo "target/debug/gate.exe not found: cargo build -j 8 -p gate (or set ALLOW_STALE=1)." >&2
        return 1
    fi
    "$_fresh_exe_gate" fresh "$1"
}
