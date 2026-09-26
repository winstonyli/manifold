//! `%APPDATA%\manifold`, the one folder the app keeps its files in:
//! `settings.json`, `bookmarks.json`, `history.json`, the session store
//! and the named sessions. Every path into it starts from `dir()`.

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

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn tests_have_no_profile_dir() {
        assert_eq!(super::dir(), None);
    }

    /// `App::default()` loads settings, bookmarks and history. Under test
    /// it must reach no real profile file, which holds as long as no other
    /// source file looks up `%APPDATA%` itself.
    #[test]
    fn app_default_touches_no_profile_file() {
        let app = crate::App::default();
        assert_eq!(app.settings_fingerprint, None);

        let needle = ["APP", "DATA\""].concat();
        let mut offenders = Vec::new();
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needle, &mut offenders);
        assert_eq!(offenders, Vec::<String>::new(), "look up the profile folder with profile::dir()");
    }

    fn scan(dir: &Path, needle: &str, offenders: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, needle, offenders);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("profile.rs")
                && std::fs::read_to_string(&path).unwrap().contains(needle)
            {
                offenders.push(path.display().to_string());
            }
        }
    }
}
