//! `%APPDATA%\manifold`, the one folder the app keeps its files in:
//! `settings.json`, `bookmarks.json`, `history.json`, the session store,
//! the named sessions, fetched content (`content_dir`) and CEF's throwaway
//! cache dirs, under `cef-cache`. Every path into it starts from `dir()`
//! or, for engines, `engine_data_dir()`.

use std::path::PathBuf;

/// `None` when there is no `%APPDATA%`, and always under `cargo test`: a
/// test must never read or write the real profile, and loading a corrupt
/// file there renames it aside. Every caller already treats `None` as
/// "nothing saved", so tests start from defaults. A test that needs files
/// uses `store::testutil::temp_dir` and passes the path in.
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("manifold"))
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
#[cfg_attr(not(test), allow(dead_code))] // Used from Task 3 on (a fetch saves here); removed there.
pub fn content_dir() -> Option<PathBuf> {
    Some(dir()?.join("content"))
}

/// The per-user data directory (Known Folders on Windows, which puts it at
/// `%APPDATA%\manifold\manifold\data`; XDG on Linux; Application Support on
/// macOS) where `process_manager::engine_binary_path` looks for an engine
/// that isn't next to `manifold.exe`. `None` with no home directory, and
/// always under `cargo test`, like `dir()`.
pub fn engine_data_dir() -> Option<PathBuf> {
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
        assert_eq!(super::engine_data_dir(), None);
        assert_eq!(super::cef_cache_dir(), None);
        assert_eq!(super::content_dir(), None);
    }

    /// `App::default()` loads settings, bookmarks and history. Under test
    /// it must reach no real profile file, which holds as long as no other
    /// source file looks up `%APPDATA%` or an `etcetera` directory itself.
    #[test]
    fn app_default_touches_no_profile_file() {
        let app = crate::App::default();
        assert_eq!(app.settings_fingerprint, None);

        let mut offenders = Vec::new();
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &["\"APPDATA\"", "etcetera"], &mut offenders);
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
