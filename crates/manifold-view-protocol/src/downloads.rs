//! Where engines put downloads and how they name them: the folder the app set (`folder`), and one rule for every
//! engine (`destination`), so a download's `path` (the host's key for it) is the same string in every message about
//! it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::ViewToHost;

/// The folder the app last set (`host_env::apply`), read for each new download.
static FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets `folder` (`host_env::apply`): a download already running keeps its path.
pub(crate) fn set_folder(dir: Option<&str>) {
    *FOLDER.lock().unwrap_or_else(PoisonError::into_inner) = dir.map(PathBuf::from);
}

/// The folder this host saves its next download in, as the app last set it (`host_env::apply`); `None` until then,
/// and when the app has none to give.
pub fn folder() -> Option<PathBuf> {
    FOLDER.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A server-suggested file name made safe to create in the downloads folder,
/// on every platform alike (one rule for every engine):
/// - only its last component, split on `/` and `\` both, so `..` or an
///   absolute name can't leave the folder;
/// - control characters and the ones Windows refuses (`<>:"/\|?*`) become
///   `_`, so a `:` can't name an alternate data stream (`a.txt:x`);
/// - trailing dots and spaces dropped (Windows drops them itself, and the
///   file would then not be at the path this returns);
/// - at most `MAX_NAME_UNITS` UTF-16 units, cut from the stem so the
///   extension stays (`cap_length`);
/// - a Windows device name (`is_windows_device_name`) gets a leading `_`, within that length;
/// - "download" if nothing is left, or only `.`/`..`.
pub fn safe_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String =
        last.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let capped = cap_length(cleaned.trim_end_matches(['.', ' ']));
    if capped.is_empty() {
        "download".to_string()
    } else if is_windows_device_name(&capped) {
        // Cut again with the `_` in place, so the prefix can't push it past the cap.
        cap_length(&format!("_{capped}"))
    } else {
        capped
    }
}

/// The longest name `safe_name` returns, in UTF-16 units: NTFS and most
/// other file systems take 255 per component, less room for the ` (n)`
/// `unique_path` may add. One rule on every platform, like the rest.
const MAX_NAME_UNITS: usize = 255 - 15;

/// An extension longer than this (in UTF-16 units) isn't kept whole when a
/// name is cut: it's part of a long name, not a file type.
const MAX_KEPT_EXTENSION_UNITS: usize = 32;

/// `name` cut to `MAX_NAME_UNITS` UTF-16 units: its stem is cut and its
/// extension (`split_extension`) kept, unless that is itself too long, when
/// the whole name is cut. Trailing dots and spaces the cut exposes go too.
fn cap_length(name: &str) -> String {
    let units = |s: &str| s.encode_utf16().count();
    if units(name) <= MAX_NAME_UNITS {
        return name.to_string();
    }
    let (stem, ext) = split_extension(name);
    let tail = ext.map(|e| format!(".{e}")).filter(|t| units(t) <= MAX_KEPT_EXTENSION_UNITS).unwrap_or_default();
    let head = if tail.is_empty() { name } else { stem };
    let mut budget = MAX_NAME_UNITS - units(&tail);
    let cut: String = head
        .chars()
        .take_while(|c| {
            let fits = c.len_utf16() <= budget;
            budget = budget.saturating_sub(c.len_utf16());
            fits
        })
        .collect();
    format!("{}{tail}", cut.trim_end_matches(['.', ' ']))
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

/// Whether two download paths name the same file, for `unique_path`'s
/// `taken` check against downloads in flight: ignoring case on Windows, whose
/// file systems do (so `A.pdf` in flight takes `a.pdf`, as `exists()` would
/// once it's written), exactly elsewhere. Compared as NTFS does: each
/// character uppercased on its own (`ntfs_upper`).
pub fn same_download_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        Path::new(&ntfs_upper(&a.to_string_lossy())) == Path::new(&ntfs_upper(&b.to_string_lossy()))
    } else {
        a == b
    }
}

