//! `gate trail`: Back and Forward over a tab's pages and over opened-from, end to end (spec
//! docs/superpowers/specs/2026-10-14-trail-design.md, "Tests"). One throwaway profile seeded with tab A on a local site
//! served by this process (every page titled by its path). From A, NEWTAB opens tab B (parented to the focused tab) at
//! the home page /b1; B goes to /b2 and /b3. Back twice walks B's pages, a third Back focuses A (opened-from, and A
//! records `left_to`), Forward on A returns to B, and a new page from /b1 makes a sibling of /b2 (EXPORTSTATE
//! `trail`). In Graph view the focused tab's trail is drawn as one strip card per page, and a screenshot there (at 60%) and one in Tab view each hold the engine-colored icon tiles (`engine_pixels`). A real press-and-drag of tab A's card over the strip's second page is held for a screenshot (`drag-over-strip.png` in `RUN_DIR`) and released back. A page that calls history.back() itself must read as a Back. `ENGINE=servo` (etc.) tests another engine; for WebKit add `SITE_HOST` (http.rs).
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. The instance is killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::check::{check_is, exit_code, failures, note_failure, Section};
use crate::http::{response, Handler, Server};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::{bad, no_verdict_words};
use crate::state::{State, TAB};
use crate::windows_main::{manifold_exe, work_dir};

/// How long the instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

struct Run {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
    /// The site's origin (`Server::base`).
    base: String,
    /// A second site (another port, so another origin) that serves a favicon.
    alias: String,
    /// The engine kind under test (`ENGINE`, default webview2).
    engine: String,
}

impl Run {
    /// Settings (home page /b1, so NEWTAB opens B there) and a session holding tab A at /a.
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, &self.engine, &format!(", \"home_page\": \"{}/b1\"", self.base))?;
        let seed = self.profile.join("seed.json");
        let tab = serde_json::json!({"id": "a", "type": TAB, "url": format!("{}/a", self.base), "view_kind": self.engine,
            "pos": [0.0, 0.0]});
        std::fs::write(&seed, serde_json::json!({ "nodes": [tab] }).to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }

    /// Types `path` on the site into the focused tab's address bar and goes there.
    fn go(&self, inst: &mut Instance, path: &str) -> io::Result<()> {
        inst.send(&format!("ADDRESSBAR {}{path}", self.base))?;
        inst.send("WAITIDLE")?;
        inst.send("ENTER")
    }
}

/// The screen rects of EXPORTSTATE's labelled `rows`, as `[left, top, right, bottom]`.
fn rows(s: &State) -> Vec<Vec<Value>> {
    s.0["rows"]
        .as_array()
        .map_or_else(Vec::new, |rows| rows.iter().filter_map(|r| r["rect"].as_array().cloned()).collect())
}

/// How many icons of any kind EXPORTSTATE's `identities` lists.
fn icons(s: &State) -> usize {
    s.0["identities"].as_array().map_or(0, Vec::len)
}

/// The tile icons EXPORTSTATE's `identities` lists whose top edge is above `below` (all of them if `None`).
fn tiles(s: &State, below: Option<f64>) -> usize {
    let ids = s.0["identities"].as_array().map_or(&[][..], Vec::as_slice);
    ids.iter()
        .filter(|i| i["kind"] == "tile" && below.is_none_or(|y| i["rect"][1].as_f64().is_some_and(|top| top < y)))
        .count()
}

/// Tab `id`'s entry in EXPORTSTATE's `trail`.
fn trail_of<'a>(s: &'a State, id: &str) -> Option<&'a Value> {
    s.0["trail"].get(id)
}

/// The id of the one trail whose cursor URL ends with `path`.
fn trail_at(s: &State, path: &str) -> Option<String> {
    let trails = s.0["trail"].as_object()?;
    let ids: Vec<&String> =
        trails.iter().filter(|(_, t)| t["url"].as_str().is_some_and(|u| u.ends_with(path))).map(|(id, _)| id).collect();
    match ids.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

fn field<'a>(s: &'a State, id: &str, key: &str) -> Option<&'a Value> {
    trail_of(s, id).map(|t| &t[key])
}

