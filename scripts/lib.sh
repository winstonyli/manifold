# Helpers the probe scripts share. Source after winpid.sh:
#   # shellcheck source=lib.sh
#   . "$repo_root/scripts/lib.sh"
# `bash scripts/lib-test.sh` checks every helper without the app.

# seed_settings PROFILE [ENGINE] [MORE]: PROFILE/manifold/settings.json that skips the first-launch demo and
# starts tabs on ENGINE (default webview2). MORE is extra JSON members, starting with ", ".
seed_settings() {
    mkdir -p "$1/manifold"
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"${2:-webview2}\"${3:-}}" >"$1/manifold/settings.json"
}
