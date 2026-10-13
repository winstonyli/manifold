#!/usr/bin/env bash
# Usage: scripts/release.sh [--with-servo] [--with-webkit] [--reuse-hosts-from vX.Y.Z]
#        scripts/release.sh --verify vX.Y.Z
#
# Cuts the bare-exe release (plan docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md, Task 1). In order:
#   1. refuses a dirty tree;
#   2. builds the engine hosts ONCE, in release: WebView2 (to embed in manifold.exe) and CEF, plus Servo with
#      --with-servo and WebKit with --with-webkit (built inside WSL into target-linux/, see README). Windows hosts but Servo
#      use a static CRT (Servo's mozjs/mozangle are /MD) with depot_tools stripped from PATH, as the old package.sh did;
#   3. scripts/host_pin.py hashes the CEF/Servo/WebKit hosts and writes crates/manifold/src/host_pin.rs (WebView2 is
#      embedded, so it has no row);
#   4. builds manifold.exe with MANIFOLD_WEBVIEW2_HOST naming the built WebView2 host; crates/manifold/build.rs
#      embeds that file (and its SHA-256) in the exe;
#   5. writes dist/manifold-v<ver>-windows-x64.exe (+ .exe.sha256), dist/hosts/<file>-<platform> (the exact files to
#      upload, which the pin hashes) and the embedded host's hash under dist/, and prints the commands to commit
#      host_pin.rs, tag and upload. It never uploads, commits, tags or pushes.
# Host pins name uploaded artifacts: PE output is not bit-reproducible, so the hosts are built here once and the
# hash is taken from that file (plan Decision 7). Step 4 builds only `-p manifold`, which cannot relink a host
# (hosts do not depend on manifold); the script checks that anyway.
#
# The finished tree is dirty by exactly crates/manifold/src/host_pin.rs: commit it now, then tag; the exe was built
# from this tree.
#
# --reuse-hosts-from vX.Y.Z  (Decision 11) keep that tag's CEF/Servo/WebKit entries unchanged, pointing at its
#                            uploaded assets, and build only what it lacks. Refused unless
#                            `git diff vX.Y.Z -- crates/ipc-protocol crates/manifold-view-*` is empty and
#                            PROTOCOL_VERSION is the same at that tag.
# --verify vX.Y.Z            re-checks dist/ against that tag: its host_pin.rs hashes equal dist/hosts/*, the exe matches
#                            its .sha256, and the embedded host's recorded hash matches the saved copy.
# SKIP_BUILD=1  use the hosts already in target/ (target-servo/, target-linux/) instead of building them (a build you
#               ran yourself, e.g. at BelowNormal; build with RUSTFLAGS="-C target-feature=+crt-static"). manifold.exe
#               is always built, because it must carry the table written in step 3.
# SKIP_SERVO=1 / SKIP_WEBKIT=1  ignore --with-servo / --with-webkit.
# JOBS=N        cargo's -j (default 8, at most 8).
# CARGO_TARGET_DIR is honoured for the Windows hosts and manifold.exe (a short path avoids cef-dll-sys path limits).
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
cd "$repo_root"

pin_rs=crates/manifold/src/host_pin.rs
jobs="${JOBS:-8}"
[ "$jobs" -gt 8 ] && jobs=8
tdir="${CARGO_TARGET_DIR:-target}/release"
version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -1)"
tag="v$version"
with_servo="" with_webkit="" reuse="" verify_tag=""
while [ $# -gt 0 ]; do
    case "$1" in
        --with-servo) with_servo=1 ;;
        --with-webkit) with_webkit=1 ;;
        --reuse-hosts-from) reuse="${2:?--reuse-hosts-from needs a tag}"; shift ;;
        --verify) verify_tag="${2:?--verify needs a tag}"; shift ;;
        *) sed -n '2,3p' "$0" >&2; exit 2 ;;
    esac
    shift
done
[ -n "${SKIP_SERVO:-}" ] && with_servo=""
[ -n "${SKIP_WEBKIT:-}" ] && with_webkit=""