/// The centre of a `[left, top, right, bottom]` rect.
fn centre(r: [f64; 4]) -> (f64, f64) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// Whether two `[left, top, right, bottom]` rects share area.
fn overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Parks a card other than the strip's tab at a free spot in the open, at 100% zoom, and returns the settled state with
/// its id; `None` if the card never holds the spot (the camera or the layout moves it after the move). The strip hangs
/// under its tab's card and follows it, so that card can never be brought over the strip; the layout puts any other
/// card anywhere, even off screen.
fn park_card(inst: &mut Instance) -> io::Result<Option<(State, String)>> {
    let is_strip = |c: &Value| c["virtual"].as_str() == Some("trail");
    for _ in 0..3 {
        // The strip's arrival eases the camera, so rects read now may be mid-ease by the time the pointer gets there.
        let (s, settled) =
            State::settle(|| inst.export_state("drag-settled"), 20, std::time::Duration::from_millis(300))?;
        if !settled {
            println!("  park: the cards were still moving");
            continue;
        }
        let (w, h) = (s.0["window"][0].as_f64().unwrap_or(0.0), s.0["window"][1].as_f64().unwrap_or(0.0));
        // A strip page's id is `virtual:trail:<tab>:<epoch>:<entry>`.
        let cards = s.all_cards();
        let strip_tab =
            cards.iter().find(|c| is_strip(c)).and_then(|c| c["id"].as_str()?.split(':').nth(2)).unwrap_or_default();
        let Some(moving) = cards.iter().find(|c| c["id"] != strip_tab && c["virtual"].is_null()) else {
            return Err(bad("no card besides the strip's tab to drag".to_string()));
        };
        let (Some(rect), Some(wx), Some(wy)) =
            (State::rect_from(&moving["rect"]), moving["world"][0].as_f64(), moving["world"][1].as_f64())
        else {
            return Err(bad("the card to drag has no rect or world position".to_string()));
        };
        let id = moving["id"].as_str().unwrap_or_default().to_string();
        // The camera is at 100%, so a world step is the same screen step. The spot is the first of a few in the open
        // at the sides that no other card covers.
        let size = (rect[2] - rect[0], rect[3] - rect[1]);
        let free = |spot: (f64, f64)| {
            let r = [spot.0 - size.0 / 2.0, spot.1 - size.1 / 2.0, spot.0 + size.0 / 2.0, spot.1 + size.1 / 2.0];
            let clear = cards
                .iter()
                .filter(|c| c["id"] != moving["id"])
                .filter_map(|c| State::rect_from(&c["rect"]))
                .all(|other| !overlap(r, other));
            clear && r[0] >= 0.0 && r[2] <= w && r[1] >= 0.0 && r[3] <= h
        };
        let spots = [(0.2, 0.55), (0.8, 0.55), (0.2, 0.25), (0.8, 0.25), (0.2, 0.85), (0.8, 0.85)];
        let Some(want) = spots.iter().map(|(fx, fy)| (w * fx, h * fy)).find(|&s| free(s)) else {
            return Err(bad("no free spot on screen to park the card to drag".to_string()));
        };
        let now = centre(rect);
        // Compared in world coordinates: the camera may still be easing, which moves the screen position.
        let target = (wx + want.0 - now.0, wy + want.1 - now.1);
        inst.send(&format!("MOVENODE {id} {} {}", target.0, target.1))?;
        let id_ref = id.as_str();
        let world_of = |s: &State| {
            let c = s.all_cards().iter().find(|c| c["id"] == id_ref)?;
            Some((c["world"][0].as_f64()?, c["world"][1].as_f64()?))
        };
        let at_spot =
            |s: &State| world_of(s).is_some_and(|w| (w.0 - target.0).abs() < 1.0 && (w.1 - target.1).abs() < 1.0);
        let s = inst.export_until("drag-moved", at_spot)?;
        if !at_spot(&s) {
            println!("  {id} at world {:?}, wanted {target:?}, moved from {:?}", world_of(&s), (wx, wy));
            continue;
        }
        let (s, _) = State::settle(|| inst.export_state("drag-parked"), 20, std::time::Duration::from_millis(300))?;
        // The camera can still move once the card is there, so the spot is only good if the card holds it on screen.
        if s.rect(&id).is_some_and(|r| (centre(r).0 - want.0).abs() < 1.0 && (centre(r).1 - want.1).abs() < 1.0) {
            return Ok(Some((s, id)));
        }
        println!("  park: {id} on screen at {:?}, wanted the centre {want:?}", s.rect(&id));
    }
    Ok(None)
}

