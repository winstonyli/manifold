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

/// A server-suggested file name reduced to its last component, so `..` or
/// an absolute name can't leave the downloads folder; "download" if empty.
pub fn safe_name(suggested: &str) -> String {
    Path::new(suggested)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string()
}

/// `dir/name`, or `dir/stem (n).ext` with the first n that isn't `taken`
/// (an existing file, or one another download in flight has reserved).
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = as_path.extension().and_then(|e| e.to_str());
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