die() { echo "release.sh: $*" >&2; exit 1; }
wp() { cygpath -m "$1"; } # native path for python / MANIFOLD_WEBVIEW2_HOST
sha() { sha256sum "$1" | cut -d' ' -f1; }
pin() { python -P "$repo_root/scripts/host_pin.py" "$@" </dev/null; }

# run_low <cmd...>: runs it at BelowNormal and returns its exit status (`cmd /c start /belownormal` loses it).
run_low() {
    "$@" &
    local p=$! rc=0
    below_normal "$p" >/dev/null || true
    wait "$p" || rc=$?
    return $rc
}

if [ -n "$verify_tag" ]; then
    ver="${verify_tag#v}"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    git show "$verify_tag:$pin_rs" >"$tmp/pin.rs" || die "$verify_tag has no $pin_rs"
    list="$(pin --list "$(wp "$tmp/pin.rs")")"
    status=0
    ok() { echo "PASS: $1"; }
    bad() { echo "FAIL: $1"; status=1; }
    [ "$(sed -n 's/^version //p' <<<"$list")" = "$ver" ] && ok "PIN_VERSION is $ver" || bad "PIN_VERSION is not $ver"
    while read -r kind platform file size sha256 url; do
        [ "$kind" = version ] && continue
        asset="$file-$platform"
        if [ "${url#*/download/$verify_tag/}" = "$url" ]; then
            echo "SKIP: $kind $asset is reused from an older release (${url#*/download/}); not in dist/"
            continue
        fi
        f="dist/hosts/$asset"
        if [ ! -f "$f" ]; then bad "$f is missing"
        elif [ "$(sha "$f")" = "$sha256" ] && [ "$(stat -c %s "$f")" = "$size" ]; then ok "$asset matches the pin"
        else bad "$asset differs from the pin in $verify_tag"; fi
    done <<<"$list"
    exe="dist/manifold-$verify_tag-windows-x64.exe"
    if [ -f "$exe" ] && (cd dist && sha256sum -c --status "$(basename "$exe").sha256"); then ok "$(basename "$exe") matches its .sha256"
    else bad "$exe is missing or does not match its .sha256"; fi
    rec="dist/manifold-$verify_tag-windows-x64.embedded-host.sha256"
    if [ -f "$rec" ] && [ "$(cut -d' ' -f1 "$rec")" = "$(sha dist/embedded/manifold-engine-webview2.exe)" ]; then
        ok "the embedded WebView2 host copy matches the recorded hash"
    else bad "$rec is missing or does not match dist/embedded/manifold-engine-webview2.exe"; fi
    echo "NOTE: that the exe really contains those bytes is not checked here (build.rs embeds the file and hashes it at build time; bare-exe Task 7's --check runs the exe)."
    exit $status
fi

[ -z "$(git status --porcelain)" ] || die "the working tree is dirty; commit first"

# Decision 11: reuse an older release's hosts only when their sources and the protocol are unchanged.
keep=""
reused=" "
if [ -n "$reuse" ]; then
    git rev-parse -q --verify "refs/tags/$reuse" >/dev/null || die "no tag $reuse"
    git diff --quiet "$reuse" -- crates/ipc-protocol 'crates/manifold-view-*' ||
        die "crates/ipc-protocol or an engine crate changed since $reuse; the hosts cannot be reused"
    old_proto="$(git show "$reuse:crates/ipc-protocol/src/lib.rs" | sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9]*\);/\1/p')"
    new_proto="$(sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9]*\);/\1/p' crates/ipc-protocol/src/lib.rs)"
    [ -n "$old_proto" ] && [ "$old_proto" = "$new_proto" ] ||
        die "PROTOCOL_VERSION is '$new_proto' here but '$old_proto' at $reuse"
    keep="$(mktemp -d)"
    git show "$reuse:$pin_rs" >"$keep/pin.rs" || die "$reuse has no $pin_rs"
    old="$(pin --list "$(wp "$keep/pin.rs")")"
    reused=" $(awk '$1 != "version" { print $1 "/" $2 }' <<<"$old" | sort -u | tr '\n' ' ')"
    [ "$reused" != " " ] || die "$reuse pins no hosts; nothing to reuse"
    echo "reusing from $reuse:$reused"
