//! `gate chrome-shots` and `gate chrome-shots-sizes` (were scripts/chrome-shots.sh and chrome-shots-sizes.sh):
//! screenshots of the app's chrome in every view, for iterating on the UI.
//!
//! One headless instance on a fresh isolated profile with a small seeded session (a tab, a group with a tab in it, a
//! note, an image, a JSON file); every tab is `about:blank`, so no page content competes with the chrome. Writes,
//! under `OUT` (default `target/shots`):
//!
//! - `<view>.png`: the whole window, as `SCREENSHOT` composites it
//! - `crop/<view>.png`: the top `CROP_PX` (110) rows (toolbar and address bar)
//! - `sheet.png`: every view in one grid
//!
//! Each view starts from Graph view at the top level and leaves the app there, so `VIEWS` picks any subset and a run is
//! about 40 s. `SIZE` (`"1280 800"`) is the window size; the clicks on fixed coordinates (card positions on the
//! canvas, parking the pointer) assume the default. Chrome controls (toolbar buttons, the search field, the Tab actions
//! popup's rows) are clicked or hovered by the name `EXPORTSTATE`'s `controls` lists (`crate::state`), wherever the
//! toolbar puts them; a name that is not on screen fails the view, naming it. The default views are `DEFAULT_VIEWS`;
//! the rest are opt-in:
//!
//! - hover scans (`hover-graph hover-json hover-json-sel hover-tabview hover-find hover-settings hover-menu-tab
//!   hover-dropdown hover-quickswitch`) and Tab-key scans (`tab-graph tab-tabview tab-settings tab-find tab-quickswitch
//!   tab-dropdown tab-popup tab-popup-history tab-popup-downloads tab-engine tab-combo`) sweep the mouse or the Tab key
//!   along a bar and print where anything other than the hovered widget moved (`shots_img::hover_report`);
//!   `focus-graph` does the same for a click into the Graph search field. `tab-graph` and the JSON bar's scan (in
//!   `json-keys`) press Tab until no named control has focus (`tab_walk`), print each stop, and fail if a focusable
//!   named control of the bar was never a stop, so no stop count goes stale and a walk that ends early cannot pass
//! - keyboard views: `graph-enter-group graph-enter-json graph-menu-key graph-select-key graph-pinned-dense json-keys tabview-split-key`
//! - assertion views, which print `PASS:`/`FAIL:`: `mouse-regress` (drag, middle-click close),
//!   `note-key`, `digit-jump`, `json-tab-offscreen`, `tabview-actions-click`
//! - `graph-menu-jars graph-menu-chip graph-menu-group-jars`: the menu's Jar page
//!
//! `engine-states-picker engine-states-settings engine-install-confirm` (any one runs the block) start more instances
//! to shoot the CEF install states: a copy of the exe with no CEF host beside it, on its own profile, walks Not
//! installed, Failed, Installing, Installed and Update needed with nothing mocked in the app (the runtime is files in
//! the profile's data dir; the download goes through `HTTPS_PROXY` to `shots_net::Proxy`, which refuses or stalls), and
//! two more name a bring-your-own folder that matches the pin or not (Custom). A release pins its hosts through a
//! manifest compiled into the exe; a debug build reads one from `MANIFOLD_HOST_PINS_FILE`, so the block writes a fixture
//! manifest with a made-up CEF host (`fake_cef_host`) and needs no real download.
//!
//! `gate chrome-shots-sizes` runs it at each of `SIZES` (`"400 300,800 600,1920 1080"`) into `OUT/<W>x<H>/`, with
//! `VIEWS` defaulting to the ones that need no click on a fixed coordinate. `gate layout` checks the popups' bounds
//! by number; this is for looking.
//!
//! `gate shots-diff A B` compares two runs' trees by decoded pixels (PNG bytes differ even where pixels match). Files
//! whose path shows in a shot live in a folder named by their contents (`content_dir`), not the pid-named work dir. A/A
//! on 2026-10-27 (60412333; run `chrome-shots-aa-5` in `docs/runs.md`): all 67 default views matched. What made that
//! possible: headless mode's caret doesn't blink, the download server keeps one port (`shots_net`; a busy port is
//! logged, and then the two download views won't match), and the address bar's suggestions come from the local
//! `suggest::Server`. Later runs showed two more: a view's toast outlived it and showed in the next view's shot or not
//! by timing (`reset` sends `CLEARTOASTS`), and the graph layout summed forces in an order set by each launch's random
//! actor, so cards settled a ulp apart (`graph_layout::ExactSum`). `gate shots-aa` is that A/A as one stage, run by `gate all` in `full`. Build first: `cargo build -p manifold -p manifold-view-webview2 -p gate`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use manifold_hosts::{Os, CEF, CEF_RUNTIME_DIR, HOSTS_DIR, WEBVIEW2};
use serde_json::{json, Value};

use crate::check::{check, check_eq, note_failure, Section};
use crate::instance::{seed_settings, Instance};
use crate::probes::{finish, import_session, quit};
use crate::shots_img::{hover_report, write_sheet};
use crate::shots_net::{resume_body, Downloads, Proxy};
use crate::state::{
    bar_stops, round_half_even, unvisited_bar_stops, Control, State, COPY_VALUE, GROUP, IMAGE, JSON, NOTE, SEARCH,
    SPLIT_VIEW, TAB, TAB_ACTIONS, TAB_ACTIONS_CLOSE_TAB, TAB_ACTIONS_DUPLICATE, TAB_ACTIONS_PIN,
};
use crate::suggest;

const DEFAULT_VIEWS: &str = "graph graph-hover graph-menu-tab graph-menu-group graph-menu-note graph-link graph-menu-links graph-link-hover graph-link-handles graph-link-drag graph-drop-hint graph-dissolve-undo graph-filter graph-group json-level json-menu tabview tabview-dropdown tabview-find tabview-actions tabview-split settings popup-bookmarks popup-history popup-downloads shortcuts quickswitch graph-closed graph-newnode-menu newnode-zoom graph-tooltip image-view cards-f note-edit missing-file download-real download-progress download-failed download-resume json-zoom settings-engine settings-engine-tab settings-combo toast tabview-engine json-tooltip engine-states-picker engine-states-settings engine-install-confirm";
const SIZES_VIEWS: &str = "graph graph-menu-tab graph-newnode-menu shortcuts quickswitch settings tabview";
/// The CEF host the fixture manifest pins: (file, size, SHA-256). Nothing downloads it; the files on disk are sparse.
fn fake_cef_host() -> (String, u64, &'static str) {
    (CEF.file_name(Os::here()), 740_000, "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c")
}
const CEF_PIN: &str = include_str!("../../manifold/src/cef_pin.rs");
/// Where a click outside every popup lands and closes it.
const AWAY: &str = "CLICK 1100 300";
/// The settings page's pickers, by the labels of their rows (`labelled_by`).
const DEFAULT_ENGINE: &str = "Default starting engine (new tabs)";
const SEARCH_ENGINE: &str = "Default search engine";
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);
/// The most Tab presses a `tab_walk` makes before it reports that it never ended.
const TAB_WALK_CAP: u32 = 40;

fn knob(name: &str, default: u32) -> u32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    let mut it = text.split_whitespace().map(str::parse::<u32>);
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(w)), Some(Ok(h)), None) => Some((w, h)),
        _ => None,
    }
}

struct Config {
    size: (u32, u32),
    views: Vec<String>,
    out: PathBuf,
    crop_px: u32,
}

/// `target/` beside the exe's `debug/`.
fn default_out(exe: &Path, leaf: &str) -> PathBuf {
    exe.parent().and_then(Path::parent).map_or_else(|| PathBuf::from("target"), Path::to_path_buf).join(leaf)
}

/// A `chrome-shots` run from `SIZE`, `VIEWS` and `CROP_PX`, into `out`; `None` (and said) when `SIZE` isn't "W H".
fn config(out: PathBuf) -> Option<Config> {
    let Some(size) = parse_size(&std::env::var("SIZE").unwrap_or_else(|_| "1280 800".into())) else {
        eprintln!("SIZE is \"W H\"");
        return None;
    };
    let views = std::env::var("VIEWS").unwrap_or_else(|_| DEFAULT_VIEWS.into());
    Some(Config {
        size,
        views: views.split_whitespace().map(str::to_owned).collect(),
        out,
        crop_px: knob("CROP_PX", 110),
    })
}

pub(crate) fn chrome_shots(exe: &Path, work: &Path) -> ExitCode {
    let Some(cfg) = config(std::env::var_os("OUT").map_or_else(|| default_out(exe, "shots"), PathBuf::from)) else {
        return ExitCode::from(2);
    };
    run(exe, work, cfg);
    finish(work)
}

/// `gate shots-aa`: `chrome-shots` twice, into `OUT/a` and `OUT/b` (default `target/shots-aa`, emptied first), then
/// `shots-diff` of the two. Fails unless every shot matches pixel for pixel: a shot that depends on more than the
/// commit (a settle's path, a clock, a port) shows up here before it confuses a comparison across commits.
pub(crate) fn shots_aa(exe: &Path, work: &Path) -> ExitCode {
    let out = std::env::var_os("OUT").map_or_else(|| default_out(exe, "shots-aa"), PathBuf::from);
    for leaf in ["a", "b"] {
        let Some(cfg) = config(out.join(leaf)) else { return ExitCode::from(2) };
        println!("== {leaf} -> {}", cfg.out.display());
        let _ = fs::remove_dir_all(&cfg.out);
        run(exe, &work.join(leaf), cfg);
    }
    match crate::shots_img::diff_trees(&out.join("a"), &out.join("b")) {
        Ok((lines, same)) => {
            lines.iter().for_each(|l| println!("{l}"));
            check("the two runs' shots match pixel for pixel", same);
        }
        Err(e) => {
            println!("FAIL: shots-diff: {e}");
            note_failure();
        }
    }
    finish(work)
}

pub(crate) fn chrome_shots_sizes(exe: &Path, work: &Path) -> ExitCode {
    let sizes = std::env::var("SIZES").unwrap_or_else(|_| "400 300,800 600,1920 1080".into());
    let views = std::env::var("VIEWS").unwrap_or_else(|_| SIZES_VIEWS.into());
    let out = std::env::var_os("OUT").map_or_else(|| default_out(exe, "shots-sizes"), PathBuf::from);
    for text in sizes.split(',') {
        let Some(size) = parse_size(text) else {
            println!("FAIL: SIZES entry {text:?} is not \"W H\"");
            note_failure();
            continue;
        };
        let leaf = format!("{}x{}", size.0, size.1);
        println!("== {} {} -> {}", size.0, size.1, out.join(&leaf).display());
        let cfg = Config {
            size,
            views: views.split_whitespace().map(str::to_owned).collect(),
            out: out.join(&leaf),
            crop_px: knob("CROP_PX", 110),
        };
        run(exe, &work.join(&leaf), cfg);
    }
    finish(work)
}

