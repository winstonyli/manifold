//! `gate layout`: every menu, dialog and dropdown the headless app can open, at several window sizes, checked for the
//! layout oddities a person finds by eye. Per window size, one instance on the default (demo) session for the canvas
//! popups and one for the Settings page's.
//!
//! Two checks per popup and size:
//! - *inside the window*: every new `popups` entry of the exported state (`App::popups_state`) lies within `window`.
//!   Catches a dialog taller than a short window, a menu opened past an edge.
//! - *nothing shows through*: the popup is shot twice, once over the demo cards and once with every other card moved
//!   far away, and its rect must come out pixel-identical (headless mode draws a steady caret). A card's
//!   text or chip painting over a popup, or a popup drawn under the cards (as the empty-canvas menu once was, inside
//!   the Scene), makes the two differ. Pixels are compared in the screenshot, so this sees what a person sees.
//!
//! What it cannot see: a layer problem under a popup that looks identical both ways, and anything that needs the
//! popup to be opened by a toolbar click (the commands that open those are coordinates, not names).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::Value;

use crate::check::{check, exit_code, failures, note_failure, Section};
use crate::instance::{seed_settings, Instance};
use crate::state::{State, GROUP, NOTE, TAB};
use crate::windows_main::{manifold_exe, work_dir};

/// Window sizes in points: the common case, a small window, a large one.
const SIZES: [(u32, u32); 3] = [(800, 600), (480, 360), (1280, 800)];

/// Where cards are parked for the "nothing under the popup" shot: far outside any window.
const FAR: f64 = 200_000.0;

/// Points trimmed off each side of a popup's rect before comparing pixels.
const EDGE: f64 = 4.0;

/// A colour channel difference above this counts as a changed pixel (antialiasing and fades stay under it).
const CHANNEL_TOLERANCE: i32 = 8;

/// A popup scenario: what opens it, what closes it, and which card (by type) it is anchored to, if any.
struct Scenario {
    name: &'static str,
    open: Open,
    close: &'static str,
    /// A list of live content (the quick switcher's tabs, whose titles and order change as pages load) starts this
    /// many points below the popup's top: only what is above it is compared.
    list_below: Option<f64>,
}

