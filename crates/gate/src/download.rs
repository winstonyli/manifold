//! `gate download`: a tab restored at a URL that answers an attachment starts a download, on each of `VIEWS` (default
//! `webview2`; `VIEWS="webview2 cef webkit"` adds CEF and WebKit, whose hosts must be built; WebKit also needs `SITE_HOST`,
//! see `gate all`). `CASES` (default `saved refused broken`) picks the cases, three per view, each on its own profile and server: the profile's own download folder, where
//! the one card must complete and save `real.zip` with the server's bytes; a folder that cannot be made (a path under a
//! regular file), where the host refuses the download and the one card must show it failed as `FolderUnusable`
//! (`downloads::refused`); and the server dying mid-transfer (`/slow`, 300 KB of 1 MB), where the one card must show it
//! failed as `Network`, the engine's own failure code as the host maps it. The refused case also checks that no
//! `real*.zip*` appears in the folder the engine would save to on its own: CEF's refusal (return 0) used to leave the
//! file in the user's Downloads, and the host now cancels it. For WebKit that folder is `~/Downloads` in WSL, and its
//! folder reaches the host as `/mnt/<drive>/...`.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::check::Section;
use crate::instance::{download_dir, Instance};
use crate::probes::{finish, guarded, quit, seed, tab_node, views};
use crate::shots_net::Downloads;

/// The three cases a view runs, each on its own profile and its own server.
#[derive(Clone, Copy, PartialEq)]
enum Case {
    Saved,
    Refused,
    Broken,
}

const CASES: [(Case, &str); 3] = [(Case::Saved, "saved"), (Case::Refused, "refused"), (Case::Broken, "broken")];

pub(crate) fn download(exe: &Path, work: &Path) -> ExitCode {
    let cases = std::env::var("CASES").unwrap_or_else(|_| "saved refused broken".into());
    for view in views() {
        for (case, label) in CASES.iter().filter(|(_, label)| cases.split_whitespace().any(|c| c == *label)) {
            let name = format!("{view}-{label}");
            let _t = Section::start(&format!("download-{name}"));
            guarded(&name, || run_case(exe, work, &view, &name, *case));
        }
    }
    finish(work)
}

fn run_case(exe: &Path, work: &Path, view: &str, name: &str, case: Case) -> io::Result<()> {
    let server = Downloads::start()?;
    let url = server.url(if case == Case::Broken { "/slow" } else { "/real.zip" });
    let refused = case == Case::Refused;
    let profile = work.join(format!("appdata-{name}"));
    seed(exe, &profile, view, &[tab_node(Some(&url), view)])?;
    let folder = if refused {
        // A folder under a file cannot be made: the host must refuse the download rather than lose it.
        let file = work.join(format!("{name}-not-a-folder"));
        fs::write(&file, b"")?;
        let folder = file.join("downloads");
        set_download_folder(&profile, &folder)?;
        folder
    } else {
        download_dir(&profile)?
    };
    let before = engine_downloads(view);
    let mut inst = Instance::launch(exe, &profile, work, &format!("download-{name}.log"))?;
    inst.send("WAITTABS")?;
    let saved = folder.join(if case == Case::Broken { "slow.zip" } else { "real.zip" });
    let saved_path = saved.to_string_lossy().into_owned();
    if case == Case::Broken {
        // Still arriving (300 KB of 1 MB) before the server dies.
        let started =
            |s: &crate::state::State| matches!(s.downloads_from(&url)[..], [d] if d["path"] == saved_path.as_str());
        let state = inst.export_until(&format!("started-{name}"), started)?;
        if !started(&state) {
            quit(&mut inst, name);
            return Err(io::Error::other(format!(
                "{view}: no card saving {url} to {saved_path}: {:?}",
                state.downloads_from(&url)
            )));
        }
        server.kill();
    }
    let done = |s: &crate::state::State| match s.downloads_from(&url)[..] {
        [d] if refused => d["failed"] == true && d["failure"] == "FolderUnusable",
        [d] if case == Case::Broken => {
            d["failed"] == true && d["failure"] == "Network" && d["path"] == saved_path.as_str()
        }
        [d] => d["completed"] == true && d["path"] == saved_path.as_str(),
        _ => false,
    };
    let state = inst.export_until(&format!("state-{name}"), done)?;
    quit(&mut inst, name);
    let cards = state.downloads_from(&url);
    if !done(&state) {
        let want = match case {
            Case::Refused => "failed as FolderUnusable",
            Case::Broken => "failed as Network, saving to the folder",
            Case::Saved => "completed, saved in the folder",
        };
        return Err(io::Error::other(format!("{view}: the download of {url} is not one card {want}: {cards:?}")));
    }
    if refused {
        crate::check::check(&format!("{view} refused: nothing is saved in the folder"), !folder.exists());
        // A refusal that only failed the card would still let the engine save the file in its own folder.
        let leaked: Vec<_> = engine_downloads(view).difference(&before).cloned().collect();
        crate::check::check(
            &format!("{view} refused: nothing is saved in the engine's own Downloads folder {leaked:?}"),
            leaked.is_empty(),
        );
    } else if case == Case::Saved {
        crate::check::check_eq(
            &format!("{view} saved: real.zip holds the server's bytes"),
            "data".to_owned(),
            String::from_utf8_lossy(&fs::read(&saved).unwrap_or_default()).into_owned(),
        );
    }
    Ok(())
}

/// `PROFILE/settings.json` with `download_directory` set to `folder`.
fn set_download_folder(profile: &Path, folder: &Path) -> io::Result<()> {
    let path = profile.join("settings.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)?;
    settings["download_directory"] = Value::from(folder.to_string_lossy().into_owned());
    fs::write(path, settings.to_string())
}

/// The names `real*.zip*` (also a partial `.crdownload`) in the folder `view`'s engine saves to when it ignores the app's
/// folder: WebKit's `~/Downloads` in WSL, the user's Downloads folder for the others; empty if there is no such folder.
/// Only these names, so another program's downloads don't count.
fn engine_downloads(view: &str) -> BTreeSet<String> {
    let names: Vec<String> = if view == "webkit" {
        let listed = Command::new("wsl.exe")
            .args(["-d", "Ubuntu", "--", "sh", "-c", "ls \"$HOME/Downloads\" 2>/dev/null"])
            .stdin(Stdio::null())
            .output();
        listed.map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect()).unwrap_or_default()
    } else {
        let Some(home) = std::env::var_os("USERPROFILE") else { return BTreeSet::new() };
        let Ok(entries) = fs::read_dir(Path::new(&home).join("Downloads")) else { return BTreeSet::new() };
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    };
    names.into_iter().filter(|n| n.starts_with("real") && n.contains(".zip")).collect()
}