/// Where a download the server suggested as `suggested` is saved in `dir`: `unique_path` of its `safe_name`, the
/// folder made first if missing. `Err` is the path it would have had when there is no folder, it is not absolute, or
/// it cannot be made: the host then refuses the download (`refused`) rather than saving it anywhere else.
pub fn destination(dir: Option<&Path>, suggested: &str, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, PathBuf> {
    let name = safe_name(suggested);
    let Some(dir) = dir.filter(|dir| dir.is_absolute()) else {
        return Err(dir.map_or_else(|| PathBuf::from(&name), |dir| dir.join(&name)));
    };
    std::fs::create_dir_all(dir).map_err(|_| dir.join(&name))?;
    Ok(unique_path(dir, &name, taken))
}

/// The messages for a download the host refused (`destination`'s `Err`): it starts and fails at once, so the app
/// shows a failed card.
pub fn refused(path: String, url: Option<String>) -> [ViewToHost; 2] {
    [ViewToHost::DownloadStarted { path: path.clone(), total_bytes: None, url }, ViewToHost::DownloadFailed { path }]
}

/// Close to NTFS's case folding: it uppercases through a one-to-one table, so
/// a character whose Unicode uppercase is several (`ß` -> `SS`) stays as it
/// is, and `ß.txt` and `ss.txt` are different files.
fn ntfs_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

/// One tick's engine messages with each download's progress reduced to the
/// latest, kept at its last position; every other message keeps its order.
/// Fast downloads report per network chunk, hundreds of times a tick.
pub fn coalesce_progress(msgs: Vec<ViewToHost>) -> Vec<ViewToHost> {
    let last_progress: std::collections::HashMap<String, usize> = msgs
        .iter()
        .enumerate()
        .filter_map(|(i, m)| match m {
            ViewToHost::DownloadProgress { path, .. } => Some((path.clone(), i)),
            _ => None,
        })
        .collect();
    msgs.into_iter()
        .enumerate()
        .filter(|(i, m)| match m {
            ViewToHost::DownloadProgress { path, .. } => last_progress.get(path) == Some(i),
            _ => true,
        })
        .map(|(_, m)| m)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Review Focus 3: a folder that can't be had refuses the download; nothing is saved anywhere else.
    #[test]
    fn a_destination_is_made_in_the_folder_or_refused() {
        let root = std::env::temp_dir().join(format!("manifold-destination-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("Zoë Q dl");
        let none = |_: &Path| false;
        assert_eq!(destination(Some(dir.as_path()), "../a.zip", none), Ok(dir.join("a.zip")));
        assert!(dir.is_dir(), "the folder is made at the first download");
        std::fs::write(dir.join("a.zip"), b"x").unwrap();
        assert_eq!(destination(Some(dir.as_path()), "a.zip", |p| p.exists()), Ok(dir.join("a (1).zip")));
        assert_eq!(destination(None, "b.zip", none), Err(PathBuf::from("b.zip")), "no folder");
        let rel = Path::new("manifold-relative-dl");
        assert_eq!(destination(Some(rel), "b.zip", none), Err(rel.join("b.zip")), "not a full path");
        assert!(!rel.exists(), "a relative folder is never made");
        let file = root.join("a file");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(destination(Some(file.as_path()), "c.zip", none), Err(file.join("c.zip")), "a file in its place");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_download_starts_and_fails_at_once() {
        let [started, failed] = refused(r"D:\dl\a.zip".into(), Some("http://h/a".into()));
        assert!(
            matches!(&started, ViewToHost::DownloadStarted { path, total_bytes: None, url }
                if path == r"D:\dl\a.zip" && url.as_deref() == Some("http://h/a")),
            "{started:?}"
        );
        assert!(matches!(&failed, ViewToHost::DownloadFailed { path } if path == r"D:\dl\a.zip"), "{failed:?}");
    }

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
    fn a_device_name_prefix_stays_within_the_cap_and_safe_name_is_idempotent() {
        let units = |s: &str| s.encode_utf16().count();
        let inputs = [
            format!("CON.{}", "x".repeat(300)),
            format!("CON.{}", "\u{1F600}".repeat(200)),
            format!("nul{}.txt", " ".repeat(300)),
            format!("CON{}x.txt", " ".repeat(300)),
            "COM1".repeat(100),
            "CON".to_string(),
            "con.txt".to_string(),
            "LPT\u{b9}.tar.gz".to_string(),
            "\u{1F600}".repeat(300),
            format!("{}.pdf", "a".repeat(300)),
        ];
        for raw in &inputs {
            let once = safe_name(raw);
            assert!(units(&once) <= MAX_NAME_UNITS, "{raw:?} gave {} units", units(&once));
            assert!(!is_windows_device_name(&once), "{raw:?} gave a device name");
            assert_eq!(safe_name(&once), once, "{raw:?} is not idempotent");
        }
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        let capped = safe_name(&format!("CON.{}", "x".repeat(300)));
        assert_eq!(units(&capped), MAX_NAME_UNITS);
        assert!(capped.starts_with("_CON.x"));
    }

    #[test]
    fn a_long_name_is_cut_keeping_its_extension() {
        let units = |s: &str| s.encode_utf16().count();
        let long = format!("{}.pdf", "a".repeat(300));
        let name = safe_name(&long);
        assert_eq!(units(&name), MAX_NAME_UNITS);
        assert!(name.ends_with("a.pdf"), "{name}");
        let archive = safe_name(&format!("{}.tar.gz", "b".repeat(300)));
        assert!(archive.ends_with("b.tar.gz") && units(&archive) == MAX_NAME_UNITS, "{archive}");
        // An astral character is two units: never split, so the cut may fall one short.
        let emoji = safe_name(&format!("{}.txt", "\u{1F600}".repeat(200)));
        assert!(emoji.ends_with(".txt") && units(&emoji) <= MAX_NAME_UNITS, "{emoji}");
        let no_ext = safe_name(&"c".repeat(300));
        assert_eq!(no_ext, "c".repeat(MAX_NAME_UNITS));
        let huge_ext = safe_name(&format!("x.{}", "e".repeat(300)));
        assert_eq!(units(&huge_ext), MAX_NAME_UNITS, "a too-long extension is cut with the rest");
        assert!(huge_ext.starts_with("x.e"));
        let dots = safe_name(&format!("{}{}.pdf", "d".repeat(230), ". ".repeat(30)));
        assert!(dots.ends_with("d.pdf"), "the cut's trailing dots and spaces go: {dots}");
        assert_eq!(safe_name("short.pdf"), "short.pdf");
        let numbered = unique_path(Path::new("d"), &name, |p: &Path| p.ends_with(&name));
        let numbered = numbered.file_name().unwrap().to_string_lossy().into_owned();
        assert!(units(&numbered) <= 255, "room is left for the number");
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
    fn an_in_flight_name_takes_its_other_cases_on_windows_only() {
        let dir = Path::new("d");
        assert!(same_download_path(&dir.join("a.pdf"), &dir.join("a.pdf")));
        assert_eq!(same_download_path(&dir.join("Report.PDF"), &dir.join("report.pdf")), cfg!(windows));
        assert_eq!(same_download_path(&dir.join("Ärger.txt"), &dir.join("ärger.txt")), cfg!(windows));
        assert!(!same_download_path(&dir.join("a.pdf"), &dir.join("b.pdf")));
        assert!(!same_download_path(&dir.join("ß.txt"), &dir.join("ss.txt")), "NTFS folds one character at a time");
        assert_eq!(same_download_path(&dir.join("ǆ.txt"), &dir.join("Ǆ.txt")), cfg!(windows));
        let in_flight = [dir.join("A.pdf")];
        let taken = |p: &Path| in_flight.iter().any(|t| same_download_path(t, p));
        let want = if cfg!(windows) { "a (1).pdf" } else { "a.pdf" };
        assert_eq!(unique_path(dir, "a.pdf", taken), dir.join(want));
    }

    #[test]
    fn progress_is_coalesced_to_the_latest_per_path_in_order() {
        use crate::ViewToHost::*;
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
