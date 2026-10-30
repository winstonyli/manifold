//! `gate download`: a tab restored at a URL that answers an attachment starts a download, on each of `VIEWS` (default
//! `webview2`; `VIEWS="webview2 cef"` adds CEF, whose host must be built). Two cases per view, each on its own profile:
//! the profile's own download folder, where the one card must complete and save `real.zip` with the server's bytes; and
//! a folder that cannot be made (a path under a regular file), where the host refuses the download and the one card must
//! show it failed as `FolderUnusable` (`downloads::refused`). It also checks that no `real*.zip*` appears in the user's own
//! Downloads folder: CEF's refusal (return 0) used to leave the file there, and the host now cancels it. WebKit has no
//! gate stage (it runs under WSL), so its `/mnt/<drive>/...` folder stays unexercised (PARKED).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::check::Section;
use crate::instance::{download_dir, Instance};
use crate::probes::{finish, guarded, quit, seed, tab_node, views};
use crate::shots_net::Downloads;

pub(crate) fn download(exe: &Path, work: &Path) -> ExitCode {
    let server = match Downloads::start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("download server: {e}");
            return ExitCode::from(1);
        }
    };
    let url = server.url("/real.zip");
    for view in views() {
        for refused in [false, true] {
            let name = format!("{view}-{}", if refused { "refused" } else { "saved" });
            let _t = Section::start(&format!("download-{name}"));
            guarded(&name, || run_case(exe, work, &view, &name, &url, refused));
        }
    }
    finish(work)
}

fn run_case(exe: &Path, work: &Path, view: &str, name: &str, url: &str, refused: bool) -> io::Result<()> {
    let profile = work.join(format!("appdata-{name}"));
    seed(exe, &profile, view, &[tab_node(Some(url), view)])?;
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
    let before = users_downloads();
    let mut inst = Instance::launch(exe, &profile, work, &format!("download-{name}.log"))?;
    inst.send("WAITTABS")?;
    let saved = folder.join("real.zip");
    let done = |s: &crate::state::State| match s.downloads_from(url)[..] {
        [d] if refused => d["failed"] == true && d["failure"] == "FolderUnusable",
        [d] => d["completed"] == true && d["path"] == saved.to_string_lossy().as_ref(),
        _ => false,
    };
    let state = inst.export_until(&format!("state-{name}"), done)?;
    quit(&mut inst, name);
    let cards = state.downloads_from(url);
    if !done(&state) {
        let want = if refused { "failed as FolderUnusable" } else { "completed, saved in the folder" };
        return Err(io::Error::other(format!("{view}: the download of {url} is not one card {want}: {cards:?}")));
    }
    if refused {
        crate::check::check(&format!("{view} refused: nothing is saved in the folder"), !folder.exists());
        // A refusal that only failed the card would still let the engine save the file in the user's own folder.
        let leaked: Vec<_> = users_downloads().difference(&before).cloned().collect();
        crate::check::check(
            &format!("{view} refused: nothing is saved in the user's Downloads folder {leaked:?}"),
            leaked.is_empty(),
        );
    } else {
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

/// The names `real*.zip*` (also a partial `.crdownload`) in the user's own Downloads folder (where the server's file would be saved if an engine ignored
/// the app's folder); empty if there is no such folder. Only these names, so another program's downloads don't count.
fn users_downloads() -> BTreeSet<String> {
    let Some(home) = std::env::var_os("USERPROFILE") else { return BTreeSet::new() };
    let Ok(entries) = fs::read_dir(Path::new(&home).join("Downloads")) else { return BTreeSet::new() };
    entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("real") && n.contains(".zip"))
        .collect()
}