/// One run: a failure is printed and counted, not returned.
fn run(exe: &Path, work: &Path, cfg: Config) {
    let _t = Section::start("chrome-shots");
    let views = cfg.views.clone();
    let (out, crop_px) = (cfg.out.clone(), cfg.crop_px);
    let result = Shots::start(exe, work, cfg).and_then(|mut s| {
        let result = s.run_views(exe);
        if result.is_err() {
            println!("--- tail of {}\n{}", s.inst.log.display(), s.inst.log_tail(30));
        }
        result
    });
    if let Err(e) = result {
        println!("FAIL: chrome-shots stopped early: {e}");
        note_failure();
    }
    match write_sheet(&out, crop_px, &views) {
        Ok(line) => println!("{line}"),
        Err(e) => println!("sheet: {e}"),
    }
}

struct Shots {
    work: PathBuf,
    /// The instance's download folder (`instance::download_dir`), emptied before the download views.
    downloads: PathBuf,
    /// `pic.png` and `data.json`, the files ADDFILE adds (`content_dir`).
    seed: PathBuf,
    inst: Instance,
    out: PathBuf,
    size: (u32, u32),
    views: Vec<String>,
    /// The engine-states instance's data dir for the engine hosts.
    eng_data: PathBuf,
    /// The address bar's suggestions, local and fixed, so the dropdown shows the same rows every run.
    suggestions: suggest::Server,
}

macro_rules! send {
    ($s:expr, $($c:expr),+ $(,)?) => { $s.send_all(&[$($c.to_string()),+]) };
}

impl Shots {
    fn start(exe: &Path, work: &Path, cfg: Config) -> io::Result<Shots> {
        fs::create_dir_all(work)?;
        fs::create_dir_all(cfg.out.join("crop"))?;
        let root = work.join("appdata");
        let profile = root.join("manifold");
        seed_settings(&profile, "webview2", "")?;
        // Rows for the Bookmarks and History popups.
        fs::write(
            profile.join("bookmarks.json"),
            json!([
                {"title": "Example Domain", "url": "https://example.com/", "folder": null},
                {"title": "Rust docs", "url": "https://doc.rust-lang.org/", "folder": "Reading"},
                {"title": "A rather long page title that has to be truncated by the popup", "url": "https://example.org/long", "folder": "Reading"},
            ])
            .to_string(),
        )?;
        fs::write(
            profile.join("history.json"),
            json!([
                {"title": "Example Domain", "url": "https://example.com/"},
                {"title": "Wikipedia", "url": "https://en.wikipedia.org/"},
                {"title": "Hacker News", "url": "https://news.ycombinator.com/"},
            ])
            .to_string(),
        )?;
        import_session(
            exe,
            &profile,
            &[
                json!({"id": "t1", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}),
                json!({"id": "g", "type": GROUP, "name": "Research", "pos": [300.0, 0.0]}),
                json!({"id": "t2", "type": TAB, "url": "about:blank", "view_kind": "webview2", "group": "g", "pos": [0.0, 0.0]}),
                json!({"id": "n1", "type": NOTE, "text": "a note", "pos": [-300.0, 0.0]}),
            ],
        )?;
        // An image and a JSON file, added by ADDFILE once the app is up.
        let mut png = io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(240, 160, image::Rgb([70, 110, 160]))
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        let data = seed_data();
        let seed = content_dir(&std::env::temp_dir(), &[("pic.png", png.get_ref()), ("data.json", data.as_bytes())])?;
        let suggestions = suggest::Server::start()?;
        let (appdata, suggest_url) = (root.display().to_string(), suggestions.url());
        let inst = Instance::launch_env(
            exe,
            &profile,
            work,
            "log",
            &[("APPDATA", &appdata), ("MANIFOLD_SUGGESTIONS_URL", &suggest_url)],
        )?;
        let downloads = crate::instance::download_dir(&profile)?;
        Ok(Shots {
            work: work.to_path_buf(),
            downloads,
            seed,
            inst,
            out: cfg.out,
            size: cfg.size,
            views: cfg.views,
            eng_data: PathBuf::new(),
            suggestions,
        })
    }

    // ---- driving -------------------------------------------------------------------------------------------------

    fn has(&self, view: &str) -> bool {
        self.views.iter().any(|v| v == view)
    }

    fn has_any(&self, views: &[&str]) -> bool {
        views.iter().any(|v| self.has(v))
    }

    fn send_all(&mut self, cmds: &[String]) -> io::Result<()> {
        cmds.iter().try_for_each(|c| self.inst.send(c))
    }

    fn state(&mut self) -> io::Result<State> {
        self.inst.export_state("state")
    }

    /// Runs one step; an I/O error (a timeout, no such card) is a failure of that step and the run goes on, unless
    /// the instance is gone.
    fn step(&mut self, label: &str, body: impl FnOnce(&mut Self) -> io::Result<()>) -> io::Result<()> {
        if let Err(e) = body(self) {
            println!("FAIL: {label}: {e}");
            note_failure();
            if !self.inst.is_running() {
                return Err(io::Error::other("the instance exited"));
            }
        }
        Ok(())
    }

    /// `step` when the view `name` is asked for.
    fn view(&mut self, name: &str, body: impl FnOnce(&mut Self) -> io::Result<()>) -> io::Result<()> {
        if self.has(name) {
            self.step(name, body)?;
        }
        Ok(())
    }

    /// The first top-level (`place` "top"; else any) card of type `ty` (and engine state `loaded`): the centre of the
    /// part of its rect inside the window, since a card can hang off the edge. Place "chip" is the point where the
    /// jar chip hangs, a fixed number of screen points above the card's bottom-left corner.
    fn center(&mut self, ty: &str, place: &str, loaded: Option<bool>) -> io::Result<(i64, i64)> {
        let s = self.state()?;
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let c = s
            .all_cards()
            .iter()
            .find(|c| State::card_is(c, ty, loaded) && (place != "top" || c["group"].is_null()))
            .ok_or_else(|| io::Error::other(format!("no {place} card of type {ty}")))?;
        let r = rect(c)?;
        Ok(if place == "chip" {
            ((r[0] + 18.0) as i64, (r[3] - 33.0) as i64)
        } else {
            (((r[0].max(0.0) + r[2].min(w)) / 2.0) as i64, ((r[1].max(36.0) + r[3].min(h)) / 2.0) as i64)
        })
    }

    /// The id of the first top-level card of type `ty` (import gives the seeded nodes fresh ids).
    fn card_id(&mut self, ty: &str) -> io::Result<String> {
        let s = self.state()?;
        s.all_cards()
            .iter()
            .find(|c| State::card_is(c, ty, None) && c["group"].is_null())
            .and_then(|c| c["id"].as_str().map(str::to_string))
            .ok_or_else(|| io::Error::other(format!("no top-level card of type {ty}")))
    }

    /// Waits for the app to be idle: SLEEP draws no frames, and the camera eases after entering a level, so a
    /// fixed sleep can shoot mid-ease.
    fn settle(&mut self) -> io::Result<()> {
        self.inst.send("WAITIDLE")
    }

    /// Double-clicks the first top-level loaded tab card, looked up now (a fit or a new card moves it), and waits for
    /// its Tab view.
    fn open_tab(&mut self) -> io::Result<()> {
        let (x, y) = self.center(TAB, "top", Some(true))?;
        send!(self, format!("DOUBLECLICK {x} {y}"))?;
        if !self.inst.export_until("tab-view", State::tab_view)?.tab_view() {
            return Err(io::Error::other(format!("a double-click at {x} {y} did not open Tab view")));
        }
        self.settle()
    }

    fn shot(&mut self, name: &str) -> io::Result<()> {
        self.settle()?;
        let path = self.out.join(format!("{name}.png"));
        self.inst.screenshot_to(&path)?;
        println!("shot: {name}");
        Ok(())
    }

    /// Back to Graph view at the top level, nothing open. Escape closes popups and leaves Tab view; the toolbar's
    /// back button (top left) leaves a group or a JSON level. A view's toast is dropped too: it expires on the clock,
    /// so a later view's shot would show it or not depending on how long the steps between took.
    fn reset(&mut self) -> io::Result<()> {
        send!(self, "CLEARTOASTS")?;
        for _ in 0..5 {
            send!(self, "ESCAPE")?;
            self.settle()?;
            let s = self.state()?;
            if !s.tab_view() {
                if s.current_group().is_none() && s.0["json_level"].is_null() {
                    return Ok(());
                }
                self.inst.click_control(crate::state::BACK)?;
            }
        }
        eprintln!("reset: not back at the top level");
        Ok(())
    }

    fn menu_at(&mut self, x: i64, y: i64) -> io::Result<()> {
        send!(
            self,
            format!("MOUSEMOVE {x} {y}"),
            format!("MOUSEDOWN {x} {y} right"),
            format!("MOUSEUP {x} {y} right"),
            "SLEEP 1"
        )
    }

    /// `click` on the chrome control named `name` (`crate::state`), wherever the toolbar has put it.
    fn click_named(&mut self, name: &str) -> io::Result<()> {
        self.inst.click_control(name)?;
        send!(self, "SLEEP 1")
    }

    fn click(&mut self, x: i64, y: i64) -> io::Result<()> {
        send!(
            self,
            format!("MOUSEMOVE {x} {y}"),
            format!("MOUSEDOWN {x} {y} left"),
            format!("MOUSEUP {x} {y} left"),
            "SLEEP 1"
        )
    }

    /// Opens the focused tab's menu, closes it, and names two jars (Alpha, Beta): the focused tab uses the second.
    fn make_jars(&mut self, x: i64, y: i64) -> io::Result<()> {
        self.menu_at(x, y)?;
        send!(self, "ESCAPE", "SETJAR Alpha", "SETJAR Beta", "SLEEP 1")
    }

    fn tab_presses(&mut self, n: u32, pause: &str) -> io::Result<()> {
        for _ in 0..n {
            send!(self, "TABKEY", format!("SLEEP {pause}"))?;
        }
        Ok(())
    }