fi
is_reused() { case "$reused" in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
for k in "servo/windows-x64:$with_servo" "webkit/linux-x64:$with_webkit"; do
    is_reused "${k%%:*}" && [ -n "${k#*:}" ] && die "${k%%:*} is reused from $reuse; drop its --with flag"
done

stage="$(mktemp -d)"
trap 'rm -rf "$stage" "$keep"' EXIT
mkdir -p "$repo_root/dist/hosts" "$repo_root/dist/embedded"

# Which hosts to build: WebView2 always (it is embedded); the rest unless reused or not asked for.
win_pkgs=(-p manifold-view-webview2)
is_reused cef/windows-x64 || win_pkgs+=(-p manifold-view-cef)
build_servo="" build_webkit=""
{ [ -n "$with_servo" ] && ! is_reused servo/windows-x64; } && build_servo=1
{ [ -n "$with_webkit" ] && ! is_reused webkit/linux-x64; } && build_webkit=1

if [ -z "${SKIP_BUILD:-}" ]; then
    # depot_tools' extensionless `ninja` shadows the real ninja.exe and breaks CEF's build (README, "Building").
    # A static CRT spares users the VC++ Redistributable. RUSTFLAGS (not .cargo/config.toml) keeps it to this build.
    (
        PATH="$(echo "$PATH" | tr ':' '\n' | grep -v -F depot_tools | tr '\n' ':')"
        export PATH RUSTFLAGS="-C target-feature=+crt-static"
        run_low cargo build --release -j "$jobs" "${win_pkgs[@]}"
        if [ -n "$build_servo" ]; then
            # About 33 minutes and several GB of disk and RAM: refuse early rather than fail at minute 30.
            free_gb="$(df -BG --output=avail . | tail -1 | tr -dc 0-9)"
            [ "$free_gb" -ge 30 ] || die "Servo's release build needs about 30 GB of disk; $free_gb GB free"
            free_mb="$(powershell.exe -NoProfile -Command '[int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1024)' </dev/null | tr -dc 0-9)"
            [ "$free_mb" -ge 8000 ] || die "Servo's release build needs about 8 GB of free memory; $free_mb MB free"
            # Not with the static CRT: mozjs and mozangle are built against the dynamic one (/MD) and the linker refuses to
            # mix them (LNK2038, found by the Task 7 dry run), so this host imports vcruntime140/msvcp140.
            (unset RUSTFLAGS; CARGO_TARGET_DIR=target-servo run_low cargo build --release -j "$jobs" -p manifold-view-servo)
        fi
    )
    if [ -n "$build_webkit" ]; then
        # Inside WSL, into target-linux/ (git-ignored). Neither the static CRT nor depot_tools applies there.
        wsl.exe -d Ubuntu -- bash -c 'cd "$(wslpath -u "$1")" && CARGO_TARGET_DIR=target-linux nice cargo build --release -j "$2" -p manifold-view-webkit' \
            _ "$(cygpath -w "$repo_root")" "$jobs" </dev/null
    fi
fi

# Collect the hosts into $stage under their file names (the pin keys on those), and fail on a missing one.
assets=()
take() { # take <kind/platform> <built path>
    [ -f "$2" ] || die "$2 is missing: build it (omit SKIP_BUILD) or drop its flag"
    cp "$2" "$stage/$(basename "$2")"
    assets+=(--asset "$1=$(wp "$stage/$(basename "$2")")")
}
win_host="$stage/manifold-engine-webview2.exe"
cp "$tdir/manifold-engine-webview2.exe" "$win_host" || die "$tdir/manifold-engine-webview2.exe is missing"
is_reused cef/windows-x64 || take cef/windows-x64 "$tdir/manifold-engine-cef.exe"
if [ -n "$build_servo" ]; then
    take servo/windows-x64 target-servo/release/manifold-engine-servo.exe
    # The exe loads ANGLE's two DLLs from beside itself (Task 0). Ask cargo where the mozangle build script that this
    # build used left them (a no-op rebuild, same environment as the Servo build): the out dirs under
    # target-servo/release/build/ pile up across rebuilds, so the newest by mtime can be a stale one.
    angle_out="$( (unset RUSTFLAGS; CARGO_TARGET_DIR=target-servo cargo build --release -j "$jobs" -p manifold-view-servo --message-format=json </dev/null) \
        | grep '"reason":"build-script-executed"' | grep 'mozangle@' | sed -n 's/.*"out_dir":"\([^"]*\)".*/\1/p' | head -1 | sed 's/\\\\/\//g')"
    [ -n "$angle_out" ] && [ -f "$angle_out/libEGL.dll" ] || die "cargo did not report a mozangle out_dir holding libEGL.dll (got '$angle_out')"
    angle_dir="$(cygpath -u "$angle_out")"
    take servo/windows-x64 "$angle_dir/libEGL.dll"
    take servo/windows-x64 "$angle_dir/libGLESv2.dll"
fi
[ -n "$build_webkit" ] && take webkit/linux-x64 target-linux/release/manifold-engine-webkit

keep_args=()
[ -n "$keep" ] && keep_args=(--keep-from "$(wp "$keep/pin.rs")")
table_sha="$(pin --version "$version" "${keep_args[@]}" "${assets[@]}")"

# Build manifold.exe after the table is written, with the WebView2 host to embed (crates/manifold/build.rs).
(
    PATH="$(echo "$PATH" | tr ':' '\n' | grep -v -F depot_tools | tr '\n' ':')"
    export PATH RUSTFLAGS="-C target-feature=+crt-static"
    export MANIFOLD_WEBVIEW2_HOST
    MANIFOLD_WEBVIEW2_HOST="$(wp "$win_host")"
    run_low cargo build --release -j "$jobs" -p manifold
)

# The hosts must be the files that were hashed: building manifold.exe cannot have relinked them.
for f in "${assets[@]}"; do
    case "$f" in --asset) continue ;; esac
    built="${f#*=}"
    case "$built" in *manifold-engine-webkit) src=target-linux/release/manifold-engine-webkit ;;
        *manifold-engine-servo.exe) src=target-servo/release/manifold-engine-servo.exe ;;
        *libEGL.dll | *libGLESv2.dll) src="$angle_dir/$(basename "$built")" ;;
        *) src="$tdir/$(basename "$built")" ;;
    esac
    [ "$(sha "$src")" = "$(sha "$built")" ] || die "$src changed while manifold.exe was built"
