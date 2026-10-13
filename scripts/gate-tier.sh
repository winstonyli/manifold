#!/usr/bin/env bash
# Usage: scripts/gate-tier.sh [BASE]     the tier to gate the changes since BASE (default: merge-base with main)
#        scripts/gate-tier.sh --stdin    classify the paths on stdin, one per line (what gate-tools-test.sh uses)
# Prints one line per changed path, strictest first, then `TIER ui|light|full`. It only advises: nothing here runs or
# changes the gate. The strictest tier of any path wins. Changed means tracked files that differ from BASE in the
# working tree, plus untracked files. On `main` with everything committed the default base is HEAD and nothing differs:
# pass the commit the last gate ran at. Edit the lists below when a module changes its role.
set -u
cd "$(dirname "$0")/.." || exit 1
src="${TIER_SRC:-crates/manifold/src}"
# Modules a UI-only change may touch: drawing, input and view state, none of which write the store or reach an engine.
# A file here that imports a core module is demoted to light (classify checks the file itself).
ui_files=" bookmarks click_count failure_toasts graph_layout history image_view input json_level_ui json_view pdf_render rgba shortcuts theme view_modes zoom_chip "
core_import='crate::(store|session_sync|mirror|session|process_manager|net)\b'
# Layer directories (view/, app/, ...) group module files without changing a module's name (`#[path = "model/jar.rs"] mod jar;`):
# classify reads a file in one as if it sat directly under the source folder. None exists yet; the spike is in README.
layers=" view app model network engine io "

rank() { case $1 in ui) echo 1 ;; light) echo 2 ;; full) echo 3 ;; esac; }

# classify PATH: "<tier> <reason>"
classify() {
    local b rel first
    rel=${1#"$src"/}; first=${rel%%/*}
    if [ "$rel" != "$1" ] && [ "$rel" != "$first" ] && [[ "$layers" == *" $first "* ]]; then set -- "$src/${rel#*/}"; fi
    case "$1" in
    Cargo.toml | Cargo.lock | .cargo/* | rust-toolchain* | crates/*/Cargo.toml | crates/ipc-protocol/* | crates/manifold-view-*/*)
        echo "full dependency, protocol or engine crate" ;;
    "$src"/session_sync.rs | "$src"/session_sync/* | "$src"/store/* | "$src"/mirror.rs | "$src"/mirror/* | "$src"/session.rs | "$src"/session/* | "$src"/local_state.rs | "$src"/local_state/* | \
        "$src"/process_manager.rs | "$src"/process_manager/* | "$src"/net.rs | "$src"/net/* | "$src"/view_install.rs | "$src"/view_install/* | "$src"/redirect_fetch.rs | "$src"/redirect_fetch/* | "$src"/favicon_fetch.rs | "$src"/favicon_fetch/* | "$src"/jar.rs | "$src"/jar/*)
        echo "full sync, store, mirror, session, engine or network core" ;;
    scripts/gate*.sh | crates/gate/* | scripts/fresh-exe.sh | scripts/winpid.sh)
        echo "full the gate's own scripts and their helpers" ;;
    "$src"/*/*) echo "light a module directory on no list" ;;
    "$src"/*.rs)
        b=$(basename "$1" .rs)
        case "$ui_files" in
        *" $b "*)
            if [ -f "$1" ] && grep -qE "$core_import" "$1"; then echo "light $b is on the ui list but imports a core module"; else echo "ui $b: drawing or input, no core import"; fi
            return ;;
        esac
        echo "light $b is on neither the ui nor the full list" ;;
    scripts/* | .githooks/* | .github/*) echo "light script or tooling" ;;
    *.md | docs/* | .superpowers/* | .gitignore | .gitattributes | rustfmt.toml | LICENSE*) echo "ui documentation or config only (nothing to test)" ;;
    *) echo "light path on no list" ;;
    esac
}

paths() {
    if [ "${1:-}" = "--stdin" ]; then tr '\n' '\0'; return; fi
    { git diff --name-only -z "$base" --; git ls-files -z --others --exclude-standard; }
}

base=""
if [ "${1:-}" != "--stdin" ]; then base="${1:-$(git merge-base HEAD main)}"; fi
best=0 tier=ui n=0 lines=""
while IFS= read -r -d '' p; do
    read -r t reason <<<"$(classify "$p")"
    n=$((n + 1))
    lines+="$(rank "$t") $t $p ($reason)"$'\n'
    if [ "$(rank "$t")" -gt "$best" ]; then best=$(rank "$t"); tier=$t; fi
done < <(paths "$@")
if [ "$n" -eq 0 ]; then
    echo "no changed files since ${base:-stdin}; pass the commit the last gate ran at (git log) as BASE" >&2
    exit 3
fi
printf '%s' "$lines" | sort -r | cut -d' ' -f2-
echo "TIER $tier"
