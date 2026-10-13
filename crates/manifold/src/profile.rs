//! `%APPDATA%\manifold`, the one folder the app keeps its files in:
//! `settings.json`, `bookmarks.json`, `history.json`, `tab-history.json`, the session store,
//! the named sessions, fetched content (`content_dir`) and CEF's throwaway
//! cache dirs, under `cef-cache`. Every path into it starts from `dir()`
//! or, for engines, `view_data_dir()`. The one user folder outside it,
//! the screenshot folder, comes from `pictures_dir()`.

use std::path::PathBuf;

/// `None` when there is no `%APPDATA%`, and always under `cargo test`: a
/// test must never read or write the real profile, and loading a corrupt
/// file there renames it aside. Every caller already treats `None` as
/// "nothing saved", so tests start from defaults. A test that needs files
/// uses `store::testutil::temp_dir` and passes the path in.
///
/// `--profile-dir <dir>` or `--profile-dir=<dir>` names the folder itself
/// instead (for a run that can't set `APPDATA`, like one started by the
/// elevated `Samply` task). Read once per run.
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        dir_from(&args, std::env::var_os("APPDATA"))
    })
    .clone()
}

fn dir_from(args: &[String], appdata: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let flag = args.iter().enumerate().find_map(|(i, a)| match a.strip_prefix("--profile-dir") {
        Some("") => args.get(i + 1).cloned(),
        Some(rest) => rest.strip_prefix('=').map(str::to_string),
        None => None,
    });
    match flag {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(PathBuf::from(appdata?).join("manifold")),
    }
}

/// The user's Pictures folder, where the toolbar's Screenshot saves. `None`
/// without a home folder, and always under `cargo test`, like `dir()`.
pub fn pictures_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join("Pictures"))
}

/// Where CEF engine-hosts keep their browser data: one throwaway
/// subdirectory per engine process (see `process_manager::spawn_tab`).
/// `None` like `dir()`.
pub fn cef_cache_dir() -> Option<PathBuf> {
    Some(dir()?.join("cef-cache"))
}

/// Where `FaviconFetcher` keeps each origin's favicon between launches.
/// `None` like `dir()`.
pub fn favicon_cache_dir() -> Option<PathBuf> {
    Some(dir()?.join("favicons"))
}

/// Where content fetched from the web lives: each file is named
/// `<node id>.<ext>` (see `content`). `None` like `dir()`.
pub fn content_dir() -> Option<PathBuf> {
    Some(dir()?.join("content"))
}

/// Where cookie jars live (`jar::Jar`). `None` like `dir()`.
pub fn jars_dir() -> Option<PathBuf> {
    Some(dir()?.join("jars"))
}

/// The per-user data directory (Known Folders on Windows, which puts it at
/// `%APPDATA%\manifold\manifold\data`; XDG on Linux; Application Support on
/// macOS) where `process_manager::view_binary_path` looks for an engine
/// that isn't next to `manifold.exe`. `None` with no home directory, and
/// always under `cargo test`, like `dir()`.
pub fn view_data_dir() -> Option<PathBuf> {
    use etcetera::{app_strategy::AppStrategyArgs, choose_app_strategy, AppStrategy};
    if cfg!(test) {
        return None;
    }
    let args =
        AppStrategyArgs { top_level_domain: "io".into(), author: "manifold".into(), app_name: "manifold".into() };
    Some(choose_app_strategy(args).ok()?.data_dir())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn tests_have_no_profile_dir() {
        assert_eq!(super::dir(), None);
        assert_eq!(super::view_data_dir(), None);
        assert_eq!(super::cef_cache_dir(), None);
        assert_eq!(super::content_dir(), None);
        assert_eq!(super::jars_dir(), None);
        assert_eq!(super::pictures_dir(), None);
    }

    #[test]
    fn profile_dir_accepts_the_equals_form() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let appdata = Some(std::ffi::OsString::from("A"));
        assert_eq!(super::dir_from(&args(&["m", "--profile-dir=P"]), appdata.clone()), Some("P".into()));
        // `cli::validate` refuses this before `profile::dir` is read (M4); `dir_from` alone still ignores it.
        assert_eq!(
            super::dir_from(&args(&["m", "--profile-directory", "X"]), appdata),
            Some(Path::new("A").join("manifold")),
            "only the exact flag"
        );
    }

    #[test]
    fn profile_dir_flag_beats_appdata() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let appdata = Some(std::ffi::OsString::from("A"));
        assert_eq!(super::dir_from(&args(&["m", "--profile-dir", "P"]), appdata.clone()), Some("P".into()));
        assert_eq!(super::dir_from(&args(&["m", "--profile-dir", "P"]), None), Some("P".into()));
        assert_eq!(super::dir_from(&args(&["m"]), appdata.clone()), Some(Path::new("A").join("manifold")));
        // `cli::validate` refuses a bare `--profile-dir` first (M4); this is `dir_from`'s own fallback.
        assert_eq!(super::dir_from(&args(&["m", "--profile-dir"]), appdata), Some(Path::new("A").join("manifold")));
        assert_eq!(super::dir_from(&args(&["m"]), None), None);
    }

    /// `App::default()` loads settings, bookmarks and history. Under test
    /// it must reach no real profile file, which holds as long as no other
    /// source file looks up `%APPDATA%` or an `etcetera` directory itself.
    #[test]
    fn app_default_touches_no_profile_file() {
        let app = crate::App::default();
        assert_eq!(app.settings_fingerprint, None);

        let mut offenders = Vec::new();
        let needles =
            ["\"APPDATA\"", "\"LOCALAPPDATA\"", "\"USERPROFILE\"", "\"HOME\"", "etcetera", "dirs::", "known_folder"];
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
        assert_eq!(offenders, Vec::<String>::new(), "look up the profile folder with profile::dir()");
    }

    fn scan(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, needles, offenders);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("profile.rs")
                && needles.iter().any(|needle| std::fs::read_to_string(&path).unwrap().contains(needle))
            {
                offenders.push(path.display().to_string());
            }
        }
    }
}