    /// The rect `[x0, y0, x1, y1]` of the open menu or popup, if any (the key hint beside a keyboard-focused card does
    /// not count).
    fn popup_rect(&mut self) -> io::Result<Option<[f64; 4]>> {
        Ok(self.state()?.popup_rects().first().copied())
    }

    fn menu_open(&mut self) -> io::Result<bool> {
        Ok(self.popup_rect()?.is_some())
    }

    fn editing(&mut self) -> io::Result<bool> {
        Ok(self.state()?.cards().iter().any(|c| c["editing"].as_bool() == Some(true)))
    }

    fn card_count(&mut self) -> io::Result<usize> {
        Ok(self.state()?.cards().len())
    }

    // ---- scans ---------------------------------------------------------------------------------------------------

    fn scan_dir(&self, name: &str) -> io::Result<PathBuf> {
        let dir = self.out.join("hover").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// The scan's reference shot with the pointer parked at `pos`.
    fn rest_shot(&mut self, dir: &Path, pos: &str) -> io::Result<()> {
        send!(self, format!("MOUSEMOVE {pos}"), "SLEEP 1")?;
        self.settle()?;
        self.inst.screenshot_to(&dir.join("rest.png"))
    }

    fn print_report(dir: &Path, band: u32, column: bool, x0: u32) -> io::Result<()> {
        hover_report(dir, band, column, x0)?.iter().for_each(|l| println!("{l}"));
        Ok(())
    }

    /// With the app in the view `name`: shoot it at rest and with the mouse every `HOVER_STEP` px along the top bar,
    /// into `OUT/hover/<name>/`, and report where a hover moved anything other than the hovered widget.
    fn hover_scan(&mut self, name: &str, y: u32, band: u32) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "5 790")?;
        for x in (4..=self.size.0).step_by(knob("HOVER_STEP", 14) as usize) {
            send!(self, format!("MOUSEMOVE {x} {y}"), "SLEEP 0.3")?;
            self.inst.screenshot_to(&dir.join(format!("x{x:04}.png")))?;
        }
        send!(self, "MOUSEMOVE 5 790")?;
        Self::print_report(&dir, band, false, 0)
    }

    /// The same down the column at `x` (the settings page's widgets).
    fn hover_column(&mut self, name: &str, x: u32, y0: u32, y1: u32, band: u32, x0: u32) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "1275 795")?;
        for y in (y0..=y1).step_by(knob("HOVER_STEP", 14) as usize) {
            send!(self, format!("MOUSEMOVE {x} {y}"), "SLEEP 0.3")?;
            self.inst.screenshot_to(&dir.join(format!("y{y:04}.png")))?;
        }
        Self::print_report(&dir, band, true, x0)
    }

    /// One Tab key press (egui keyboard focus moving on) and the shot after it, `x<i>.png` in `dir`.
    fn tab_press(&mut self, dir: &Path, i: u32) -> io::Result<()> {
        send!(self, "TABKEY", "SLEEP 0.5")?;
        self.inst.screenshot_to(&dir.join(format!("x{i:04}.png")))
    }

    /// Rest, then `n` Tab key presses, a shot after each.
    fn tab_scan(&mut self, name: &str, n: u32, band: u32) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "1275 795")?;
        for i in 1..=n {
            self.tab_press(&dir, i)?;
        }
        Self::print_report(&dir, band, false, 0)
    }

    /// `tab_scan` with no stop count to go stale: presses Tab (a shot and an export after each) and ends at the first
    /// press after which no control is selected except those selected at rest (compared by name and rect), or after
    /// `TAB_WALK_CAP` presses, which is a failure. A press ends it by moving focus off the last Tab stop on screen
    /// (egui drops focus there; the presses after that are the canvas's own walk over cards) or onto a control the
    /// export does not name. Each stop prints its control's name; a glyph-only icon button's name is its glyph, a
    /// private-use character that prints blank. Fails if no press lands on a named control, if the walk does not end,
    /// if the bar at rest has no focusable named control (`state::bar_stops`; a disabled button is not one), or if one
    /// of them was never a stop (`state::unvisited_bar_stops`): an unnamed stop in the middle of the bar ends the walk
    /// there, and the controls after it show up as never reached.
    fn tab_walk(&mut self, name: &str, band: u32) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "1275 795")?;
        let rest = self.state()?.controls();
        // Selected at rest (a toggled button, a highlighted row) is not focus.
        let selected_at_rest: Vec<&Control> = rest.iter().filter(|c| c.selected).collect();
        let mut stops: Vec<Control> = Vec::new();
        let mut presses = 0;
        let mut ended = false;
        for i in 1..=TAB_WALK_CAP {
            self.tab_press(&dir, i)?;
            let focus: Vec<Control> = self
                .state()?
                .selected_controls()
                .into_iter()
                .filter(|c| !selected_at_rest.iter().any(|r| r.is(c)))
                .collect();
            if focus.is_empty() {
                ended = true;
                break;
            }
            println!("{name}: Tab stop {i}: {}", focus.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));
            presses = i;
            stops.extend(focus);
        }
        check(&format!("{name}: the first Tab lands on a named control"), presses > 0);
        check(&format!("{name}: the Tab walk ends within {TAB_WALK_CAP} stops ({presses} named)"), ended);
        // A bar with no focusable control at rest checks nothing, so that fails too. A glyph's name shows escaped.
        let bar = bar_stops(&rest).len();
        let missed = unvisited_bar_stops(&rest, &stops);
        let mut label = format!("{name}: the Tab walk reached all {bar} focusable bar controls");
        if !missed.is_empty() {
            let list: Vec<String> = missed.iter().map(|c| format!("{:?} at {:?}", c.name, c.rect)).collect();
            label = format!("{label}; never reached: {}", list.join("; "));
        }
        check(&label, bar > 0 && missed.is_empty());
        Self::print_report(&dir, band, false, 0)
    }

    /// Rest, then a click on each of the controls named `controls` (focusing it), Escape between. Same report as
    /// `hover_scan`. A name the toolbar no longer shows fails the view, naming it.
    fn focus_scan(&mut self, name: &str, controls: &[&str]) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "5 790")?;
        for (i, control) in controls.iter().enumerate() {
            self.inst.click_control(control)?;
            send!(self, "SLEEP 0.5", "MOUSEMOVE 5 790", "SLEEP 0.5")?;
            self.inst.screenshot_to(&dir.join(format!("x{:04}.png", i + 1)))?;
            send!(self, "ESCAPE", "SLEEP 0.5")?;
        }
        Self::print_report(&dir, 34, false, 0)
    }

    // ---- the views -----------------------------------------------------------------------------------------------

    fn run_views(&mut self, exe: &Path) -> io::Result<()> {
        let (w, h) = self.size;
        let (work, seed) = (self.work.clone(), self.seed.clone());
        send!(self, format!("RESIZE {w} {h}"), "WAITTABS", "SLEEP 3")?;
        send!(self, format!("ADDFILE {} 200 640", seed.join("pic.png").display()))?;
        send!(self, format!("ADDFILE {} 640 700", seed.join("data.json").display()), "SLEEP 2")?;

        self.view("graph", |s| s.shot("graph"))?;
        self.graph_menus()?;
        self.link_views()?;
        self.view("hover-graph", |s| s.hover_scan("graph", knob("HOVER_Y", 16), knob("HOVER_BAND", 34)))?;
        self.view("tab-graph", |s| s.tab_walk("graph", knob("HOVER_BAND", 34)))?;
        self.keyboard_views()?;
        self.view("mouse-regress", Self::mouse_regress)?;
        self.view("hover-menu-tab", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            s.menu_at(x, y)?;
            s.hover_column(
                "menu-tab",
                knob("MENU_X", 700),
                knob("MENU_Y0", 424),
                knob("MENU_Y1", 646),
                190,
                knob("MENU_X0", 640),
            )?;
            s.reset()
        })?;
        self.view("focus-graph", |s| s.focus_scan("graph", &[SEARCH]))?;
        // Right-click on empty canvas: the New Tab/Note/Group card.
        self.view("graph-newnode-menu", |s| {
            s.menu_at_empty()?;
            s.shot("graph-newnode-menu")?;
            s.reset()
        })?;
        // The same card zoomed far out: its rows keep a constant screen size (they once scaled with zoom and overflowed).
        self.view("newnode-zoom", |s| {
            for _ in 0..6 {
                s.click_named(crate::state::ZOOM_OUT)?;
            }
            s.menu_at_empty()?;
            s.shot("newnode-zoom")?;
            s.reset()
        })?;
        // A toolbar tooltip (the shortcuts help button).
        self.view("graph-tooltip", |s| {
            s.inst.hover_control("Keyboard Shortcuts (F1)")?;
            send!(s, "SLEEP 2")?;
            s.shot("graph-tooltip")?;
            send!(s, "MOUSEMOVE 5 790")
        })?;
        // A download card and a sleeping tab beside the live cards (header F on cards with no seeded kind of their own).
        self.view("cards-f", |s| {
            send!(s, format!("ADDDOWNLOAD {}", seed.join("data.json").display()), "SLEEPNODE t1", "SLEEP 6")?;
            s.inst.click_control(crate::state::FIT_VIEW)?;
            send!(s, "SLEEP 2")?;
            s.shot("cards-f")
        })?;
        // A note being edited: a double-click on the rendered note enters the editor in place.
        self.view("note-edit", |s| {
            let (x, y) = s.center(NOTE, "top", None)?;
            send!(s, format!("DOUBLECLICK {x} {y}"), "SLEEP 1", "MOUSEMOVE 5 790", "SLEEP 1")?;
            s.shot("note-edit")?;
            send!(s, "ESCAPE", "SLEEP 1")
        })?;
        self.view("image-view", |s| {
            let (x, y) = s.center(IMAGE, "top", None)?;
            send!(s, format!("DOUBLECLICK {x} {y}"), "SLEEP 3")?;
            s.shot("image-view")?;
            s.reset()
        })?;
        self.view("graph-filter", |s| {
            send!(s, "GRAPHFILTERTEXT res", "SLEEP 1")?;
            s.shot("graph-filter")?;
            send!(s, "GRAPHFILTERTEXT ")
        })?;
        self.view("graph-group", |s| {
            let (x, y) = s.center(GROUP, "top", None)?;
            send!(s, format!("DOUBLECLICK {x} {y}"), "SLEEP 3")?;
            s.shot("graph-group")?;
            s.reset()
        })?;
        self.json_views()?;
        self.tabview_views()?;
        self.view("tab-quickswitch", |s| {
            send!(s, "QUICKSWITCHOPEN", "SLEEP 1")?;
            s.tab_scan("quickswitch", 4, 400)?;
            send!(s, "QUICKSWITCHESCAPE", "SLEEP 1")
        })?;
        self.view("hover-quickswitch", |s| {
            send!(s, "QUICKSWITCHOPEN", "SLEEP 1")?;
            s.hover_column("quickswitch", 640, 90, 230, 400, 440)?;
            send!(s, "QUICKSWITCHESCAPE", "SLEEP 1")
        })?;
        self.view("shortcuts", |s| {
            send!(s, "TOGGLESHORTCUTSHELP", "SLEEP 1")?;
            s.shot("shortcuts")?;
            send!(s, "TOGGLESHORTCUTSHELP", "SLEEP 1")
        })?;
        self.view("quickswitch", |s| {
            send!(s, "QUICKSWITCHOPEN", "SLEEP 1")?;
            s.shot("quickswitch")?;
            send!(s, "QUICKSWITCHESCAPE", "SLEEP 1")
        })?;
        // Closes the note (middle-click), so it must come before the settings block and after every view that shows it.
        self.view("graph-closed", |s| {
            let (x, y) = s.center(NOTE, "top", None)?;
            send!(s, format!("CLICK {x} {y} middle"), "SLEEP 1")?;
            s.settle()?;
            let button = s.state()?.recently_closed_name();
            s.click_named(&button)?;
            s.shot("graph-closed")?;
            s.reset()
        })?;
        self.settings_views()?;
        // A toast, from opening a session that doesn't exist (which also leaves the settings page).
        self.view("toast", |s| {
            send!(s, "OPENSESSION nosuch", "SLEEP 1")?;
            s.shot("toast")
        })?;
        // Restored cards whose files are gone (an image and a JSON file): save a session with them, delete the files,
        // open it again. Before json-zoom, which must run last.
        self.view("missing-file", |s| {
            let (pic, data) = (work.join("gone.png"), work.join("gone.json"));
            fs::copy(seed.join("pic.png"), &pic)?;
            append(&pic, b"x")?;
            fs::copy(seed.join("data.json"), &data)?;
            append(&data, b" ")?;
            let session = work.join("missing.mnf");
            send!(
                s,
                format!("ADDFILE {} 100 300", pic.display()),
                format!("ADDFILE {} 400 300", data.display()),
                "SLEEP 2",
                format!("EXPORTSESSION {}", session.display()),
                "SLEEP 1"
            )?;
            s.state()?;
            fs::remove_file(&pic)?;
            fs::remove_file(&data)?;
            send!(
                s,
                format!("IMPORTSESSION {}", session.display()),
                "SLEEP 1",
                "CONFIRMIMPORT",
                "SLEEP 4",
                "MOUSEMOVE 5 790",
                "SLEEP 1"
            )?;
            s.shot("missing-file")
        })?;
        if self.has_any(&["download-real", "download-progress", "download-failed", "download-resume"]) {
            self.step("downloads", Self::download_views)?;
        }
        self.view("json-zoom", Self::json_zoom)?;
        quit(&mut self.inst, "chrome-shots");
        if self.has_any(&["engine-states-picker", "engine-states-settings", "engine-install-confirm"]) {
            self.step("engine-states", |s| s.engine_states(exe))?;
        }
        Ok(())
    }

    /// Right-click on empty canvas.
    fn menu_at_empty(&mut self) -> io::Result<()> {
        send!(self, "MOUSEMOVE 700 150", "MOUSEDOWN 700 150 right", "MOUSEUP 700 150 right", "SLEEP 1")
    }

    fn graph_menus(&mut self) -> io::Result<()> {
        self.view("graph-hover", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            send!(s, format!("MOUSEMOVE {x} {y}"), "SLEEP 1")?;
            s.shot("graph-hover")?;
            send!(s, "MOUSEMOVE 5 790")
        })?;
        for (kind, ty, loaded) in [("tab", TAB, Some(true)), ("group", GROUP, None), ("note", NOTE, None)] {
            let name = format!("graph-menu-{kind}");
            self.view(&name, |s| {
                let (x, y) = s.center(ty, "top", loaded)?;
                s.menu_at(x, y)?;
                s.shot(&name)?;
                s.reset()
            })?;
        }
        // The Jar page: two named jars exist and the focused tab uses the second, so it lists three rows (Default and
        // both jars) with the current one selected.
        self.view("graph-menu-jars", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            s.make_jars(x, y)?;
            s.menu_at(x, y)?;
            send!(s, "MENUPAGE jar", "SLEEP 1")?;
            s.shot("graph-menu-jars")?;
            s.reset()
        })?;
        // A right-click on the jar chip: the menu opens straight on the Jar page, with no MENUPAGE.
        self.view("graph-menu-chip", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            s.make_jars(x, y)?;
            let (cx, cy) = s.center(TAB, "chip", Some(true))?;
            s.menu_at(cx, cy)?;
            s.shot("graph-menu-chip")?;
            s.reset()
        })?;
        // The group's Default jar page with those jars: none chosen (so Default is the selected row).
        self.view("graph-menu-group-jars", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            s.make_jars(x, y)?;
            let (gx, gy) = s.center(GROUP, "top", None)?;
            s.menu_at(gx, gy)?;
            send!(s, "MENUPAGE jar", "SLEEP 1")?;
            s.shot("graph-menu-group-jars")?;
            s.reset()
        })
    }

    /// Links: the top-level tab linked to the note (a dashed line between the cards), the line hovered, then the
    /// tab's menu with its Links section. UNLINK after, so later views have no link. Then connect-drag: the tab
    /// hovered shows its four edge handles; a drag from its left handle 4/5 of the way to the note's centre (the
    /// rubber band, and the note's target ring bright under the pointer) is shot mid-drag, and Esc cancels it before
    /// the button comes up, so nothing is linked.
    fn link_views(&mut self) -> io::Result<()> {
        if self.has_any(&["graph-link", "graph-link-hover", "graph-menu-links"]) {
            self.step("links", |s| {
                let (tab, note) = (s.card_id(TAB)?, s.card_id(NOTE)?);
                send!(s, format!("LINK {tab} {note}"), "SLEEP 1")?;
                if s.has("graph-link") {
                    s.shot("graph-link")?;
                }
                // The pointer on the link line: the line drawn thicker (the cursor, a pointing hand, is not in a screenshot).
                if s.has("graph-link-hover") {
                    let st = s.state()?;
                    let edge = st.0["edges"]
                        .as_array()
                        .and_then(|es| es.iter().find(|e| e["kind"] == "link"))
                        .ok_or_else(|| io::Error::other("no link edge"))?;
                    let (lx, ly) = (
                        round_half_even(edge["screen"][0].as_f64().unwrap_or(0.0)),
                        round_half_even(edge["screen"][1].as_f64().unwrap_or(0.0)),
                    );
                    send!(s, format!("MOUSEMOVE {lx} {ly}"), "SLEEP 1")?;
                    s.shot("graph-link-hover")?;
                    s.reset()?;
                }
                if s.has("graph-menu-links") {
                    let (x, y) = s.center(TAB, "top", Some(true))?;
                    s.menu_at(x, y)?;
                    s.shot("graph-menu-links")?;
                    s.reset()?;
                }
                send!(s, format!("UNLINK {tab} {note}"), "SLEEP 1")
            })?;
        }
        if self.has_any(&["graph-link-handles", "graph-link-drag"]) {
            self.step("link handles", |s| {
                let tab = s.card_id(TAB)?;
                let (tx, ty) = s.center(TAB, "top", Some(true))?;
                send!(s, format!("MOUSEMOVE {tx} {ty}"), "SLEEP 1")?;
                let st = s.state()?;
                let r = st.0["link_handles"][&tab]["left"]
                    .as_array()
                    .filter(|a| a.len() == 4)
                    .ok_or_else(|| io::Error::other("no left link handle"))?;
                let f = |i: usize| r[i].as_f64().unwrap_or(0.0);
                let (hx, hy) =
                    (round_half_even((f(0) + f(2)) / 2.0) as i64, round_half_even((f(1) + f(3)) / 2.0) as i64);
                if s.has("graph-link-handles") {
                    send!(s, format!("MOUSEMOVE {hx} {hy}"), "SLEEP 1")?;
                    s.shot("graph-link-handles")?;
                }
                if s.has("graph-link-drag") {
                    let (nx, ny) = s.center(NOTE, "top", None)?;
                    send!(
                        s,
                        format!("MOUSEMOVE {tx} {ty}"),
                        format!("MOUSEMOVE {hx} {hy}"),
                        format!("MOUSEMOVE {hx} {hy}"),
                        format!("MOUSEDOWN {hx} {hy}")
                    )?;
                    for i in 1..=8 {
                        send!(s, format!("MOUSEMOVE {} {}", hx + (nx - hx) * i * 4 / 40, hy + (ny - hy) * i * 4 / 40))?;
                    }
                    send!(s, "SLEEP 1")?;
                    s.shot("graph-link-drag")?;
                    send!(
                        s,
                        "ESCAPE",
                        format!("MOUSEUP {} {}", hx + (nx - hx) * 4 / 5, hy + (ny - hy) * 4 / 5),
                        "SLEEP 1"
                    )?;
                }
                s.reset()
            })?;
        }
        // Dissolve Group from the menu, then the toast's Undo button: the members are back in the same group.
        self.view("graph-dissolve-undo", |s| {
            let (x, y) = s.center(GROUP, "top", None)?;
            let gid = s.card_id(GROUP)?;
            // The mirror's own groups (a collapsed group's members are not drawn at this level).
            let members = |s: &mut Self| -> io::Result<usize> {
                Ok(s.state()?.0["mirror_groups"]
                    .as_object()
                    .map_or(0, |m| m.values().filter(|g| g.as_str() == Some(gid.as_str())).count()))
            };
            let closed = |s: &mut Self| -> io::Result<bool> {
                Ok(s.state()?.0["closed"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|id| id.as_str() == Some(gid.as_str()))))
            };
            let before = members(s)?;
            // The menu's second-last row is "Dissolve Group" (the last is "Close Group and Contents").
            s.menu_at(x, y)?;
            let menu = s.popup_rect()?.ok_or_else(|| io::Error::other("no group menu"))?;
            s.click((menu[0] + 60.0) as i64, (menu[3] - 44.0) as i64)?;
            s.shot("graph-dissolve-undo-toast")?;
            check(
                "Dissolve Group takes the members out and hides the group",
                before > 0 && members(s)? == 0 && closed(s)?,
            );
            // The toast carries the Undo button at its right end.
            let toast = s.state()?.0["popups"]
                .as_array()
                .and_then(|list| list.iter().find(|p| p["id"].as_str().is_some_and(|id| id.contains("save_toast"))))
                .and_then(|p| {
                    Some([
                        p["rect"][0].as_f64()?,
                        p["rect"][1].as_f64()?,
                        p["rect"][2].as_f64()?,
                        p["rect"][3].as_f64()?,
                    ])
                })
                .ok_or_else(|| io::Error::other("no toast"))?;
            s.click((toast[2] - 28.0) as i64, ((toast[1] + toast[3]) / 2.0) as i64)?;
            s.settle()?;
            s.shot("graph-dissolve-undo-back")?;
            check("Undo brings the same group back around its members", !closed(s)? && members(s)? == before);
            s.reset()
        })?;
        // A card dragged over the group says what the drop would do ("Move into ...") beside the pointer.
        self.view("graph-drop-hint", |s| {
            let (nx, ny) = s.center(NOTE, "top", None)?;
            let (gx, gy) = s.center(GROUP, "top", None)?;
            send!(s, format!("MOUSEMOVE {nx} {ny}"), format!("MOUSEMOVE {nx} {ny}"), format!("MOUSEDOWN {nx} {ny}"))?;
            let toward = |i: i64| format!("MOUSEMOVE {} {}", nx + (gx - nx) * i / 10, ny + (gy - ny) * i / 10);
            for i in 1..=10 {
                send!(s, toward(i))?;
            }
            send!(s, "SLEEP 1")?;
            s.shot("graph-drop-hint")?;
            // Back to where it started, so the release moves nothing.
            for i in (0..10).rev() {
                send!(s, toward(i))?;
            }
            send!(s, format!("MOUSEUP {nx} {ny}"), "SLEEP 1")?;
            s.reset()
        })?;
        Ok(())
    }

    fn keyboard_views(&mut self) -> io::Result<()> {
        // A click focuses the group card without entering it; Enter then does what a double-click does.
        self.view("graph-enter-group", |s| {
            s.tab_presses(knob("GROUP_STOP", 12), "0.5")?;
            send!(s, "SLEEP 1")?;
            s.shot("graph-enter-group-click")?;
            send!(s, "ENTER", "SLEEP 0.4")?;
            s.shot("graph-enter-group-early")?;
            send!(s, "SLEEP 3")?;
            s.shot("graph-enter-group")?;
            s.reset()
        })?;
        // Tab to the JSON card and Enter: the level opens with its root card focused.
        self.view("graph-enter-json", |s| {
            s.tab_presses(knob("JSON_ENTRY_STOP", 16), "0.5")?;
            send!(s, "ENTER", "SLEEP 3")?;
            s.shot("graph-enter-json")?;
            s.reset()
        })?;
        // The menu opens on the focused card, so the card's Tab stop is found by trying each stop (as `note-key` does).
        self.view("graph-menu-key", |s| {
            let mut found = 0;
            for stop in 1..=20 {
                s.reset()?;
                s.tab_presses(stop, "0.2")?;
                send!(s, "SHIFTF10", "SLEEP 1")?;
                if s.menu_open()? {
                    found = stop;
                    break;
                }
            }
            check(&format!("Shift+F10 on a Tab-focused card opens its menu (Tab stop {found})"), found != 0);
            s.shot("graph-menu-key-open")?;
            send!(s, "TABKEY", "SLEEP 0.5")?;
            s.shot("graph-menu-key-tab")?;
            // Still open: Down through the rows.
            // A shot after each Down: the frame it forces is what lets the next key see the new focus.
            for i in 1..=5 {
                send!(s, "ARROWDOWN", "SLEEP 0.3")?;
                s.shot(&format!("graph-menu-key-down{i}"))?;
            }
            send!(s, "ESCAPE", "SLEEP 1")?;
            s.shot("graph-menu-key-closed")?;
            s.reset()
        })?;
        // Space on a Tab-focused tab card selects it (and Space again clears it).
        self.view("graph-select-key", |s| {
            s.tab_presses(knob("SELECT_STOP", 12), "0.5")?;
            send!(s, "SPACEKEY", "SLEEP 1")?;
            s.shot("graph-select-key-on")?;
            send!(s, "SPACEKEY", "SLEEP 1")?;
            s.shot("graph-select-key-off")?;
            // Two more cards selected: the selection bar sits on a box around both.
            send!(s, "TABKEY", "SLEEP 0.5", "SPACEKEY", "SLEEP 0.5", "TABKEY", "SLEEP 0.5", "SPACEKEY", "SLEEP 1")?;
            s.shot("graph-select-key-two")?;
            // Pinned: the bar's pin goes accent, and each card keeps its passive pin mark.
            send!(s, "TOGGLEPIN", "SLEEP 1")?;
            s.shot("graph-select-key-pinned")?;
            s.reset()
        })?;
        // Over `LOD_SIMPLIFY_COUNT` cards the graph draws simple: a pinned card shows its accent dot, not the glyph.
        self.view("graph-pinned-dense", |s| {
            for _ in 0..70 {
                send!(s, "NEWNOTE")?;
            }
            send!(s, "SLEEP 3")?;
            s.tab_presses(1, "0.5")?;
            send!(s, "SPACEKEY", "SLEEP 0.5", "TOGGLEPIN", "SLEEP 1")?;
            for _ in 0..6 {
                s.inst.click_control(crate::state::ZOOM_OUT)?;
                send!(s, "SLEEP 0.5")?;
            }
            send!(s, "SLEEP 1")?;
            s.shot("graph-pinned-dense")?;
            s.reset()
        })?;
        // Tab to the Note card and Enter: the note starts editing; Escape ends it and Enter edits again (focus
        // returned to the card). The note's Tab stop is found by trying each stop.
        self.view("note-key", |s| {
            let mut found = 0;
            for stop in 1..=20 {
                s.reset()?;
                s.tab_presses(stop, "0.2")?;
                send!(s, "ENTER", "SLEEP 1")?;
                if s.editing()? {
                    found = stop;
                    break;
                }
            }
            if found == 0 {
                check("Enter on a focused Note card starts editing (no Tab stop did)", false);
            } else {
                check(&format!("Enter on the Note card (Tab stop {found}) starts editing"), true);
                s.shot("note-key-editing")?;
                send!(s, "ESCAPE", "SLEEP 1")?;
                check("Escape ends editing", !s.editing()?);
                send!(s, "ENTER", "SLEEP 1")?;
                check("Enter edits again, so focus returned to the card", s.editing()?);
            }
            s.reset()
        })?;
        // Enter the JSON level by keyboard in a small window (so some cards are off screen) and Tab through every
        // card after the root: each Tab must select a new card, off-screen ones included (zoomed in so they are).
        self.view("json-tab-offscreen", |s| {
            let sel = |s: &mut Self| -> io::Result<(String, usize)> {
                let st = s.state()?;
                let j = &st.0["json_level"];
                Ok((j["selected"].to_string(), j["cards"].as_array().map_or(0, Vec::len)))
            };
            s.tab_presses(knob("JSON_ENTRY_STOP", 16), "0.3")?;
            send!(s, "ENTER", "SLEEP 3")?;
            // Zoomed in, the cards other than the root fall off screen.
            send!(
                s,
                "ZOOMIN",
                "ZOOMIN",
                "ZOOMIN",
                "MOUSEMOVE 700 650",
                "MOUSEDOWN 700 650",
                "MOUSEMOVE 400 650",
                "MOUSEMOVE 100 650",
                "MOUSEUP 100 650",
                "MOUSEMOVE 100 650",
                "MOUSEMOVE 5 790",
                "SLEEP 2"
            )?;
            s.settle()?;
            s.shot("json-tab-offscreen-before")?;
            let total = sel(s)?.1;
            let mut seen = Vec::new();
            for _ in 1..total {
                send!(s, "TABKEY", "SLEEP 1.5")?;
                s.settle()?;
                seen.push(sel(s)?.0);
            }
            println!("selected after each Tab: {} (cards: {total})", seen.join(" "));
            seen.sort();
            seen.dedup();
            check(
                &format!("Tab reached {} of {} JSON cards after the root", seen.len(), total.saturating_sub(1)),
                seen.len() >= total.saturating_sub(1),
            );
            s.shot("json-tab-offscreen")?;
            s.reset()
        })?;
        // Digits jump to the card with that Tab-order number (the Note is 3 of the 5 seeded cards): 3 focuses it, so
        // Enter starts editing.
        self.view("digit-jump", |s| {
            s.reset()?;
            send!(s, "DIGITKEY 3", "SLEEP 1")?;
            s.shot("digit-jump")?;
            send!(s, "ENTER", "SLEEP 1")?;
            check("digit 3 focuses card 3 (the note), Enter edits it", s.editing()?);
            s.reset()
        })
    }

    /// Mouse behaviour that the Tab-focus model must not disturb, asserted rather than shot: dragging a card moves
    /// it, middle-click closes it.
    fn mouse_regress(&mut self) -> io::Result<()> {
        let note_rect = |s: &mut Self| -> io::Result<[i64; 4]> {
            let st = s.state()?;
            let c = st
                .all_cards()
                .iter()
                .find(|c| State::card_is(c, NOTE, None))
                .ok_or_else(|| io::Error::other("no note"))?;
            Ok(rect(c)?.map(|v| v as i64))
        };
        let [x0, y0, x1, y1] = note_rect(self)?;
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        send!(
            self,
            format!("MOUSEMOVE {cx} {cy}"),
            format!("MOUSEDOWN {cx} {cy}"),
            format!("MOUSEMOVE {} {}", cx + 30, cy + 15),
            format!("MOUSEMOVE {} {}", cx + 60, cy + 30),
            format!("MOUSEMOVE {} {}", cx + 90, cy + 45),
            format!("MOUSEUP {} {}", cx + 90, cy + 45),
            "SLEEP 1"
        )?;
        let [n0, m0, n1, m1] = note_rect(self)?;
        check_eq("drag moves the card by the drag distance", "90,45".to_string(), format!("{},{}", n0 - x0, m0 - y0));
        let before = self.card_count()?;
        let (mx, my) = ((n0 + n1) / 2, (m0 + m1) / 2);
        send!(
            self,
            format!("MOUSEMOVE {mx} {my}"),
            format!("MOUSEDOWN {mx} {my} middle"),
            format!("MOUSEUP {mx} {my} middle"),
            "SLEEP 1"
        )?;
        check_eq("middle-click closes the card", before - 1, self.card_count()?);
        self.reset()
    }

    fn json_views(&mut self) -> io::Result<()> {
        let names = ["json-level", "json-menu", "json-tooltip", "json-keys", "hover-json", "hover-json-sel"];
        if !self.has_any(&names) {
            return Ok(());
        }
        self.step("json level", |s| {
            let (x, y) = s.center(JSON, "top", None)?;
            send!(s, format!("DOUBLECLICK {x} {y}"), "SLEEP 3")?;
            if s.has("json-level") {
                s.shot("json-level")?;
            }
            if s.has("hover-json") {
                s.hover_scan("json", knob("HOVER_Y", 16), knob("HOVER_BAND", 34))?;
            }
            // With a card selected, the bar's Copy buttons are enabled.
            if s.has("hover-json-sel") {
                send!(s, "CLICK 280 380", "SLEEP 1")?;
                s.hover_scan("json-sel", knob("HOVER_Y", 16), knob("HOVER_BAND", 34))?;
            }
            // Copy value is disabled until a row is selected, and a disabled widget shows no tooltip: select the root
            // (JSONSELECT with no pointer), hover the button by name, and Escape clears the selection after.
            if s.has("json-tooltip") {
                send!(s, "JSONSELECT", "SLEEP 1")?;
                s.inst.hover_control(COPY_VALUE)?;
                send!(s, "SLEEP 2")?;
                s.shot("json-tooltip")?;
                send!(s, "MOUSEMOVE 5 790", "ESCAPE", "SLEEP 1")?;
            }
            // Keys: Down x3 selects the third outline row, Enter toggles it, Right opens it, Up+Left+Left closes then
            // selects the parent, Tab focuses a card.
            if s.has("json-keys") {
                send!(s, "ARROWDOWN", "ARROWDOWN", "ARROWDOWN", "SLEEP 1")?;
                s.shot("json-keys-down")?;
                send!(s, "ENTER", "SLEEP 1")?;
                s.shot("json-keys-enter")?;
                send!(s, "ARROWRIGHT", "SLEEP 1")?;
                s.shot("json-keys-right")?;
                send!(s, "ARROWUP", "ARROWLEFT", "ARROWLEFT", "SLEEP 1")?;
                s.shot("json-keys-left")?;
                s.tab_walk("json", knob("HOVER_BAND", 34))?;
            }
            // Right-click the first card the level lays out (data.json at 1280x800).
            if s.has("json-menu") {
                s.settle()?;
                send!(s, "MOUSEMOVE 280 380", "MOUSEDOWN 280 380 right", "MOUSEUP 280 380 right", "SLEEP 1")?;
                s.shot("json-menu")?;
                send!(s, "ESCAPE")?;
            }
            s.reset()
        })
    }

    fn tabview_views(&mut self) -> io::Result<()> {
        let names = [
            "tabview",
            "hover-tabview",
            "tab-tabview",
            "tabview-dropdown",
            "tabview-find",
            "tabview-actions",
            "tabview-actions-click",
            "tabview-split",
            "tabview-engine",
            "hover-find",
            "hover-dropdown",
            "tab-find",
            "tab-dropdown",
            "tabview-split-key",
        ];
        if !self.has_any(&names) {
            return Ok(());
        }
        let band = knob("HOVER_BAND", 34);
        self.step("tab view", |s| {
            s.open_tab()?;
            if s.has("tabview") {
                s.shot("tabview")?;
            }
            if s.has("tab-tabview") {
                s.tab_scan("tabview", 12, band)?;
            }
            if s.has("hover-tabview") {
                s.hover_scan("tabview", knob("HOVER_Y", 16), band)?;
            }
            if s.has("tabview-dropdown") {
                send!(s, "ADDRESSBAR exam", "SLEEP 2")?;
                s.shot("tabview-dropdown")?;
                send!(s, "ESCAPE", "SLEEP 1")?;
            }
            if s.has("tab-dropdown") {
                send!(s, "ADDRESSBAR exam", "SLEEP 2")?;
                s.tab_scan("dropdown", 4, 820)?;
                send!(s, "ESCAPE", "SLEEP 1")?;
            }
            if s.has("hover-dropdown") {
                send!(s, "ADDRESSBAR exam", "SLEEP 2")?;
                s.hover_column("dropdown", 400, 40, 250, 820, 210)?;
                send!(s, "ESCAPE", "SLEEP 1")?;
            }
            if s.has("tabview-find") {
                send!(s, "OPENFINDBAR", "FINDTEXT abc", "SLEEP 1")?;
                s.shot("tabview-find")?;
                send!(s, "CLOSEFINDBAR")?;
            }
            if s.has("tabview-actions") {
                s.click_named(TAB_ACTIONS)?;
                s.shot("tabview-actions")?;
                send!(s, "ESCAPE")?;
            }
            if s.has("hover-find") {
                send!(s, "OPENFINDBAR", "FINDTEXT abc", "SLEEP 1")?;
                s.hover_scan("find", knob("FIND_Y", 60), knob("FIND_BAND", 90))?;
                send!(s, "CLOSEFINDBAR")?;
            }
            if s.has("tab-find") {
                send!(s, "OPENFINDBAR", "FINDTEXT abc", "SLEEP 1")?;
                s.tab_scan("find", 6, knob("FIND_BAND", 90))?;
                send!(s, "CLOSEFINDBAR")?;
            }
            // The Split button opens the engine picker.
            if s.has("tabview-engine") {
                s.click_named(SPLIT_VIEW)?;
                s.shot("tabview-engine")?;
                send!(s, "ESCAPE", "SLEEP 1")?;
            }
            // Keyboard-only pick from the Split engine picker: it opens on the selected card, Down moves, Enter picks
            // and opens the Split pane.
            if s.has("tabview-split-key") {
                s.click_named(SPLIT_VIEW)?;
                s.shot("tabview-split-key-open")?;
                send!(s, "ARROWDOWN", "SLEEP 1")?;
                s.shot("tabview-split-key-down")?;
                send!(s, "ENTER", "SLEEP 3")?;
                s.shot("tabview-split-key")?;
                send!(s, "CLOSESPLIT", "SLEEP 1")?;
            }
            if s.has("tabview-split") {
                send!(s, "SPLIT webview2", "SLEEP 3")?;
                s.shot("tabview-split")?;
                send!(s, "CLOSESPLIT", "SLEEP 1")?;
            }
            // Asserted: the popup's Pin row pins the focused tab, its Duplicate row adds a card, its Close Tab row
            // removes one. Last, because Close Tab ends the Tab view and a view after it would find no toolbar to click.
            if s.has("tabview-actions-click") {
                let pinned = |s: &mut Self| -> io::Result<usize> {
                    Ok(s.state()?.cards().iter().filter(|c| c["pinned"].as_bool() == Some(true)).count())
                };
                let (pins0, cards0) = (pinned(s)?, s.card_count()?);
                s.click_named(TAB_ACTIONS)?;
                s.click_named(TAB_ACTIONS_PIN)?;
                check("Tab actions > Pin pins the tab", pinned(s)? == pins0 + 1);
                s.click_named(TAB_ACTIONS)?;
                s.inst.click_control(TAB_ACTIONS_DUPLICATE)?;
                send!(s, "SLEEP 2")?;
                check("Tab actions > Duplicate adds a card", s.card_count()? == cards0 + 1);
                s.click_named(TAB_ACTIONS)?;
                s.inst.click_control(TAB_ACTIONS_CLOSE_TAB)?;
                send!(s, "SLEEP 2")?;
                let now = s.card_count()?;
                check(&format!("Tab actions > Close Tab removes the tab ({now} vs {cards0})"), now == cards0);
            }
            s.reset()
        })
    }

    fn settings_views(&mut self) -> io::Result<()> {
        let names = [
            "settings",
            "hover-settings",
            "tab-settings",
            "settings-engine",
            "settings-engine-tab",
            "settings-combo",
            "popup-bookmarks",
            "popup-history",
            "popup-downloads",
            "tab-popup",
            "tab-popup-history",
            "tab-popup-downloads",
            "tab-combo",
            "tab-engine",
        ];
        if !self.has_any(&names) {
            return Ok(());
        }
        let data = self.seed.join("data.json");
        self.step("settings", |s| {
            // Taller, so a popup opening below the Lists buttons is not pushed up over them.
            send!(s, format!("RESIZE {} 1000", s.size.0), "TOGGLESETTINGSPAGE", "SLEEP 2")?;
            if s.has("popup-downloads") {
                send!(s, format!("ADDDOWNLOAD {}", data.display()), "SLEEP 1")?;
            }
            send!(s, "SESSIONNAMETEXT demo", "SAVESESSIONAS demo", "SLEEP 1")?;
            if s.has("settings") {
                s.shot("settings")?;
            }
            if s.has("tab-settings") {
                s.tab_scan("settings", 24, 1000)?;
            }
            if s.has("hover-settings") {
                s.hover_column("settings", 40, 44, 960, 520, 0)?;
            }
            // The engine pickers and the search-engine combo, clicked by name: rows above them move them.
            for (view, control) in [
                ("settings-engine", DEFAULT_ENGINE),
                ("settings-engine-tab", "Engine"),
                ("settings-combo", SEARCH_ENGINE),
            ] {
                if s.has(view) {
                    s.inst.click_control(control)?;
                    send!(s, "SLEEP 1")?;
                    s.shot(view)?;
                    send!(s, AWAY, "SLEEP 1")?;
                }
            }
            for (view, control, name) in
                [("tab-engine", DEFAULT_ENGINE, "engine"), ("tab-combo", SEARCH_ENGINE, "combo")]
            {
                if s.has(view) {
                    s.inst.click_control(control)?;
                    send!(s, "SLEEP 1")?;
                    s.tab_scan(name, 5, 1000)?;
                    send!(s, AWAY, "SLEEP 1")?;
                }
            }
            // The Lists popups, opened by name (the buttons' place on the page moves with the sections above them).
            for (view, name) in
                [("popup-bookmarks", "bookmarks"), ("popup-history", "history"), ("popup-downloads", "downloads")]
            {
                if s.has(view) {
                    send!(s, format!("TOGGLEPOPUP {name}"), "SLEEP 1")?;
                    s.shot(view)?;
                    if view == "popup-history" {
                        // The filter narrows the list, and Down from the field focuses the first match.
                        // The filter field is the popup's first row.
                        let Some([x0, y0, ..]) = s.popup_rect()? else {
                            check("The History popup is open", false);
                            return s.reset();
                        };
                        send!(s, format!("CLICK {} {}", x0 + 40.0, y0 + 13.0), "SLEEP 1", "EGUITEXT wiki", "SLEEP 1")?;
                        // A shot forces the frame that `controls` is read from (see PARKED.md, headless debug keys).
                        s.shot("popup-history-typed")?;
                        let st = s.state()?;
                        let row = |name: &str| st.popup_controls().into_iter().find(|c| c["name"] == name).cloned();
                        check(
                            "History filter 'wiki' keeps Wikipedia and drops Hacker News",
                            row("Wikipedia").is_some() && row("Hacker News").is_none(),
                        );
                        send!(s, "ARROWDOWN", "SLEEP 1")?;
                        s.shot("popup-history-filtered")?;
                        let st = s.state()?;
                        let selected =
                            st.popup_controls().iter().any(|c| c["name"] == "Wikipedia" && c["selected"] == true);
                        check("Down from the History filter focuses the first match", selected);
                    }
                    send!(s, AWAY, "SLEEP 1")?;
                }
            }
            for (view, name) in
                [("tab-popup", "bookmarks"), ("tab-popup-history", "history"), ("tab-popup-downloads", "downloads")]
            {
                if s.has(view) {
                    send!(s, format!("TOGGLEPOPUP {name}"), "SLEEP 1")?;
                    s.tab_scan(view.trim_start_matches("tab-"), 6, 1000)?;
                    send!(s, AWAY, "SLEEP 1")?;
                }
            }
            send!(s, "TOGGLESETTINGSPAGE", format!("RESIZE {} {}", s.size.0, s.size.1), "SLEEP 1")
        })
    }

    /// Real downloads into the instance's own folder (`instance::download_dir`), each checked by its exact path: a
    /// local HTTP server answers /real.zip as a small attachment, /slow as a 1 MB one it stalls after 300 KB
    /// (`download-failed` kills the server mid-transfer), and /resume as one an engine can resume after its transfer
    /// breaks; the live tab navigates to it, so the engine starts the download (the card keeps the URL as its source
    /// line). Before json-zoom, which must run last.
    fn download_views(&mut self) -> io::Result<()> {
        let folder = self.downloads.clone();
        // A rerun in the same work folder starts from an empty one, so every name below is exact.
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        let at = |name: &str| folder.join(name).to_string_lossy().into_owned();
        if self.has_any(&["download-progress", "download-failed"]) {
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, slow) = (server.url("/slow"), at("slow.zip"));
            // Still arriving: 300 KB of 1 MB. The fit button frames the new card, which opens above the tab's.
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER", "SLEEP 6")?;
            let named = |s: &State| matches!(s.downloads_from(&url)[..], [d] if d["path"] == slow.as_str());
            let state = self.inst.export_until("download-progress", named)?;
            if !named(&state) {
                return Err(io::Error::other(format!(
                    "the download of {url} is not one card saving to {slow}: {:?}",
                    state.downloads_from(&url)
                )));
            }
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            if self.has("download-progress") {
                self.shot("download-progress")?;
            }
            if self.has("download-failed") {
                // The server dies mid-transfer. WebView2 retries the download as a second one, which its host gives
                // the first one's path (`retry_path`) and which fails once the retry is refused (runs
                // 2026-10-27-dl-probe-*); the card shows it failed for the network (`NETWORK_FAILED`, run
                // 2026-10-27-dl-probe-hold), and the shot waits for its toast to go.
                server.kill();
                // The download cards' names as a screen reader hears them (`controls`): kind, title, then the tooltip.
                let card_names = |s: &State| -> Vec<String> {
                    s.controls().into_iter().map(|c| c.name).filter(|n| n.starts_with("Download:")).collect()
                };
                let failed = |s: &State| {
                    matches!(s.downloads_from(&url)[..], [d]
                        if d["failed"] == true && d["path"] == slow.as_str() && d["failure"] == "Network")
                        && s.toast().is_none()
                        && card_names(s).iter().any(|n| {
                            n.starts_with("Download: slow.zip. ") && n.contains("The connection failed or timed out")
                        })
                };
                let state = self.inst.export_until("download-failed", failed)?;
                if !failed(&state) {
                    return Err(io::Error::other(format!(
                        "the one card downloading {url} to {slow} did not show why the download failed (toast {:?}, \
                         card names {:?}): {:?}",
                        state.toast(),
                        card_names(&state),
                        state.downloads_from(&url)
                    )));
                }
                send!(self, "MOUSEMOVE 5 780", "SLEEP 1")?;
                self.shot("download-failed")?;
            }
        }
        if self.has("download-real") {
            // A server of its own: `download-failed` killed the first.
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, real) = (server.url("/real.zip"), at("real.zip"));
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER", "SLEEP 6")?;
            // Framed as before (S16): the tab six seconds after the download started.
            self.shot("download-real-tab")?;
            let saved = |s: &State| {
                matches!(s.downloads_from(&url)[..], [d] if d["completed"] == true && d["path"] == real.as_str())
                    && s.toast().is_none()
            };
            let state = self.inst.export_until("download-real", saved)?;
            if !saved(&state) {
                return Err(io::Error::other(format!(
                    "the download of {url} is not one completed card saving to {real}: {:?}",
                    state.downloads_from(&url)
                )));
            }
            let bytes = fs::read(folder.join("real.zip")).unwrap_or_default();
            if bytes != b"data" {
                return Err(io::Error::other(format!("{real} holds {} bytes, not the server's 4", bytes.len())));
            }
            // The card opens above the tab's, off the frame an earlier step's fit left.
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            self.shot("download-real")?;
        }
        if self.has("download-resume") {
            // A file the engine can resume: its transfer breaks, the engine asks for the rest with a `Range` request,
            // and the WebView2 host gives that retry the first one's path (`retry_path`), so one card completes one
            // whole file.
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, resume) = (server.url("/resume"), at("resume.zip"));
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER")?;
            let arriving = |s: &State| s.downloads_from(&url).iter().any(|d| d["bytes"].as_u64() > Some(0));
            let state = self.inst.export_until("download-resume-arriving", arriving)?;
            if !arriving(&state) {
                return Err(io::Error::other(format!("no bytes of {url} arrived: {:?}", state.downloads_from(&url))));
            }
            // The connection drops while the engine still expects the rest; the server keeps answering.
            server.break_transfers();
            let settled = |s: &State| {
                let cards = s.downloads_from(&url);
                cards.len() > 1
                    || (cards.iter().any(|d| d["completed"] == true || d["failed"] == true) && s.toast().is_none())
            };
            let state = self.inst.export_until("download-resume", settled)?;
            let cards = state.downloads_from(&url);
            if !matches!(cards[..], [d] if d["completed"] == true && d["path"] == resume.as_str()) {
                return Err(io::Error::other(format!(
                    "the resumed download of {url} is not one completed card saving to {resume} ({} ranges answered): \
                     {cards:?}",
                    server.ranges_answered()
                )));
            }
            let saved = fs::read(folder.join("resume.zip")).unwrap_or_default();
            let sent = resume_body();
            if saved != sent {
                return Err(io::Error::other(format!(
                    "{resume} is not the file the server sent ({} of {} bytes)",
                    saved.len(),
                    sent.len()
                )));
            }
            if server.ranges_answered() == 0 {
                return Err(io::Error::other(format!(
                    "the engine never asked for the rest of {url} with an open-ended Range (`bytes=N-`, the only form \
                     shots_net answers with a 206)"
                )));
            }
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            self.shot("download-resume")?;
        }
        Ok(())
    }

    /// Last, because it leaves the graph zoomed (the toolbar's fit button does not restore it). The Json card
    /// preview's header stays 40 points while its rows get more or less of the card: the toolbar's zoom-out button
    /// twice (fewer rows fit under the header), then back in (the graph zoom, not the page zoom `ZOOMIN` sets).
    fn json_zoom(&mut self) -> io::Result<()> {
        self.click_named(crate::state::ZOOM_OUT)?;
        self.click_named(crate::state::ZOOM_OUT)?;
        send!(self, "MOUSEMOVE 5 790", "SLEEP 2")?;
        self.shot("json-zoom-out")?;
        // Further out: the Note card under 80 points tall, so it falls back to the 18-point name strip.
        for _ in 0..3 {
            self.click_named(crate::state::ZOOM_OUT)?;
        }
        send!(self, "MOUSEMOVE 5 790", "SLEEP 2")?;
        self.shot("note-short")?;
        for _ in 0..9 {
            self.click_named(crate::state::ZOOM_IN)?;
        }
        // In to ~280%, then drag the empty canvas up so the Json card (below the tab) is on screen.
        send!(
            self,
            "MOUSEMOVE 900 760",
            "MOUSEDOWN 900 760 left",
            "MOUSEMOVE 900 660 left",
            "MOUSEMOVE 900 560 left",
            "MOUSEUP 900 560 left",
            "MOUSEMOVE 5 790",
            "SLEEP 2"
        )?;
        self.shot("json-zoom-in")
    }

    // ---- engine states -------------------------------------------------------------------------------------------

    /// Restarts the instance on a fresh profile `eng-<name>` (a copy of the exe, the settings page open).
    fn engine_app(&mut self, eng_exe: &Path, proxy: &Proxy, name: &str, more: &str) -> io::Result<()> {
        let root = self.work.join(format!("eng-{name}"));
        let profile = root.join("manifold");
        seed_settings(&profile, "webview2", more)?;
        // etcetera's data dir under this APPDATA.
        self.eng_data = profile.join("manifold").join("data").join(HOSTS_DIR);
        fs::create_dir_all(&self.eng_data)?;
        import_session(
            eng_exe,
            &profile,
            &[
                json!({"id": "t1", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}),
                json!({"id": "t3", "type": TAB, "url": "https://example.com/", "view_kind": "cef", "pos": [300.0, 0.0]}),
            ],
        )?;
        let (appdata, proxy_url, suggest_url) = (root.display().to_string(), proxy.url(), self.suggestions.url());
        let pins_file = self.work.join("host-pins.json").display().to_string();
        self.inst = Instance::launch_env(
            eng_exe,
            &profile,
            &self.work,
            &format!("log-eng-{name}"),
            &[
                ("APPDATA", &appdata),
                ("HTTPS_PROXY", &proxy_url),
                ("MANIFOLD_HOST_PINS_FILE", &pins_file),
                ("MANIFOLD_SUGGESTIONS_URL", &suggest_url),
            ],
        )?;
        send!(self, format!("RESIZE {} 1000", self.size.0), "WAITTABS", "SLEEP 3")
    }

    /// CEF's pinned host files (sparse, at their pinned sizes) and the `cef.host` marker that lists their hashes
    /// (`old`: other hashes, so the host reads Update needed).
    fn fake_host(&self, pins: &[(String, u64, String)], old: bool) -> io::Result<()> {
        let mut marker = String::new();
        for (file, size, sha) in pins {
            fs::File::create(self.eng_data.join(file))?.set_len(*size)?;
            marker.push_str(&format!("{} {file}\n", if old { "0".repeat(64) } else { sha.clone() }));
        }
        fs::write(self.eng_data.join(format!("{}.host", CEF.id)), marker)
    }

    /// A runtime directory with a (sparse) 423 MB libcef.dll and `marker` in `ready`.
    fn fake_runtime(&self, marker: &str) -> io::Result<()> {
        let dir = self.eng_data.join(CEF_RUNTIME_DIR);
        fs::create_dir_all(&dir)?;
        let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
        fs::File::create(dir.join(lib))?.set_len(423 * 1024 * 1024)?;
        fs::write(dir.join("ready"), marker)
    }

    fn engine_stop(&mut self) -> io::Result<()> {
        self.inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    fn engine_states(&mut self, exe: &Path) -> io::Result<()> {
        let (file, size, sha256) = fake_cef_host();
        let pins = vec![(file.clone(), size, sha256.to_string())];
        // The manifest's keys: the host's id, then the platform it is pinned under (the app's parser reads both).
        let (id, platform) = (CEF.id, Os::here().pin_platform());
        fs::write(
            self.work.join("host-pins.json"),
            json!({id: {platform: [{"file": file, "size": size, "sha256": sha256}]}}).to_string(),
        )?;
        let pin_version = pin_version(CEF_PIN).ok_or_else(|| io::Error::other("cef_pin.rs has no version"))?;
        let eng = self.work.join("eng");
        fs::create_dir_all(&eng)?;
        let eng_exe = eng.join("manifold.exe");
        fs::copy(exe, &eng_exe)?;
        let webview2 = WEBVIEW2.file_name(Os::here());
        fs::copy(exe.with_file_name(&webview2), eng.join(&webview2))?;
        let proxy = Proxy::start(true)?;

        self.engine_app(&eng_exe, &proxy, "states", "")?;
        // The seeded CEF tab can't start: its card says so (graph view, then its Tab view).
        self.shot("engine-states-graph")?;
        send!(self, "GRAPHRIGHT", "GRAPHOPEN", "SLEEP 3")?;
        self.shot("engine-states-failed-tab")?;
        send!(self, "TOGGLEVIEW", "SLEEP 1", "TOGGLESETTINGSPAGE", "SLEEP 2")?;
        self.shot("engine-states-settings-notinstalled")?;
        self.picker_shot("engine-states-picker-notinstalled")?;
        // The Engines section's Download button asks first, with the size.
        self.inst.click_control("Download")?;
        send!(self, "SLEEP 1")?;
        self.shot("engine-install-confirm")?;
        // Confirm with the proxy refusing: the install fails and the row says why, with Retry. The dialog is in
        // front, so its own Download button is the one found by that name.
        self.inst.click_control("Download")?;
        send!(self, "SLEEP 4")?;
        self.shot("engine-states-settings-failed")?;
        self.picker_shot("engine-states-picker-failed")?;
        // Retry with the proxy silent: Installing, with Cancel.
        proxy.set_refuse(false);
        self.inst.click_control("Retry")?;
        send!(self, "SLEEP 1")?;
        self.inst.click_control("Download")?;
        send!(self, "SLEEP 2")?;
        self.shot("engine-states-settings-installing")?;
        self.picker_shot("engine-states-picker-installing")?;
        self.engine_stop()?;
        // Files on disk, judged by their marker: this pin's is Installed (size, Remove), another's is Update needed.
        // Fresh instances: a failed install outranks the disk in the state, so they can't follow the one above.
        self.engine_app(&eng_exe, &proxy, "installed", "")?;
        self.fake_host(&pins, false)?;
        self.fake_runtime(&pin_version)?;
        send!(self, "TOGGLESETTINGSPAGE", "SLEEP 2")?;
        self.shot("engine-states-settings-installed")?;
        self.engine_stop()?;
        self.engine_app(&eng_exe, &proxy, "stale", "")?;
        self.fake_host(&pins, true)?;
        self.fake_runtime("old-version")?;
        send!(self, "TOGGLESETTINGSPAGE", "SLEEP 2")?;
        self.shot("engine-states-settings-stale")?;
        self.picker_shot("engine-states-picker-stale")?;
        self.engine_stop()?;
        // Bring-your-own folders: one whose archive.json names this pin (Custom), one that names another build.
        for (name, archive) in [("custom", pin_version.as_str()), ("custom-bad", "151.0.1+gabcdef+chromium-151.0.0.1")]
        {
            // Its path shows on the settings page, so it is named by its files, not kept in the work dir.
            let archive = json!({"type": "minimal", "name": format!("cef_binary_{archive}_windows64_minimal.tar.bz2")})
                .to_string();
            let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
            let dir = content_dir(&std::env::temp_dir(), &[(lib, b""), ("archive.json", archive.as_bytes())])?;
            let more = format!(", \"cef_directory\": {}", Value::String(dir.display().to_string().replace('\\', "/")));
            self.engine_app(&eng_exe, &proxy, name, &more)?;
            self.fake_host(&pins, false)?; // a bring-your-own runtime still needs the downloaded host
            send!(self, "TOGGLESETTINGSPAGE", "SLEEP 2")?;
            self.shot(&format!("engine-states-settings-{name}"))?;
            self.picker_shot(&format!("engine-states-picker-{name}"))?;
            self.engine_stop()?;
        }
        Ok(())
    }

    /// The Default starting engine picker open, shot, closed.
    fn picker_shot(&mut self, name: &str) -> io::Result<()> {
        self.inst.click_control(DEFAULT_ENGINE)?;
        send!(self, "SLEEP 1")?;
        self.shot(name)?;
        send!(self, AWAY, "SLEEP 1")
    }
}

fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    io::Write::write_all(&mut fs::OpenOptions::new().append(true).open(path)?, bytes)
}

fn rect(card: &Value) -> io::Result<[f64; 4]> {
    let a = card["rect"].as_array().filter(|a| a.len() >= 4).ok_or_else(|| io::Error::other("card has no rect"))?;
    let mut out = [0.0; 4];
    for (slot, v) in out.iter_mut().zip(a) {
        *slot = v.as_f64().ok_or_else(|| io::Error::other("card rect is not numbers"))?;
    }
    Ok(out)
}

/// The text between `after` and the next `until` following it.
fn between<'a>(text: &'a str, after: &str, until: &str) -> Option<&'a str> {
    let rest = &text[text.find(after)? + after.len()..];
    Some(&rest[..rest.find(until)?])
}

/// A folder in `root` named after the SHA-256 of `files` (name, contents), holding them. A file whose path shows in a
/// shot goes here rather than in the work dir, whose name holds the pid, so the shot repeats between runs. An
/// existing folder is reused; each file is written under a per-process name and renamed into place, so runs at once
/// never read half a file.
fn content_dir(root: &Path, files: &[(&str, &[u8])]) -> io::Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for (name, bytes) in files {
        for part in [name.as_bytes(), bytes] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
    }
    let hex: String = hash.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect();
    let dir = root.join(format!("gate-shots-{hex}"));
    fs::create_dir_all(&dir)?;
    for (name, bytes) in files {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        let part = dir.join(format!("{name}.{}.part", std::process::id()));
        fs::write(&part, bytes)?;
        // Another run may have renamed its copy in first, and hold it open; theirs is the same bytes.
        if let Err(e) = fs::rename(&part, &path) {
            fs::remove_file(&part)?;
            if !path.exists() {
                return Err(e);
            }
        }
    }
    Ok(dir)
}

