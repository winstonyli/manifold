#!/usr/bin/env bash
# Usage: [SIZES="400 300,800 600,1920 1080"] [VIEWS="graph shortcuts ..."] scripts/chrome-shots-sizes.sh
#
# scripts/chrome-shots.sh at several window sizes, to see how the chrome copes with a small or a large window:
# <OUT>/<W>x<H>/<view>.png for each size (OUT defaults to target/shots-sizes), plus the usual sheet.png in each.
# VIEWS defaults to the ones that need no click on a fixed coordinate (those assume a 1280x800 window), so they hold
# at any size; `gate layout` checks the popups' bounds and layering by number, this is for looking.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
sizes="${SIZES:-400 300,800 600,1920 1080}"
views="${VIEWS:-graph graph-menu-tab graph-newnode-menu shortcuts quickswitch settings tabview}"
out="${OUT:-$repo_root/target/shots-sizes}"
status=0
IFS=',' read -ra list <<<"$sizes"
for size in "${list[@]}"; do
    dir="$out/${size// /x}"
    echo "== $size -> $dir"
    SIZE="$size" VIEWS="$views" OUT="$dir" bash "$repo_root/scripts/chrome-shots.sh" </dev/null || status=1
done
exit "$status"