/// Parks a card other than the strip's tab (`park_card`), presses it and drags it with real pointer moves until it lies
/// over the strip's second page, holds it there for a frame and a screenshot (kept in `RUN_DIR` for a look at the
/// layering), then moves it back and lets go.
fn drag_over_strip(inst: &mut Instance) -> io::Result<()> {
    inst.send("GRAPHZOOM 100")?;
    inst.send("WAITIDLE")?;
    let is_strip = |c: &Value| c["virtual"].as_str() == Some("trail");
    inst.export_until("drag-pre", |s| {
        s.all_cards().iter().filter(|c| is_strip(c)).count() == 4 && s.all_cards().iter().any(|c| c["id"].is_string())
    })?;
    let Some((s, id)) = park_card(inst)? else {
        check_is("the card to drag holds its spot", true, false);
        return Ok(());
    };
    let (cards, card) = (s.all_cards(), id.as_str());
    let (Some(from), Some(page)) = (
        cards.iter().find(|c| c["id"] == card).and_then(|c| State::rect_from(&c["rect"])),
        cards.iter().filter(|c| is_strip(c)).nth(1).and_then(|c| State::rect_from(&c["rect"])),
    ) else {
        return Err(bad("the card or the strip's second page has no screen rect".to_string()));
    };
    let (start, end) = (centre(from), centre(page));
    let page_before = page;
    let at = |f: f64| format!("{:.0} {:.0}", start.0 + (end.0 - start.0) * f, start.1 + (end.1 - start.1) * f);
    inst.send(&format!("MOUSEMOVE {}", at(0.0)))?;
    inst.send(&format!("MOUSEMOVE {}", at(0.0)))?;
    inst.send(&format!("MOUSEDOWN {}", at(0.0)))?;
    for i in 1..=8 {
        inst.send(&format!("MOUSEMOVE {}", at(f64::from(i) / 8.0)))?;
    }
    inst.send("WAITIDLE")?;
    inst.send(&format!("MOUSEMOVE {}", at(1.0)))?;
    let s = inst.export_until("drag-held", |s| s.0["dragging"].as_str() == Some(card))?;
    check_is("the press dragged the card", Some(card), s.0["dragging"].as_str());
    let held = s.all_cards().iter().find(|c| c["id"] == card).and_then(|c| State::rect_from(&c["rect"]));
    let page = s.all_cards().iter().filter(|c| is_strip(c)).nth(1).and_then(|c| State::rect_from(&c["rect"]));
    if !check_is("the held card lies over the strip's page", Some(true), held.zip(page).map(|(a, b)| overlap(a, b))) {
        println!("  held {held:?}, page {page:?}, grabbed {from:?}, page before {page_before:?}, pointer {end:?}");
    }
    if let Ok(png) = inst.screenshot("drag-over-strip") {
        if let Some(dir) = std::env::var_os("RUN_DIR") {
            let _ = std::fs::copy(png, std::path::Path::new(&dir).join("drag-over-strip.png"));
        }
    }
    for i in (0..=8).rev() {
        inst.send(&format!("MOUSEMOVE {}", at(f64::from(i) / 8.0)))?;
    }
    inst.send(&format!("MOUSEUP {}", at(0.0)))?;
    inst.send("WAITIDLE")
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
    let at = |s: &State, path: &str| s.focused_url().ends_with(path);

    next("open");
    let mut inst = Instance::launch(&run.exe, &run.profile, &run.work, "trail.log")?;
    for c in ["WAITTABS", "GRAPHRIGHT", "GRAPHOPEN", "WAITIDLE"] {
        inst.send(c)?;
    }
    let s = inst.export_until("a-open", |s| at(s, "/a") && trail_at(s, "/a").is_some())?;
    let a = trail_at(&s, "/a").ok_or_else(|| bad("tab A has no trail".to_string()))?;
    // NEWTAB leaves focus on A; NEXTTAB (creation order) moves it to B.
    for c in ["NEWTAB", "WAITTABS", "NEXTTAB", "WAITIDLE"] {
        inst.send(c)?;
    }
    let s = inst.export_until("b-open", |s| at(s, "/b1") && trail_at(s, "/b1").is_some_and(|b| b != a))?;
    let b = trail_at(&s, "/b1").ok_or_else(|| bad("tab B has no trail".to_string()))?;

    next("pages");
    for p in ["/b2", "/b3"] {
        run.go(&mut inst, p)?;
        inst.export_until(&format!("at-{}", p.trim_start_matches('/')), |s| at(s, p))?;
    }
    let s = inst.export_until("b3", |s| {
        field(s, &b, "entries") == Some(&3.into()) && field(s, &b, "depth") == Some(&2.into())
    })?;
    check_is("B's trail is /b1 /b2 /b3", Some(3), field(&s, &b, "entries").and_then(Value::as_u64));

    next("back");
    inst.send("GOBACK")?;
    let s = inst.export_until("back1", |s| at(s, "/b2") && field(s, &b, "depth") == Some(&1.into()))?;
    check_is("Forward is offered on /b2", Some(true), field(&s, &b, "forward").and_then(Value::as_bool));
    inst.send("GOBACK")?;
    let s = inst.export_until("back2", |s| at(s, "/b1") && field(s, &b, "depth") == Some(&0.into()))?;
    check_is(
        "Back at B's first page goes on (opened-from)",
        Some(true),
        field(&s, &b, "back").and_then(Value::as_bool),
    );
    inst.send("GOBACK")?;
    let s = inst.export_until("parent", |s| at(s, "/a"))?;
    check_is("A remembers it was left for B", Some(b.as_str()), field(&s, &a, "left_to").and_then(Value::as_str));
    check_is("Forward is offered on A", Some(true), field(&s, &a, "forward").and_then(Value::as_bool));

    next("forward");
    inst.send("GOFORWARD")?;
    let s = inst.export_until("child", |s| at(s, "/b1"))?;
    check_is("Forward returned to B's page", Some(0), field(&s, &b, "depth").and_then(Value::as_u64));

    next("fork");
    run.go(&mut inst, "/b4")?;
    let s = inst.export_until("fork", |s| at(s, "/b4") && field(s, &b, "entries") == Some(&4.into()))?;
    check_is("/b4 is a child of /b1", Some(1), field(&s, &b, "depth").and_then(Value::as_u64));
    check_is("the Back chip shows the branch marker", Some(true), field(&s, &b, "back_fork").and_then(Value::as_bool));

    next("strip");
    // The Graph view draws the focused tab's trail: one virtual card per page (spec trail-strip decision 10).
    inst.send("TOGGLEVIEW")?;
    let strip = |s: &State| s.all_cards().iter().filter(|c| c["virtual"].as_str() == Some("trail")).count();
    let s = inst.export_until("strip", |s| !s.tab_view() && strip(s) == 4)?;
    check_is("the strip has a card per page", 4, strip(&s));
    // Entering Graph view frames a strip that appeared off screen (`frame_new_strip`); the zoom below keeps the
    // camera's centre, so let it arrive first.
    let on_screen = |s: &State| {
        let (w, h) = (s.0["window"][0].as_f64().unwrap_or(0.0), s.0["window"][1].as_f64().unwrap_or(0.0));
        let cards = s.all_cards();
        let strip = cards.iter().filter(|c| c["virtual"].as_str() == Some("trail"));
        strip.clone().count() == 4
            && strip.into_iter().all(|c| {
                let at = |i: usize| c["screen"][i].as_f64().unwrap_or(-1.0);
                (0.0..w).contains(&at(0)) && (0.0..h).contains(&at(1))
            })
    };
    let s = inst.export_until("strip-framed", on_screen)?;
    if !check_is("the strip is on screen after the toggle", true, on_screen(&s)) {
        // This failed once in 30 solo reruns (PARKED.md): keep what the wait saw, since the work dir is deleted.
        let rects: Vec<_> = s
            .all_cards()
            .iter()
            .filter(|c| c["virtual"].as_str() == Some("trail"))
            .map(|c| c["screen"].clone())
            .collect();
        println!(
            "  strip-framed timed out: window {}, tab_view {}, strip screen rects {rects:?}",
            s.0["window"],
            s.tab_view()
        );
        if let Some(dir) = std::env::var_os("RUN_DIR") {
            if let Ok(png) = inst.screenshot("strip-framed-failed") {
                let _ = std::fs::copy(png, std::path::Path::new(&dir).join("strip-framed-failed.png"));
            }
        }
    }
    next("drag");
    drag_over_strip(&mut inst)?;
    // At 60% the cards are short: each tab and each page leads with the tile in the engine's color.
    inst.send("GRAPHZOOM 60")?;
    inst.send("WAITIDLE")?;
    // A tab card shows a spinner until its thumbnail exists, so only the four page tiles are certain.
    let s = inst.export_until("strip-zoom", |s| tiles(s, None) >= 4 && icons(s) >= 6)?;
    check_is("four page tiles and two tab icons are drawn", true, tiles(&s, None) >= 4 && icons(&s) >= 6);
    inst.send("TOGGLEVIEW")?;
    inst.export_until("strip-off", |s| s.tab_view() && at(s, "/b4"))?;
    inst.send("WAITIDLE")?;
    // The address bar's icon: the same tile (these local pages have no favicon), in the toolbar band.
    let s = inst.export_until("address-icon", |s| tiles(s, Some(40.0)) >= 1)?;
    check_is("the address bar has an icon tile", true, tiles(&s, Some(40.0)) >= 1);
    // The dropdown: each open-tab suggestion is a labelled row led by an icon tile. Suggestions only recompute when
    // the text changes, so type, then change it.
    inst.send("ADDRESSBAR 127")?;
    inst.send("EGUITEXT .")?;
    let s = inst.export_until("dropdown", |s| !rows(s).is_empty())?;
    let led = |s: &State| {
        let ids = s.0["identities"].as_array().map_or(&[][..], Vec::as_slice);
        rows(s)
            .iter()
            .filter(|r| {
                ids.iter().any(|i| {
                    i["rect"][0].as_f64() > r[0].as_f64()
                        && i["rect"][1].as_f64() >= r[1].as_f64()
                        && i["rect"][3].as_f64() <= r[3].as_f64()
                })
            })
            .count()
    };
    check_is("every dropdown row has an icon inside it", rows(&s).len(), led(&s));
    let selected = |s: &State| s.0["rows"].as_array().map_or(0, |r| r.iter().filter(|r| r["selected"] == true).count());
    check_is("no dropdown row is selected yet", 0, selected(&s));
    inst.send("ARROWDOWN")?;
    let s = inst.export_until("dropdown-selected", |s| selected(s) == 1)?;
    check_is("ArrowDown selects one dropdown row", 1, selected(&s));
    inst.send("ESCAPE")?;

    next("script");
    // A page that calls history.back() itself: the engine goes Back with no host move, and the trail must read it as a
    // Back (the cursor returns to /b4 and no entry is added), not as a new page.
    run.go(&mut inst, "/back")?;
    inst.export_until("script-page", |s| at(s, "/back"))?;
    let s = inst.export_until("script-back", |s| at(s, "/b4"))?;
    check_is("the page's own Back adds no entry", Some(5), field(&s, &b, "entries").and_then(Value::as_u64));
    check_is("Forward is offered after it", Some(true), field(&s, &b, "forward").and_then(Value::as_bool));
    next("favicon");
    // A page whose origin serves a favicon shows it, not a tile: send B to the second origin and open the
    // dropdown; its rows ask for that origin's favicon and then lead with an image. Then B goes back to the first
    // origin, so the next stage's tile is a tile.
    inst.send(&format!("ADDRESSBAR {}/fav", run.alias))?;
    inst.send("WAITIDLE")?;
    inst.send("ENTER")?;
    inst.export_until("fav-page", |s| s.0["focused_url"].as_str().is_some_and(|u| u.ends_with("/fav")))?;
    // The focused tab is not offered to itself: look at it from tab A.
    inst.send("NEXTTAB")?;
    inst.send("WAITIDLE")?;
    inst.send("ADDRESSBAR 127")?;
    inst.send("EGUITEXT .")?;
    let images =
        |s: &State| s.0["identities"].as_array().map_or(0, |ids| ids.iter().filter(|i| i["kind"] == "image").count());
    let s = inst.export_until("fav-dropdown", |s| images(s) >= 1)?;
    check_is("a page whose origin serves a favicon shows it as an image", true, images(&s) >= 1);
    inst.send("ESCAPE")?;
    inst.send("NEXTTAB")?;
    inst.send("WAITIDLE")?;
    run.go(&mut inst, "/b4")?;
    inst.export_until("fav-back", |s| at(s, "/b4"))?;

    next("closed-popup");
    // A closed tab's row in Recently Closed leads with the same tile (no favicon on these local pages).
    inst.send(&format!("CLOSENODE {b}"))?;
    inst.send("TOGGLEVIEW")?;
    inst.send("TOGGLEPOPUP recentlyclosed")?;
    let in_popup = |s: &State| {
        let popup = s.0["popups"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["id"].as_str().is_some_and(|id| id.contains("recently_closed_popup"))))?;
        let at = |v: &Value, i: usize| v["rect"][i].as_f64();
        let ids = s.0["identities"].as_array()?;
        Some(
            ids.iter()
                .filter(|i| {
                    i["kind"] == "tile"
                        && at(i, 0) >= at(popup, 0)
                        && at(i, 2) <= at(popup, 2)
                        && at(i, 1) >= at(popup, 1)
                        && at(i, 3) <= at(popup, 3)
                })
                .count(),
        )
    };
    let s = inst.export_until("closed-popup", |s| in_popup(s).is_some_and(|n| n >= 1))?;
    check_is("Recently Closed lists the closed tab behind a tile", true, in_popup(&s).is_some_and(|n| n >= 1));
    inst.quit(QUIT_TIMEOUT).map(|_| ())
}