/// The CEF version cef_pin.rs pins (its first `version: "..."`).
fn pin_version(src: &str) -> Option<String> {
    between(src, "version: \"", "\"").map(str::to_string)
}

/// The seed's `data.json`, keys in the order written: the JSON views show them, and the bytes name the seed's folder.
fn seed_data() -> String {
    json!({"name": "demo", "items": [1, 2, 3], "nested": {"a": true, "b": null}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same bytes whether gate is built alone or with manifold (whose `preserve_order` used to reach gate's
    /// serde_json only then, so the two builds seeded different folders with differently ordered files).
    #[test]
    fn the_seed_json_keeps_its_key_order() {
        assert_eq!(seed_data(), r#"{"name":"demo","items":[1,2,3],"nested":{"a":true,"b":null}}"#);
    }

    #[test]
    fn the_fixture_host_and_the_repos_cef_pin_parse() {
        let (file, size, sha) = fake_cef_host();
        assert!(file == CEF.file_name(Os::here()) && size > 0 && sha.len() == 64);
        assert!(pin_version(CEF_PIN).is_some_and(|v| v.contains("chromium")));
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("1280 800"), Some((1280, 800)));
        assert_eq!(parse_size(" 400  300 "), Some((400, 300)));
        assert_eq!(parse_size("400"), None);
        assert_eq!(parse_size("1 2 3"), None);
    }

    #[test]
    fn content_dirs_are_named_by_their_files_and_reused() {
        let root = std::env::temp_dir().join(format!("gate-content-dir-test-{}", std::process::id()));
        let a = content_dir(&root, &[("x.txt", b"one"), ("y.txt", b"")]).unwrap();
        assert_eq!(fs::read(a.join("x.txt")).unwrap(), b"one");
        assert_eq!(fs::read(a.join("y.txt")).unwrap(), b"");
        assert_eq!(content_dir(&root, &[("x.txt", b"one"), ("y.txt", b"")]).unwrap(), a, "same files, same folder");
        assert_ne!(content_dir(&root, &[("x.txt", b"two"), ("y.txt", b"")]).unwrap(), a, "new bytes, new folder");
        assert_ne!(content_dir(&root, &[("z.txt", b"one"), ("y.txt", b"")]).unwrap(), a, "new name, new folder");
        assert_eq!(fs::read_dir(&a).unwrap().count(), 2, "no temp files left behind");
        fs::remove_dir_all(&root).unwrap();
    }
}
