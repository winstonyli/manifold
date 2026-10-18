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
//! about 40 s. `SIZE` (`"1280 800"`) is the window size; the clicks on fixed coordinates assume the default. The
//! default views are `DEFAULT_VIEWS`; the rest are opt-in:
//!
//! - hover scans (`hover-graph hover-json hover-json-sel hover-tabview hover-find hover-settings hover-menu-tab
//!   hover-dropdown hover-quickswitch`) and Tab-key scans (`tab-graph tab-tabview tab-settings tab-find tab-quickswitch
//!   tab-dropdown tab-popup tab-popup-history tab-popup-downloads tab-engine tab-combo`) sweep the mouse or the Tab key
//!   along a bar and print where anything other than the hovered widget moved (`shots_img::hover_report`);
//!   `focus-graph` does the same for clicks
//! - keyboard views: `graph-enter-group graph-enter-json graph-menu-key graph-select-key json-keys tabview-split-key`
//! - assertion views, which print `PASS:`/`FAIL:`: `mouse-regress` (drag, swatch click, middle-click close),
//!   `note-key`, `digit-jump`, `json-tab-offscreen`, `tabview-actions-click`
//! - `graph-menu-jars graph-menu-chip graph-menu-color graph-menu-group-jars`: the menu's Jar and Color pages
//!
//! `engine-states-picker engine-states-settings engine-install-confirm` (any one runs the block) start more instances
//! to shoot the CEF install states: a copy of the exe with no CEF host beside it, on its own profile, walks Not
//! installed, Failed, Installing, Installed and Update needed with nothing mocked in the app (the runtime is files in
//! the profile's data dir; the download goes through `HTTPS_PROXY` to `shots_net::Proxy`, which refuses or stalls), and
//! two more name a bring-your-own folder that matches the pin or not (Custom). The block needs a build that pins a CEF
//! host (crates/manifold/src/host_pin.rs, written by `gate host-pin --adopt`) and is skipped without one.
//!
//! `gate chrome-shots-sizes` runs it at each of `SIZES` (`"400 300,800 600,1920 1080"`) into `OUT/<W>x<H>/`, with
//! `VIEWS` defaulting to the ones that need no click on a fixed coordinate. `gate layout` checks the popups' bounds
//! by number; this is for looking. Build first: `cargo build -p manifold -p manifold-view-webview2 -p gate`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{json, Value};

use crate::check::{check, check_eq, note_failure, Section};
use crate::instance::{seed_settings, Instance};
use crate::probes::{finish, import_session, quit, setup};
use crate::shots_img::{hover_report, write_sheet};
use crate::shots_net::{Downloads, Proxy};
use crate::state::{round_half_even, State, GROUP, IMAGE, JSON, NOTE, TAB};

const DEFAULT_VIEWS: &str = "graph graph-hover graph-menu-tab graph-menu-group graph-menu-note graph-link graph-menu-links graph-link-hover graph-link-handles graph-link-drag graph-filter graph-group json-level json-menu tabview tabview-dropdown tabview-find tabview-actions tabview-split settings popup-bookmarks popup-history popup-downloads shortcuts quickswitch graph-closed graph-newnode-menu newnode-zoom graph-tooltip image-view cards-f note-edit missing-file download-real download-progress download-failed json-zoom settings-engine settings-engine-tab settings-combo toast tabview-engine json-tooltip engine-states-picker engine-states-settings engine-install-confirm";
const SIZES_VIEWS: &str = "graph graph-menu-tab graph-newnode-menu shortcuts quickswitch settings tabview";
const HOST_PIN: &str = include_str!("../../manifold/src/host_pin.rs");
const CEF_PIN: &str = include_str!("../../manifold/src/cef_pin.rs");
/// Where a click outside every popup lands and closes it.
const AWAY: &str = "CLICK 1100 300";
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

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