pub fn trail(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "trail") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    fn page(line: &str) -> String {
        let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
        let script = if path == "/back" { "<script>setTimeout(() => history.back(), 500)</script>" } else { "" };
        format!("<!doctype html><title>{path}</title><p>{path}</p>{script}")
    }
    let start = |h: Arc<Handler>| match Server::start(h) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("site: {e}");
            None
        }
    };
    let Some(server) = start(Arc::new(move |line: &str| response("200 OK", &[], "text/html", page(line).as_bytes())))
    else {
        return ExitCode::from(1);
    };
    // A second origin (another port) whose `/favicon.ico` is a red 16x16 PNG; the first origin's pages keep their tiles.
    let mut png = Vec::new();
    let red = image::RgbaImage::from_pixel(16, 16, image::Rgba([220, 40, 40, 255]));
    red.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("encode the favicon");
    let Some(fav_server) = start(Arc::new(move |line: &str| {
        if line.split(' ').nth(1) == Some("/favicon.ico") {
            return response("200 OK", &[], "image/png", &png);
        }
        response("200 OK", &[], "text/html", page(line).as_bytes())
    })) else {
        return ExitCode::from(1);
    };
    let run = Run {
        exe,
        profile: work.join("profile"),
        work,
        base: server.base(),
        alias: fav_server.base(),
        engine: std::env::var("ENGINE").unwrap_or_else(|_| "webview2".to_owned()),
    };
    if let Err(e) = run.setup().and_then(|()| drive(&run, &mut sec)) {
        println!("ERROR: trail stopped early: {e}");
        note_failure();
    }
    if failures() > 0 {
        println!("--- trail.log\n{}", no_verdict_words(&log_tail(&run.work.join("trail.log"), 30)));
    }
    drop(sec);
    drop(server);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}