enum Open {
    Command(&'static str),
    /// A right-click on the empty canvas.
    CanvasMenu,
    /// A right-click on the first card of this type, then (when given) the menu page `MENUPAGE` shows.
    CardMenu(&'static str, Option<&'static str>),
    /// One of the Settings page's list popups (`bookmarks`, `history`, `downloads`).
    Settings(&'static str),
}

static SCENARIOS: [Scenario; 11] = [
    Scenario {
        name: "shortcuts-help",
        open: Open::Command("TOGGLESHORTCUTSHELP"),
        close: "TOGGLESHORTCUTSHELP",
        list_below: None,
    },
    Scenario {
        name: "quick-switcher",
        open: Open::Command("QUICKSWITCHOPEN"),
        close: "QUICKSWITCHESCAPE",
        list_below: Some(70.0),
    },
    Scenario { name: "canvas-menu", open: Open::CanvasMenu, close: "ESCAPE", list_below: None },
    Scenario { name: "tab-menu", open: Open::CardMenu(TAB, None), close: "ESCAPE", list_below: None },
    Scenario { name: "note-menu", open: Open::CardMenu(NOTE, None), close: "ESCAPE", list_below: None },
    Scenario { name: "group-menu", open: Open::CardMenu(GROUP, None), close: "ESCAPE", list_below: None },
    Scenario { name: "tab-menu-jar", open: Open::CardMenu(TAB, Some("jar")), close: "ESCAPE", list_below: None },
    Scenario { name: "group-menu-jar", open: Open::CardMenu(GROUP, Some("jar")), close: "ESCAPE", list_below: None },
    Scenario {
        name: "bookmarks-popup",
        open: Open::Settings("bookmarks"),
        close: "TOGGLEPOPUP bookmarks",
        list_below: None,
    },
    Scenario { name: "history-popup", open: Open::Settings("history"), close: "TOGGLEPOPUP history", list_below: None },
    Scenario {
        name: "downloads-popup",
        open: Open::Settings("downloads"),
        close: "TOGGLEPOPUP downloads",
        list_below: None,
    },
];

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// A rect as `[left, top, right, bottom]`.
type Rect = [f64; 4];

fn rect_of(v: &Value) -> Option<Rect> {
    State::rect_from(v)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn window(s: &State) -> io::Result<(f64, f64)> {
    let w = &s.0["window"];
    Ok((w[0].as_f64().ok_or_else(|| bad("no window in state".into()))?, w[1].as_f64().unwrap_or(0.0)))
}

/// The popups showing now, by id.
fn popups(s: &State) -> HashMap<String, Rect> {
    s.0["popups"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| Some((p["id"].as_str()?.to_owned(), rect_of(&p["rect"])?)))
                .filter(|(id, _)| !HINTS.iter().any(|hint| id.contains(hint)))
                .collect()
        })
        .unwrap_or_default()
}

/// Popups that come and go on their own (the key hint shown beside a keyboard-focused card), not opened by a scenario.
const HINTS: [&str; 1] = ["card_key_hint"];

/// The rects of `after` that were not in `before`.
fn new_popups(before: &HashMap<String, Rect>, after: &State) -> Vec<(String, Rect)> {
    let mut found: Vec<_> = popups(after).into_iter().filter(|(id, _)| !before.contains_key(id)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn union(rects: &[(String, Rect)]) -> Rect {
    rects.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |u, (_, r)| {
        [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]
    })
}

/// Lets fades and the next frame finish.
fn settle(inst: &mut Instance) -> io::Result<()> {
    // Each command draws one frame and SLEEP draws none: frames spaced in time let a popup's sizing pass and
    // its fades and hover animations finish.
    inst.send("WAITIDLE")?;
    for _ in 0..3 {
        inst.send("MOUSEMOVE 2 2")?;
        inst.send("SLEEP 0.3")?;
    }
    inst.send("MOUSEMOVE 2 2")
}

/// A point on the canvas under no card, clear of the toolbar.
fn empty_point(s: &State, w: f64, h: f64) -> Option<(f64, f64)> {
    let cards: Vec<Rect> = s.all_cards().iter().filter_map(|c| rect_of(&c["rect"])).collect();
    for fy in [0.85, 0.75, 0.65, 0.55] {
        for fx in [0.5, 0.3, 0.7, 0.15, 0.85] {
            let (x, y) = (w * fx, h * fy);
            let probe = [x - 12.0, y - 12.0, x + 12.0, y + 12.0];
            if y > 80.0 && !cards.iter().any(|c| overlaps(*c, probe)) {
                return Some((x, y));
            }
        }
    }
    None
}

fn right_click(inst: &mut Instance, x: f64, y: f64) -> io::Result<()> {
    inst.send(&format!("MOUSEDOWN {x:.0} {y:.0} right"))?;
    inst.send(&format!("MOUSEUP {x:.0} {y:.0} right"))
}

/// Opens `sc`; returns the id of the card it is anchored to (kept in place for the second shot). `anchor` names the card
/// to use again (the first of its type otherwise).
fn open(
    inst: &mut Instance,
    sc: &Scenario,
    s: &State,
    w: f64,
    h: f64,
    anchor: Option<&str>,
) -> io::Result<Option<String>> {
    match &sc.open {
        Open::Command(cmd) => inst.send(cmd).map(|()| None),
        Open::Settings(name) => inst.send(&format!("TOGGLEPOPUP {name}")).map(|()| None),
        Open::CanvasMenu => {
            let (x, y) = empty_point(s, w, h).ok_or_else(|| bad("no empty canvas point".into()))?;
            right_click(inst, x, y).map(|()| None)
        }
        Open::CardMenu(ty, page) => {
            let card = s
                .all_cards()
                .iter()
                .find(|c| c["type"] == *ty && anchor.is_none_or(|a| c["id"] == a))
                .ok_or_else(|| bad(format!("no {ty} card in the demo session")))?;
            let (x, y) = (card["screen"][0].as_f64().unwrap_or(0.0), card["screen"][1].as_f64().unwrap_or(0.0));
            right_click(inst, x, y)?;
            if let Some(page) = page {
                inst.send(&format!("MENUPAGE {page}"))?;
            }
            Ok(card["id"].as_str().map(str::to_owned))
        }
    }
}

/// How many pixels of `region` (points) differ between two screenshots, and the pixel box around them.
fn differing_pixels(a: &Path, b: &Path, region: Rect, window_w: f64) -> io::Result<(usize, [u32; 4])> {
    let (ia, ib) = (crate::shots_img::load(a)?, crate::shots_img::load(b)?);
    if ia.dimensions() != ib.dimensions() {
        return Err(bad(format!("screenshots differ in size: {:?} vs {:?}", ia.dimensions(), ib.dimensions())));
    }
    let scale = f64::from(ia.width()) / window_w;
    let clamp = |v: f64, max: u32| (v.max(0.0) as u32).min(max);
    let (x0, y0) = (clamp((region[0] * scale).floor(), ia.width()), clamp((region[1] * scale).floor(), ia.height()));
    let (x1, y1) = (clamp((region[2] * scale).ceil(), ia.width()), clamp((region[3] * scale).ceil(), ia.height()));
    let mut count = 0;
    let mut bbox = [u32::MAX, u32::MAX, 0, 0];
    for y in y0..y1 {
        for x in x0..x1 {
            let (pa, pb) = (ia.get_pixel(x, y), ib.get_pixel(x, y));
            if pa.0.iter().zip(pb.0.iter()).any(|(p, q)| (i32::from(*p) - i32::from(*q)).abs() > CHANNEL_TOLERANCE) {
                count += 1;
                bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x), bbox[3].max(y)];
            }
        }
    }
    Ok((count, bbox))
}