pub(crate) fn chrome_shots(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "chrome-shots") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let Some(size) = parse_size(&std::env::var("SIZE").unwrap_or_else(|_| "1280 800".into())) else {
        eprintln!("SIZE is \"W H\"");
        return ExitCode::from(2);
    };
    let views = std::env::var("VIEWS").unwrap_or_else(|_| DEFAULT_VIEWS.into());
    let cfg = Config {
        size,
        views: views.split_whitespace().map(str::to_owned).collect(),
        out: std::env::var_os("OUT").map_or_else(|| default_out(&exe, "shots"), PathBuf::from),
        crop_px: knob("CROP_PX", 110),
    };
    run(&exe, &work, cfg);
    finish(&work)
}

pub(crate) fn chrome_shots_sizes(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "chrome-shots-sizes") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let sizes = std::env::var("SIZES").unwrap_or_else(|_| "400 300,800 600,1920 1080".into());
    let views = std::env::var("VIEWS").unwrap_or_else(|_| SIZES_VIEWS.into());
    let out = std::env::var_os("OUT").map_or_else(|| default_out(&exe, "shots-sizes"), PathBuf::from);
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
        run(&exe, &work.join(&leaf), cfg);
    }
    finish(&work)
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
    inst: Instance,
    out: PathBuf,
    size: (u32, u32),
    views: Vec<String>,
    /// The engine-states instance's data dir for the engine hosts.
    eng_data: PathBuf,
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
        image::RgbImage::from_pixel(240, 160, image::Rgb([70, 110, 160]))
            .save(work.join("pic.png"))
            .map_err(io::Error::other)?;
        fs::write(
            work.join("data.json"),
            json!({"name": "demo", "items": [1, 2, 3], "nested": {"a": true, "b": null}}).to_string(),
        )?;
        let appdata = root.display().to_string();
        let inst = Instance::launch_env(exe, &profile, work, "log", &[("APPDATA", &appdata)])?;
        Ok(Shots {
            work: work.to_path_buf(),
            inst,
            out: cfg.out,
            size: cfg.size,
            views: cfg.views,
            eng_data: PathBuf::new(),
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

    /// Waits until two exports agree on the cards: SLEEP draws no frames, and the camera eases after entering a
    /// level, so a fixed sleep can shoot mid-ease.
    fn settle(&mut self) -> io::Result<()> {
        let mut prev = String::new();
        for _ in 0..20 {
            let cur: String = self.state()?.all_cards().iter().map(|c| format!("{}{};", c["id"], c["rect"])).collect();
            if cur == prev {
                break;
            }
            prev = cur;
        }
        Ok(())
    }

    fn shot(&mut self, name: &str) -> io::Result<()> {
        self.settle()?;
        let path = self.out.join(format!("{name}.png"));
        self.inst.screenshot_to(&path)?;
        println!("shot: {name}");
        Ok(())
    }

    /// Back to Graph view at the top level, nothing open. Escape closes popups and leaves Tab view; the toolbar's
    /// back button (top left) leaves a group or a JSON level.
    fn reset(&mut self) -> io::Result<()> {
        for _ in 0..5 {
            send!(self, "ESCAPE")?;
            self.settle()?;
            let s = self.state()?;
            if !s.tab_view() {
                if s.current_group().is_none() && s.0["json_level"].is_null() {
                    return Ok(());
                }
                send!(self, "CLICK 14 16")?;
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

    /// Whether a menu or popup is open (the key hint beside a keyboard-focused card does not count).
    fn menu_open(&mut self) -> io::Result<bool> {
        let st = self.state()?;
        let open = st.0["popups"]
            .as_array()
            .is_some_and(|list| list.iter().any(|p| p["id"].as_str().is_some_and(|id| !id.contains("card_key_hint"))));
        Ok(open)
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

    /// Rest, then `n` Tab key presses (egui keyboard focus moving on), a shot after each.
    fn tab_scan(&mut self, name: &str, n: u32, band: u32) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "1275 795")?;
        for i in 1..=n {
            send!(self, "TABKEY", "SLEEP 0.5")?;
            self.inst.screenshot_to(&dir.join(format!("x{i:04}.png")))?;
        }
        Self::print_report(&dir, band, false, 0)
    }

    /// Rest, then a click at each point (focusing whatever is there), Escape between. Same report as `hover_scan`.
    fn focus_scan(&mut self, name: &str, points: &[(i64, i64)]) -> io::Result<()> {
        let dir = self.scan_dir(name)?;
        self.rest_shot(&dir, "5 790")?;
        for (i, (x, y)) in points.iter().enumerate() {
            send!(self, format!("CLICK {x} {y}"), "SLEEP 0.5", "MOUSEMOVE 5 790", "SLEEP 0.5")?;
            self.inst.screenshot_to(&dir.join(format!("x{:04}.png", i + 1)))?;
            send!(self, "ESCAPE", "SLEEP 0.5")?;
        }
        Self::print_report(&dir, 34, false, 0)
    }

    // ---- the views -----------------------------------------------------------------------------------------------

    fn run_views(&mut self, exe: &Path) -> io::Result<()> {
        let (w, h) = self.size;
        let work = self.work.clone();
        send!(self, format!("RESIZE {w} {h}"), "WAITTABS", "SLEEP 3")?;
        send!(self, format!("ADDFILE {} 200 640", work.join("pic.png").display()))?;
        send!(self, format!("ADDFILE {} 640 700", work.join("data.json").display()), "SLEEP 2")?;

        self.view("graph", |s| s.shot("graph"))?;
        self.graph_menus()?;
        self.link_views()?;
        self.view("hover-graph", |s| s.hover_scan("graph", knob("HOVER_Y", 16), knob("HOVER_BAND", 34)))?;
        self.view("tab-graph", |s| s.tab_scan("graph", knob("TAB_STOPS", 10), knob("HOVER_BAND", 34)))?;
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
        self.view("focus-graph", |s| s.focus_scan("graph", &[(385, 16), (235, 16)]))?;
        // Right-click on empty canvas: the New Tab/Note/Group card.
        self.view("graph-newnode-menu", |s| {
            s.menu_at_empty()?;
            s.shot("graph-newnode-menu")?;
            s.reset()
        })?;
        // The same card zoomed far out: its rows keep a constant screen size (they once scaled with zoom and overflowed).
        self.view("newnode-zoom", |s| {
            for _ in 0..6 {
                s.click(219, 19)?;
            }
            s.menu_at_empty()?;
            s.shot("newnode-zoom")?;
            s.reset()
        })?;
        // A toolbar tooltip (the shortcuts help button).
        self.view("graph-tooltip", |s| {
            send!(s, "MOUSEMOVE 467 16", "SLEEP 2")?;
            s.shot("graph-tooltip")?;
            send!(s, "MOUSEMOVE 5 790")
        })?;
        // A download card and a sleeping tab beside the live cards (header F on cards with no seeded kind of their own).
        self.view("cards-f", |s| {
            send!(
                s,
                format!("ADDDOWNLOAD {}", work.join("data.json").display()),
                "SLEEPNODE t1",
                "SLEEP 6",
                "MOUSEMOVE 327 19",
                "MOUSEDOWN 327 19 left",
                "MOUSEUP 327 19 left",
                "SLEEP 2"
            )?;
            s.shot("cards-f")
        })?;
        // A note being edited: a click on the rendered note enters the editor in place.
        self.view("note-edit", |s| {
            let (x, y) = s.center(NOTE, "top", None)?;
            send!(s, format!("CLICK {x} {y}"), "SLEEP 1", "MOUSEMOVE 5 790", "SLEEP 1")?;
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
        let closed_button = i64::from(knob("CLOSED_BUTTON_X", 515));
        self.view("graph-closed", |s| {
            let (x, y) = s.center(NOTE, "top", None)?;
            send!(s, format!("CLICK {x} {y} middle"), "SLEEP 1")?;
            s.settle()?;
            send!(s, format!("CLICK {closed_button} 16"), "SLEEP 1")?;
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
            fs::copy(work.join("pic.png"), &pic)?;
            append(&pic, b"x")?;
            fs::copy(work.join("data.json"), &data)?;
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
        if self.has_any(&["download-real", "download-progress", "download-failed"]) {
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
        // The same menu's Color page.
        self.view("graph-menu-color", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            s.menu_at(x, y)?;
            send!(s, "MENUPAGE color", "SLEEP 1")?;
            s.shot("graph-menu-color")?;
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
    /// it, clicking its swatch tags it, middle-click closes it.
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
        // The swatch sits on the bottom-right corner (hover reveals nothing extra; the click is what counts).
        send!(self, format!("MOUSEMOVE {} {}", n0 + 20, m0 + 20), "SLEEP 0.5", format!("CLICK {n1} {m1}"), "SLEEP 1")?;
        self.shot("mouse-regress-swatch")?;
        let st = self.state()?;
        let tagged = st
            .all_cards()
            .iter()
            .find(|c| State::card_is(c, NOTE, None))
            .is_some_and(|c| c["color"].as_bool() == Some(true));
        check("swatch click tags the card", tagged);
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
            if s.has("json-tooltip") {
                send!(s, format!("MOUSEMOVE {} 16", knob("JSON_TIP_X", 120)), "SLEEP 2")?;
                s.shot("json-tooltip")?;
                send!(s, "MOUSEMOVE 5 790")?;
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
                s.tab_scan("json", knob("JSON_TAB_STOPS", 18), knob("HOVER_BAND", 34))?;
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
        let w = i64::from(self.size.0);
        let band = knob("HOVER_BAND", 34);
        self.step("tab view", |s| {
            let (x, y) = s.center(TAB, "top", Some(true))?;
            send!(s, format!("DOUBLECLICK {x} {y}"), "SLEEP 5")?;
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
            // Asserted: the popup's Pin row pins the focused tab, its Duplicate row adds a card, its Close Tab row
            // removes one.
            if s.has("tabview-actions-click") {
                let pinned = |s: &mut Self| -> io::Result<usize> {
                    Ok(s.state()?.cards().iter().filter(|c| c["pinned"].as_bool() == Some(true)).count())
                };
                let (pins0, cards0) = (pinned(s)?, s.card_count()?);
                let ax = w - 95;
                send!(s, format!("CLICK {ax} 16"), "SLEEP 1", format!("CLICK {ax} 49"), "SLEEP 1")?;
                check("Tab actions > Pin pins the tab", pinned(s)? == pins0 + 1);
                send!(s, format!("CLICK {ax} 16"), "SLEEP 1", format!("CLICK {ax} 76"), "SLEEP 2")?;
                check("Tab actions > Duplicate adds a card", s.card_count()? == cards0 + 1);
                send!(s, format!("CLICK {ax} 16"), "SLEEP 1", format!("CLICK {ax} 155"), "SLEEP 2")?;
                let now = s.card_count()?;
                check(&format!("Tab actions > Close Tab removes the tab ({now} vs {cards0})"), now == cards0);
            }
            if s.has("tabview-actions") {
                send!(s, format!("CLICK {} 16", w - 95), "SLEEP 1")?;
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
                send!(s, format!("CLICK {} 16", w - 126), "SLEEP 1")?;
                s.shot("tabview-engine")?;
                send!(s, "ESCAPE", "SLEEP 1")?;
            }
            // Keyboard-only pick from the Split engine picker: it opens on the selected card, Down moves, Enter picks
            // and opens the Split pane.
            if s.has("tabview-split-key") {
                send!(s, format!("CLICK {} 16", w - 126), "SLEEP 1")?;
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
        let data = self.work.join("data.json");
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
            // Engine picker (Default starting engine) and the search-engine combo.
            for (view, click) in [
                ("settings-engine", "CLICK 63 180"),
                ("settings-engine-tab", "CLICK 119 513"),
                ("settings-combo", "CLICK 80 87"),
            ] {
                if s.has(view) {
                    send!(s, click, "SLEEP 1")?;
                    s.shot(view)?;
                    send!(s, AWAY, "SLEEP 1")?;
                }
            }
            for (view, click, name) in [("tab-engine", "CLICK 63 180", "engine"), ("tab-combo", "CLICK 80 87", "combo")]
            {
                if s.has(view) {
                    send!(s, click, "SLEEP 1")?;
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

    /// Real downloads (`download-failed` kills the server mid-transfer): a local HTTP server answers /real.zip as a
    /// small attachment and /slow as a 1 MB one it stalls after 300 KB, and the live tab navigates to it, so the
    /// engine starts the download (the card keeps the URL as its source line). Before json-zoom, which must run last.
    fn download_views(&mut self) -> io::Result<()> {
        let server = Downloads::start()?;
        let (x, y) = self.center(TAB, "top", Some(true))?;
        send!(self, format!("DOUBLECLICK {x} {y}"), "SLEEP 5")?;
        if self.has_any(&["download-progress", "download-failed"]) {
            // Still arriving: 300 KB of 1 MB. The fit button frames the new card, which opens above the tab's.
            send!(self, format!("ADDRESSBAR {}", server.url("/slow")), "SLEEP 1", "ENTER", "SLEEP 6")?;
            send!(self, "TOGGLEVIEW", "SLEEP 2", "CLICK 327 19", "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            if self.has("download-progress") {
                self.shot("download-progress")?;
            }
            if self.has("download-failed") {
                // The server dies mid-transfer: the engine reports the download interrupted.
                server.kill();
                send!(self, "SLEEP 5", "MOUSEMOVE 5 780", "SLEEP 1")?;
                self.shot("download-failed")?;
            }
            send!(self, format!("DOUBLECLICK {x} {y}"), "SLEEP 2")?;
        }
        if self.has("download-real") {
            send!(self, format!("ADDRESSBAR {}", server.url("/real.zip")), "SLEEP 1", "ENTER", "SLEEP 6")?;
            self.shot("download-real-tab")?;
            send!(self, "TOGGLEVIEW", "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            self.shot("download-real")?;
        }
        Ok(())
    }

    /// Last, because it leaves the graph zoomed (the toolbar's fit button does not restore it). The Json card
    /// preview's header stays 40 points while its rows get more or less of the card: the toolbar's zoom-out button
    /// twice (fewer rows fit under the header), then back in (the graph zoom, not the page zoom `ZOOMIN` sets).
    fn json_zoom(&mut self) -> io::Result<()> {
        self.click(219, 19)?;
        self.click(219, 19)?;
        send!(self, "MOUSEMOVE 5 790", "SLEEP 2")?;
        self.shot("json-zoom-out")?;
        // Further out: the Note card under 80 points tall, so it falls back to the 18-point name strip.
        for _ in 0..3 {
            self.click(219, 19)?;
        }
        send!(self, "MOUSEMOVE 5 790", "SLEEP 2")?;
        self.shot("note-short")?;
        for _ in 0..9 {
            self.click(295, 19)?;
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
        self.eng_data = profile.join("manifold").join("data").join("engine-hosts");
        fs::create_dir_all(&self.eng_data)?;
        import_session(
            eng_exe,
            &profile,
            &[
                json!({"id": "t1", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}),
                json!({"id": "t3", "type": TAB, "url": "https://example.com/", "view_kind": "cef", "pos": [300.0, 0.0]}),
            ],
        )?;
        let (appdata, proxy_url) = (root.display().to_string(), proxy.url());
        self.inst = Instance::launch_env(
            eng_exe,
            &profile,
            &self.work,
            &format!("log-eng-{name}"),
            &[("APPDATA", &appdata), ("HTTPS_PROXY", &proxy_url)],
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
        fs::write(self.eng_data.join("cef.host"), marker)
    }

    /// A runtime directory with a (sparse) 423 MB libcef.dll and `marker` in `ready`.
    fn fake_runtime(&self, marker: &str) -> io::Result<()> {
        let dir = self.eng_data.join("cef-runtime");
        fs::create_dir_all(&dir)?;
        fs::File::create(dir.join("libcef.dll"))?.set_len(423 * 1024 * 1024)?;
        fs::write(dir.join("ready"), marker)
    }

    fn engine_stop(&mut self) -> io::Result<()> {
        self.inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    fn engine_states(&mut self, exe: &Path) -> io::Result<()> {
        let pins = cef_components(HOST_PIN);
        if pins.is_empty() {
            println!("SKIP engine-states-*: host_pin.rs pins no CEF host for windows-x64 (cut a release table first)");
            return Ok(());
        }
        let pin_version = pin_version(CEF_PIN).ok_or_else(|| io::Error::other("cef_pin.rs has no version"))?;
        let eng = self.work.join("eng");
        fs::create_dir_all(&eng)?;
        let eng_exe = eng.join("manifold.exe");
        fs::copy(exe, &eng_exe)?;
        let host = exe.with_file_name("manifold-engine-webview2.exe");
        fs::copy(&host, eng.join("manifold-engine-webview2.exe"))?;
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
        send!(self, "CLICK 57 672", "SLEEP 1")?;
        self.shot("engine-install-confirm")?;
        // Confirm with the proxy refusing: the install fails and the row says why, with Retry.
        send!(self, "CLICK 476 529", "SLEEP 4")?;
        self.shot("engine-states-settings-failed")?;
        self.picker_shot("engine-states-picker-failed")?;
        // Retry with the proxy silent: Installing, with Cancel.
        proxy.set_refuse(false);
        send!(self, "CLICK 57 672", "SLEEP 1", "CLICK 476 529", "SLEEP 2")?;
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
            let dir = self.work.join(format!("mycef-{name}"));
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("libcef.dll"), "")?;
            fs::write(
                dir.join("archive.json"),
                json!({"type": "minimal", "name": format!("cef_binary_{archive}_windows64_minimal.tar.bz2")})
                    .to_string(),
            )?;
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
        send!(self, "CLICK 63 180", "SLEEP 1")?;
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

/// `(file, size, sha256)` of the CEF `windows-x64` components in host_pin.rs's table.
fn cef_components(table: &str) -> Vec<(String, u64, String)> {
    let mut in_cef = false;
    let mut out = Vec::new();
    for line in table.lines().map(str::trim) {
        if line.starts_with("Entry {") {
            in_cef = line.contains("ViewKind::Cef") && line.contains("\"windows-x64\"");
        } else if in_cef && line.starts_with("Component {") {
            let parsed = (|| {
                let size = between(line, "size: ", ",")?.parse().ok()?;
                Some((
                    between(line, "file: \"", "\"")?.to_string(),
                    size,
                    between(line, "sha256: \"", "\"")?.to_string(),
                ))
            })();
            out.extend(parsed);
        }
    }
    out
}

/// The CEF version cef_pin.rs pins (its first `version: "..."`).
fn pin_version(src: &str) -> Option<String> {
    between(src, "version: \"", "\"").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repos_pins_parse() {
        let pins = cef_components(HOST_PIN);
        assert!(!pins.is_empty(), "host_pin.rs pins a CEF windows-x64 host");
        assert!(
            pins.iter().all(|(file, size, sha)| file.ends_with(".exe") && *size > 0 && sha.len() == 64),
            "{pins:?}"
        );
        assert!(pin_version(CEF_PIN).is_some_and(|v| v.contains("chromium")));
    }

    #[test]
    fn components_of_other_engines_and_platforms_are_skipped() {
        let table = "Entry { kind: ViewKind::Servo, platform: \"windows-x64\", components: &[\n\
            Component { file: \"s.exe\", url: \"u\", size: 5, sha256: \"aa\" },\n] },\n\
            Entry { kind: ViewKind::Cef, platform: \"linux-x64\", components: &[\n\
            Component { file: \"l\", url: \"u\", size: 6, sha256: \"bb\" },\n] },\n\
            Entry { kind: ViewKind::Cef, platform: \"windows-x64\", components: &[\n\
            Component { file: \"c.exe\", url: \"u\", size: 7, sha256: \"cc\" },\n] },";
        assert_eq!(cef_components(table), [("c.exe".to_string(), 7, "cc".to_string())]);
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("1280 800"), Some((1280, 800)));
        assert_eq!(parse_size(" 400  300 "), Some((400, 300)));
        assert_eq!(parse_size("400"), None);
        assert_eq!(parse_size("1 2 3"), None);
    }
}
