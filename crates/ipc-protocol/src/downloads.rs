//! Where engines put downloads and how they name them: one rule for every
//! engine, so a download's `path` (the host's key for it) is the same string
//! in every message about it.

use std::path::{Path, PathBuf};

use crate::EngineToHost;

/// `~/Downloads`: `USERPROFILE` on Windows, `HOME` elsewhere (CEF on Linux
/// used `USERPROFILE` and so saved into its working directory).
pub fn downloads_dir() -> PathBuf {
    let home = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") };
    home.map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_default()
}

/// A server-suggested file name made safe to create in the downloads folder,
/// on every platform alike (one rule for every engine):
/// - only its last component, split on `/` and `\` both, so `..` or an
///   absolute name can't leave the folder;
/// - control characters and the ones Windows refuses (`<>:"/\|?*`) become
///   `_`, so a `:` can't name an alternate data stream (`a.txt:x`);
/// - trailing dots and spaces dropped (Windows drops them itself, and the
///   file would then not be at the path this returns);
/// - a Windows device name (`is_windows_device_name`) gets a leading `_`;
/// - "download" if nothing is left, or only `.`/`..`.
pub fn safe_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String =
        last.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let name = cleaned.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        "download".to_string()
    } else if is_windows_device_name(name) {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, or `COM`/`LPT` with a
/// digit or superscript digit, in any case, judged on the part before the
/// first dot with trailing spaces dropped (`nul.txt` and `CON ` open the
/// device too). Shared with manifold's session names (`session_name_is_safe`).
pub fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(rest.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}'))
}

/// Extensions of two parts, kept whole when numbering a taken name.
const DOUBLE_EXTENSIONS: [&str; 4] = [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"];

/// `name` split before its extension: a `DOUBLE_EXTENSIONS` one (any case)
/// whole, otherwise the last, as `Path::extension` reads it (`.bashrc` has
/// none). The extension is returned without its leading dot.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = DOUBLE_EXTENSIONS.iter().find(|ext| lower.len() > ext.len() && lower.ends_with(*ext)) {
        let at = name.len() - ext.len();
        return (&name[..at], Some(&name[at + 1..]));
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    (stem, as_path.extension().and_then(|e| e.to_str()))
}

/// `dir/name`, or `dir/stem (n).ext` with the first n that isn't `taken`
/// (an existing file, or one another download in flight has reserved).
/// `a.tar.gz` numbers as `a (1).tar.gz` (`split_extension`).
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = split_extension(name);
    (1..)
        .map(|n| {
            dir.join(match ext {
                Some(ext) => format!("{stem} ({n}).{ext}"),
                None => format!("{stem} ({n})"),
            })
        })
        .find(|p| !taken(p))
        .expect("an unbounded range finds a free name")
}