/// One popup as the first pass left it: where it showed and the screenshot taken over the cards.
struct Shot {
    label: String,
    scenario: &'static Scenario,
    region: Rect,
    over_cards: PathBuf,
    /// The card the popup is anchored to, kept in place for the second shot.
    anchor: Option<String>,
}

/// Checks `scenarios` at one window size on `inst`: each is opened over the demo cards and shot, then every card but
/// `anchor` is parked far away and each is opened and shot again; the cards are put back (EXPORTSTATE's `world`)
/// before the next scenario parks all but its own anchor.
fn run_group(inst: &mut Instance, (width, height): (u32, u32), scenarios: &[&'static Scenario]) -> io::Result<()> {
    inst.send("WAITTABS")?;
    // The first-launch demo: a tab, a note, a group and a JSON file, so there is something under every popup.
    inst.send("TRIGGERFIRSTLAUNCHDEMO")?;
    inst.send("WAITTABS")?;
    // Titles arrive after the tabs spawn, and the quick switcher lists them.
    inst.send("SLEEP 3")?;
    inst.send(&format!("RESIZE {width} {height}"))?;
    settle(inst)?;
    let s0 = inst.export_state("layout-s0")?;
    let (w, h) = window(&s0)?;
    check(&format!("layout {width}x{height}: the window resized (got {w}x{h})"), w == f64::from(width));

    // Where each card lives, so the parking below can be undone for the next scenario's anchor.
    let home: Vec<(String, f64, f64)> = s0
        .all_cards()
        .iter()
        .filter_map(|c| Some((c["id"].as_str()?.to_owned(), c["world"][0].as_f64()?, c["world"][1].as_f64()?)))
        .collect();
    let mut parked: Vec<String> = Vec::new();
    for &sc in scenarios {
        // Back home from the last scenario's parking.
        for (id, x, y) in home.iter().filter(|(id, ..)| parked.contains(id)) {
            inst.send(&format!("MOVENODE {id} {x} {y}"))?;
        }
        parked.clear();
        settle(inst)?;
        let label = format!("layout {width}x{height} {}", sc.name);
        let before = inst.export_state("layout-before")?;
        let known = popups(&before);
        let anchor = open(inst, sc, &before, w, h, None)?;
        settle(inst)?;
        let shown = inst.export_state("layout-open")?;
        let fresh = new_popups(&known, &shown);
        if !check(&format!("{label}: it opens"), !fresh.is_empty()) {
            inst.send(sc.close)?;
            continue;
        }
        let outside: Vec<_> = fresh
            .iter()
            .filter(|(_, r)| r[0] < -0.5 || r[1] < -0.5 || r[2] > w + 0.5 || r[3] > h + 0.5)
            .map(|(id, r)| format!("{id} {r:?}"))
            .collect();
        if !check(&format!("{label}: it lies inside the {w}x{h} window"), outside.is_empty()) {
            println!("  outside: {}", outside.join("; "));
        }
        // The popup's own shadow and rounded corners blend with what is under them: compare inside them.
        let [l, t, r, b] = union(&fresh);
        let region = [l + EDGE, t + EDGE, r - EDGE, b - EDGE];
        let over_cards = inst.screenshot(&format!("layout-over-cards-{}", sc.name))?;
        let shot = Shot { label: label.clone(), scenario: sc, region, over_cards, anchor };
        inst.send(sc.close)?;
        settle(inst)?;
        let after = inst.export_state("layout-closed")?;
        let left = new_popups(&known, &after);
        if !check(&format!("{label}: it closes"), left.is_empty()) {
            println!("  still showing: {left:?}");
        }

        // Straight away, with every card but its anchor parked far outside the window (the others are put back
        // first): a popup's remembered size from another scenario's page would otherwise differ between the shots.
        let (label, sc) = (&shot.label, shot.scenario);
        let others: Vec<String> =
            home.iter().map(|(id, ..)| id.clone()).filter(|id| Some(id) != shot.anchor.as_ref()).collect();
        for (id, x, y) in &home {
            let want_parked = others.contains(id);
            if want_parked != parked.contains(id) {
                let (x, y) = if want_parked { (FAR, FAR) } else { (*x, *y) };
                inst.send(&format!("MOVENODE {id} {x} {y}"))?;
            }
        }
        parked = others.clone();
        settle(inst)?;
        let parked_state = inst.export_state("layout-parked")?;
        let still_there = parked_state
            .all_cards()
            .iter()
            .filter(|c| c["id"].as_str().is_some_and(|id| others.iter().any(|o| o == id)))
            .filter_map(|c| rect_of(&c["rect"]))
            .any(|r| overlaps(r, shot.region));
        check(&format!("{label}: the other cards are out of the way"), !still_there);
        let known = popups(&parked_state);
        open(inst, sc, &parked_state, w, h, shot.anchor.as_deref())?;
        settle(inst)?;
        let reshown = inst.export_state("layout-reopen")?;
        let again = new_popups(&known, &reshown);
        // Where it opens must not depend on the cards; how tall it is may (the quick switcher's list is the tabs).
        let [al, at, ar, _] = union(&again);
        let moved = [al + EDGE - shot.region[0], at + EDGE - shot.region[1], ar - EDGE - shot.region[2]]
            .iter()
            .any(|d| d.abs() > 1.0);
        if !check(&format!("{label}: it opens in the same place without the cards"), !again.is_empty() && !moved) {
            println!(
                "  first {:?}
  again {again:?}",
                shot.region
            );
        }
        // The 1 px slack above hides a menu that comes out a pixel wider or narrower, which then differs along its
        // edge in the shots; the width must not depend on the cards at all.
        let (first_width, again_width) = (shot.region[2] - shot.region[0], ar - al - 2.0 * EDGE);
        if !check(&format!("{label}: it is as wide without the cards"), (first_width - again_width).abs() < 0.01) {
            println!("  width {first_width} first, {again_width} again");
        }
        let [_, _, _, bottom] = union(&again);
        let header = sc.list_below.map_or(f64::MAX, |h| shot.region[1] + h);
        let compared = [shot.region[0], shot.region[1], shot.region[2], shot.region[3].min(bottom - EDGE).min(header)];
        let bare = inst.screenshot(&format!("layout-bare-{}", sc.name))?;
        let (differing, bbox) = differing_pixels(&shot.over_cards, &bare, compared, w)?;
        if !check(&format!("{label}: nothing shows through it"), differing == 0) {
            println!(
                "  {differing} pixels differ in {:?} (box {bbox:?}): {} vs {}",
                shot.region,
                shot.over_cards.display(),
                bare.display()
            );
            // Kept for a look: the run's work dir is deleted unless KEEP is set.
            if let Some(keep) = std::env::var_os("LAYOUT_KEEP") {
                let dir = PathBuf::from(keep);
                let _ = std::fs::create_dir_all(&dir);
                let stem = format!("{width}x{height}-{}", sc.name);
                let _ = std::fs::copy(&shot.over_cards, dir.join(format!("{stem}-cards.png")));
                let _ = std::fs::copy(&bare, dir.join(format!("{stem}-bare.png")));
            }
        }
        inst.send(sc.close)?;
        settle(inst)?;
    }
    Ok(())
}

fn run(exe: &Path, work: &Path) -> io::Result<()> {
    let mut group = 0;
    for size in SIZES {
        // The Settings page replaces the canvas, so its popups share an instance apart from the canvas ones.
        let on = |f: fn(&Open) -> bool| SCENARIOS.iter().filter(move |sc| f(&sc.open)).collect::<Vec<&Scenario>>();
        let batches = [on(|o| !matches!(o, Open::Settings(_))), on(|o| matches!(o, Open::Settings(_)))];
        for batch in batches {
            group += 1;
            let _timing = Section::start(&format!("group{group}-{}x{}", size.0, size.1));
            let profile = work.join(format!("profile{group}"));
            seed_settings(&profile, "webview2", "")?;
            let mut inst = Instance::launch(exe, &profile, work, &format!("layout{group}.log"))?;
            println!(
                "launched pid {} for {size:?}: {}",
                inst.pid(),
                batch.iter().map(|sc| sc.name).collect::<Vec<_>>().join(", ")
            );
            let result = run_group(&mut inst, size, &batch);
            if failures() > 0 {
                println!(
                    "--- tail of {}
{}",
                    inst.log.display(),
                    inst.log_tail(30)
                );
            }
            let status = inst.quit(Duration::from_secs(30))?;
            check(&format!("layout {size:?}: the app exits with 0 after QUIT"), status.success());
            result?;
        }
    }
    Ok(())
}

pub(crate) fn layout(parent: Option<PathBuf>) -> ExitCode {
    let setup = Section::start("setup");
    let (exe, work) = match (manifold_exe(), work_dir(parent, "layout")) {
        (Ok(exe), Ok(work)) => (exe, work),
        (Err(e), _) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
        (_, Err(e)) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let steps = setup.next("layout");
    let result = run(&exe, &work);
    if let Err(e) = result {
        println!("ERROR: layout stopped early: {e}");
        note_failure();
    }
    drop(steps);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&work);
    }
    exit_code()
}