done
[ "$(sha "$tdir/manifold-engine-webview2.exe")" = "$(sha "$win_host")" ] || die "the WebView2 host changed while manifold.exe was built"

# dist/: what to upload, and what --verify checks.
exe="dist/manifold-$tag-windows-x64.exe"
cp "$tdir/manifold.exe" "$exe"
(cd dist && sha256sum "$(basename "$exe")" >"$(basename "$exe").sha256")
cp "$win_host" dist/embedded/manifold-engine-webview2.exe
echo "$(sha "$win_host")  manifold-engine-webview2.exe" >"dist/manifold-$tag-windows-x64.embedded-host.sha256"
listing="$(pin --list "$(wp "$pin_rs")")"
while read -r kind platform file _size _sha url; do
    [ "$kind" = version ] && continue
    [ "${url#*/download/$tag/}" = "$url" ] && continue # reused: already uploaded under the older tag
    cp "$stage/$file" "dist/hosts/$file-$platform"
done <<<"$listing"

echo
echo "built $exe ($(stat -c %s "$exe") bytes, sha256 $(sha "$exe"))"
echo "host table sha256 (for the release notes): $table_sha"
echo "embedded WebView2 host sha256: $(sha "$win_host")"
echo
echo "The tree is now dirty by exactly $pin_rs. Commit it now, then tag; the exe was built from this tree:"
echo "  git add $pin_rs && git commit -m 'host pins for $tag'"
echo "  git tag -a $tag -m '$tag' && scripts/release.sh --verify $tag"
echo "  (after you push the tag and create the release:)"
echo "  gh release create $tag $exe dist/manifold-$tag-windows-x64.exe.sha256 dist/hosts/* --title '$tag'"