/// One tick's engine messages with each download's progress reduced to the
/// latest, kept at its last position; every other message keeps its order.
/// Fast downloads report per network chunk, hundreds of times a tick.
pub fn coalesce_progress(msgs: Vec<EngineToHost>) -> Vec<EngineToHost> {
    let last_progress: std::collections::HashMap<String, usize> = msgs
        .iter()
        .enumerate()
        .filter_map(|(i, m)| match m {
            EngineToHost::DownloadProgress { path, .. } => Some((path.clone(), i)),
            _ => None,
        })
        .collect();
    msgs.into_iter()
        .enumerate()
        .filter(|(i, m)| match m {
            EngineToHost::DownloadProgress { path, .. } => last_progress.get(path) == Some(i),
            _ => true,
        })
        .map(|(_, m)| m)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_suggested_name_keeps_only_its_last_component() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name("report #3.pdf"), "report #3.pdf");
        assert_eq!(safe_name("..\\..\\Windows\\win.ini"), "win.ini", "a backslash splits on every platform");
        assert_eq!(safe_name("C:\\x\\a.txt"), "a.txt");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name("dir/"), "download");
    }

    #[test]
    fn a_suggested_name_loses_what_windows_refuses() {
        for (raw, want) in [
            ("a.txt:evil", "a.txt_evil"),
            ("C:a.txt", "C_a.txt"),
            ("a<b>c\"d|e?f*g", "a_b_c_d_e_f_g"),
            ("tab\there\nnl\u{0}nul\u{7f}", "tab_here_nl_nul_"),
            ("name. . .", "name"),
            ("trailing ", "trailing"),
            ("...", "download"),
            (" ", "download"),
            ("名前.pdf", "名前.pdf"),
        ] {
            assert_eq!(safe_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn a_device_name_is_prefixed() {
        for (raw, want) in [
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("Nul.tar.gz", "_Nul.tar.gz"),
            ("aux", "_aux"),
            ("PRN.", "_PRN"),
            ("COM1.log", "_COM1.log"),
            ("lpt9", "_lpt9"),
            ("COM\u{b9}", "_COM\u{b9}"),
            ("CON .txt", "_CON .txt"),
            ("conout$", "_conout$"),
            ("LPT0", "_LPT0"),
        ] {
            assert_eq!(safe_name(raw), want, "{raw:?}");
        }
        for ok in ["console.txt", "COM10", "a.con", "CONIN", "nul-ish"] {
            assert_eq!(safe_name(ok), ok);
        }
    }

    #[test]
    fn a_taken_archive_keeps_its_double_extension() {
        let dir = Path::new("d");
        let all = |p: &Path| p.file_name().is_some_and(|n| !n.to_string_lossy().contains("(1)"));
        assert_eq!(unique_path(dir, "a.tar.gz", all), dir.join("a (1).tar.gz"));
        assert_eq!(unique_path(dir, "b.TAR.XZ", all), dir.join("b (1).TAR.XZ"));
        assert_eq!(unique_path(dir, "c.tar.bz2", all), dir.join("c (1).tar.bz2"));
        assert_eq!(unique_path(dir, "d.tar.zst", all), dir.join("d (1).tar.zst"));
        assert_eq!(unique_path(dir, "e.v1.zip", all), dir.join("e.v1 (1).zip"), "otherwise only the last");
        assert_eq!(unique_path(dir, "f.gz", all), dir.join("f (1).gz"));
        assert_eq!(unique_path(dir, ".tar.gz", all), dir.join(".tar (1).gz"), "no stem: the last only");
        assert_eq!(unique_path(dir, ".bashrc", all), dir.join(".bashrc (1)"));
    }

    #[test]
    fn a_taken_name_gets_a_number_before_its_extension() {
        let dir = Path::new("d");
        let taken = |p: &Path| p == dir.join("a.pdf") || p == dir.join("a (1).pdf");
        assert_eq!(unique_path(dir, "a.pdf", taken), dir.join("a (2).pdf"));
        assert_eq!(unique_path(dir, "b.pdf", taken), dir.join("b.pdf"));
        assert_eq!(unique_path(dir, "a", |p: &Path| p == dir.join("a")), dir.join("a (1)"));
    }

    #[test]
    fn progress_is_coalesced_to_the_latest_per_path_in_order() {
        use crate::EngineToHost::*;
        let path = |p: &str| p.to_string();
        let msgs = vec![
            DownloadStarted { path: path("a"), total_bytes: None, url: None },
            DownloadProgress { path: path("a"), bytes_received: 1 },
            DownloadProgress { path: path("b"), bytes_received: 5 },
            DownloadProgress { path: path("a"), bytes_received: 2 },
            DownloadCompleted { path: path("b") },
        ];
        let out = coalesce_progress(msgs);
        let shown: Vec<String> = out.iter().map(|m| format!("{m:?}")).collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        assert!(shown[1].contains("\"b\"") && shown[1].contains('5'), "b's progress before its completion");
        assert!(shown[2].contains("\"a\"") && shown[2].contains('2'), "a's latest, at its last place: {shown:?}");
        assert!(shown[3].starts_with("DownloadCompleted") && shown[3].contains("\"b\""), "{shown:?}");
    }
}
