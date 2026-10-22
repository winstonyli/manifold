//! `gate smoke` (was scripts/headless-smoke-test.sh): one headless instance on a fresh profile, driven through 11
//! sections (labels and TIMING names as the script had them), with the suggestions endpoint of `suggest.rs` standing
//! in for DuckDuckGo. Each check is an inline closure over the exported `State`; the expression's text rides along
//! only for the FAIL line.
//!
//! The script's waits are kept as they are (this is a port, not a tuning pass): `wait_for_file`'s 30 s window and
//! size-stability rule apply to every export, `settle` re-exports until two in a row agree (0.3 s apart, 40 tries),
//! `wait_state` polls 30 times 0.5 s apart, the conversion poll 150 times 0.3 s apart.
//!
//! A failing check does not stop the run; a runner error (an export that never appears, a card the next command
//! needs that is not there) prints an `ERROR:` line, counts as a failure and ends the run. The instance is killed on
//! every exit path (`Instance`'s Drop plus the job object).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;

use crate::check::{exit_code, fail_line, failures, note_failure, pass_line, Marks};
use crate::instance::{file_url, log_tail, remove_dir_with_retries, Instance};
use crate::link::{bad, no_verdict_words, truthy};
use crate::state::{fmt0, round_half_even, State, DOWNLOAD, IMAGE, JAR, JSON, NOTE, PDF, TAB};
use crate::suggest::{query_param, url_query, Server};
use crate::windows_main::{manifold_exe, work_dir};

/// How long the instance may take to exit after QUIT (the script waits without a limit).
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The script's `sleep 0.2` / `sleep 0.3` / `sleep 0.5` polls.
const FILE_POLL: Duration = Duration::from_millis(200);
const SETTLE_POLL: Duration = Duration::from_millis(300);
const STATE_POLL: Duration = Duration::from_millis(500);

fn pass(label: &str) {
    println!("{}", pass_line(label));
}

/// The script's `failed`.
fn failed(label: &str) {
    println!("{}", fail_line(label));
    note_failure();
}

fn timed_out(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, what)
}

fn nonempty(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

/// `wait_for_file`: up to 30 s (150 x 0.2 s) for a non-empty file, then until its size stops changing (a save in
/// progress). Returns without error either way; the caller looks at what is there.
fn wait_for_file(path: &Path) {
    let mut waited = 0;
    while !nonempty(path) && waited < 150 {
        sleep(FILE_POLL);
        waited += 1;
    }
    let mut prev = None;
    while let Some(now) = fs::metadata(path).ok().filter(fs::Metadata::is_file).map(|m| m.len()) {
        if prev == Some(now) {
            break;
        }
        prev = Some(now);
        sleep(FILE_POLL);
    }
}

// ---- the `state` namespace: `s`, `url`, `q`; the closure captures `row` ----

/// What a check closure gets: the export, the focused tab's URL (`url`) and that URL's `q=`
/// search query (`q`, parse_qs without blanks).
struct View<'a> {
    s: &'a State,
    url: &'a str,
    q: String,
}

impl<'a> View<'a> {
    fn new(s: &'a State) -> View<'a> {
        let url = s.focused_url();
        View { s, url, q: query_param(url_query(url), "q", false).unwrap_or_default() }
    }
}

// ---- the `query` namespace: small accessors; a missing key is a python KeyError, which fails the check ----

fn key<'a>(s: &'a State, k: &str) -> Option<&'a Value> {
    s.0.get(k)
}

/// `s[k]` is truthy (a missing key raises in python, so it is false here).
fn truthy_key(s: &State, k: &str) -> bool {
    key(s, k).is_some_and(truthy)
}

/// `not s[k]`.
fn falsy_key(s: &State, k: &str) -> bool {
    key(s, k).is_some_and(|v| !truthy(v))
}

/// `s[k] is None`.
fn null_key(s: &State, k: &str) -> bool {
    key(s, k).is_some_and(Value::is_null)
}

/// `s['current_group'] == id`.
fn in_group(s: &State, id: &str) -> bool {
    s.current_group() == Some(id)
}

/// `s['selected']` as a list of ids.
fn selected(s: &State) -> Option<Vec<String>> {
    str_list(key(s, "selected")?)
}

fn str_list(v: &Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(|x| x.as_str().map(str::to_string)).collect()
}

fn floats(v: &Value, n: usize) -> Option<Vec<f64>> {
    let a = v.as_array()?;
    if a.len() < n {
        return None;
    }
    a.iter().take(n).map(Value::as_f64).collect()
}

/// `cards[id]['screen']`.
fn screen_of(s: &State, id: &str) -> Option<Vec<f64>> {
    floats(s.card(id)?.get("screen")?, 2)
}

/// `cards[id]['rect']`.
fn rect_of(s: &State, id: &str) -> Option<[f64; 4]> {
    s.rect(id)
}

/// `sorted(cards)`: every card id, sorted.
fn sorted_ids(s: &State) -> Vec<String> {
    s.all_cards()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// `([i for i, c in cards.items() if card_is(c, TYPE, loaded) and i not in before] or ["none"])[0]`.
fn new_card(s: &State, ty: &str, loaded: Option<bool>, before: &[String]) -> String {
    s.all_cards()
        .iter()
        .filter(|c| State::card_is(c, ty, loaded))
        .filter_map(|c| c["id"].as_str())
        .find(|id| !before.iter().any(|b| b == id))
        .unwrap_or("none")
        .to_string()
}

/// `[g for g in groups if g not in before][0]`, `None` where python raises IndexError.
fn new_group(s: &State, before: &[String]) -> Option<String> {
    s.groups().into_iter().find(|g| !before.contains(g))
}

fn count_of(s: &State, ty: &str) -> usize {
    s.all_cards().iter().filter(|c| State::card_is(c, ty, None)).count()
}

/// `abs(cards[id]['screen'][0] - x) < 200 and abs(...[1] - y) < 200`.
fn within_200(s: &State, id: &str, x: f64, y: f64) -> bool {
    screen_of(s, id).is_some_and(|c| (c[0] - x).abs() < 200.0 && (c[1] - y).abs() < 200.0)
}

/// `(lambda a, b: a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1])(rect a, rect b)`.
fn apart(s: &State, a: &str, b: &str) -> bool {
    match (rect_of(s, a), rect_of(s, b)) {
        (Some(a), Some(b)) => a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1],
        _ => false,
    }
}

fn json_level(s: &State) -> &Value {
    &s.0["json_level"]
}

/// `next(c for c in s['json_level']['cards'] if c['pointer'] == '/items')`.
fn items_card(s: &State) -> Option<&Value> {
    json_level(s)["cards"].as_array()?.iter().find(|c| c["pointer"] == "/items")
}

fn skip_is(s: &State, n: f64) -> bool {
    items_card(s).is_some_and(|c| c["skip"].as_f64() == Some(n))
}

/// The `/items` card's `rows`, `more` and `collapsed`, for the paging and toggle checks.
fn items_rows_more(s: &State) -> Option<(f64, f64, bool)> {
    let c = items_card(s)?;
    Some((c["rows"].as_f64()?, c["more"].as_f64()?, c["collapsed"].as_bool()?))
}

/// `' '.join('%.0f' % v for v in ITEMS['earlier'])`.
fn earlier_point(s: &State) -> Option<String> {
    let a = items_card(s)?.get("earlier")?.as_array()?;
    Some(a.iter().map(|v| v.as_f64().map(fmt0)).collect::<Option<Vec<_>>>()?.join(" "))
}

/// Python's `repr` of a list of ids (they never hold a quote): `['a', 'b']`.
fn py_list(ids: &[String]) -> String {
    format!("[{}]", ids.iter().map(|i| format!("'{i}'")).collect::<Vec<_>>().join(", "))
}

/// check_query's "saw:" text.
fn saw(s: &State) -> String {
    format!(
        "tab_view={} group={} tabs={} groups={}",
        if s.tab_view() { "True" } else { "False" },
        s.current_group().unwrap_or("None"),
        py_list(&s.tabs()),
        py_list(&s.groups()),
    )
}

/// `converted`: a node of `kind` ("json" or "pdf") whose url is a `file://` one ending in `/NAME`,
/// and no tab node at such a url.
fn converted(session: &Value, name: &str, kind: &str) -> bool {
    let want = match kind {
        "json" => JSON,
        "pdf" => PDF,
        _ => return false,
    };
    let Some(nodes) = session["nodes"].as_array() else { return false };
    let suffix = format!("/{name}");
    let at = |n: &Value| n["url"].as_str().is_some_and(|u| u.starts_with("file://") && u.ends_with(&suffix));
    nodes.iter().any(|n| n["type"] == want && at(n)) && !nodes.iter().any(|n| n["type"] == TAB && at(n))
}

/// Whether the card `id`'s border, sampled at the middle of each of its four edges, is within 40 (RGB distance) of
/// `want` at any of them (another card can cover one edge: the group's left edge was once hidden by a tab card).
/// Pixels off the image are skipped; a card or border with no pixel on the image fails.
fn border_near(s: &State, png: &Path, id: &str, want: [u8; 3]) -> bool {
    let Some(r) = rect_of(s, id) else { return false };
    let Ok(img) = crate::shots_img::load(png) else { return false };
    let (mid_x, mid_y) = (round_half_even((r[0] + r[2]) / 2.0) as i64, round_half_even((r[1] + r[3]) / 2.0) as i64);
    let (left, top) = (round_half_even(r[0]) as i64, round_half_even(r[1]) as i64);
    let (right, bottom) = (round_half_even(r[2]) as i64, round_half_even(r[3]) as i64);
    let mut near = f64::INFINITY;
    for (cx, cy, step) in
        [(left, mid_y, (1, 0)), (right - 3, mid_y, (1, 0)), (mid_x, top, (0, 1)), (mid_x, bottom - 3, (0, 1))]
    {
        for k in -1..3 {
            let (x, y) = (cx + k * step.0, cy + k * step.1);
            let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) else { continue };
            if px >= img.width() || py >= img.height() {
                continue;
            }
            let p = img.get_pixel(px, py).0;
            let d2: i64 = p.iter().zip(want).map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2)).sum();
            near = near.min((d2 as f64).sqrt());
        }
    }
    near < 40.0
}

/// How many pixels inside the popups showing in `s` are within 12 (RGB distance) of `want`.
fn fill_pixels(s: &State, png: &Path, want: [u8; 3]) -> usize {
    let Ok(img) = crate::shots_img::load(png) else { return 0 };
    let rects: Vec<Vec<f64>> =
        s.0["popups"].as_array().map(|l| l.iter().filter_map(|p| floats(&p["rect"], 4)).collect()).unwrap_or_default();
    let mut n = 0;
    for r in rects {
        for y in r[1].max(0.0) as u32..(r[3].max(0.0) as u32).min(img.height()) {
            for x in r[0].max(0.0) as u32..(r[2].max(0.0) as u32).min(img.width()) {
                let p = img.get_pixel(x, y).0;
                let d2: i64 = p.iter().zip(want).map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2)).sum();
                n += usize::from(d2 < 144);
            }
        }
    }
    n
}

/// `"x y"` -> ints.
fn ints(text: &str) -> io::Result<Vec<i64>> {
    text.split_whitespace()
        .map(|w| w.parse::<i64>().map_err(|_| bad(format!("not an integer: {w:?} in {text:?}"))))
        .collect()
}

fn pair(text: &str) -> io::Result<(i64, i64)> {
    match ints(text)?[..] {
        [x, y] => Ok((x, y)),
        _ => Err(bad(format!("expected \"x y\", got {text:?}"))),
    }
}

/// `xy(id)`: the card's centre as "x y", or a runner error (the script would send an empty command).
fn xy(s: &State, id: &str) -> io::Result<String> {
    s.xy(id).ok_or_else(|| bad(format!("no card {id:?} to aim at")))
}

fn first<'a>(v: &'a [String], what: &str) -> io::Result<&'a String> {
    v.first().ok_or_else(|| bad(format!("no {what}")))
}

struct Smoke {
    inst: Instance,
    /// Where the exports, screenshots and test files go (`$out_dir`).
    out: PathBuf,
    repo: PathBuf,
}

impl Smoke {
    fn send(&mut self, line: &str) -> io::Result<()> {
        self.inst.send(line)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.out.join(file)
    }

    /// `$out_dir_win\file`.
    fn win(&self, file: &str) -> String {
        self.path(file).display().to_string()
    }

    /// Sends `CMD out\FILE` (removing any old file first) and waits for it.
    fn export(&mut self, cmd: &str, file: &str) -> io::Result<PathBuf> {
        let path = self.path(file);
        let _ = fs::remove_file(&path);
        self.send(&format!("{cmd} {}", path.display()))?;
        wait_for_file(&path);
        if nonempty(&path) {
            Ok(path)
        } else {
            Err(timed_out(format!("{cmd}: {} was not written", path.display())))
        }
    }

    /// `state NAME 1`: EXPORTSTATE to NAME.json, parsed.
    fn state(&mut self, name: &str) -> io::Result<State> {
        let path = self.export("EXPORTSTATE", &format!("{name}.json"))?;
        State::parse(&fs::read_to_string(path)?).map_err(|e| bad(format!("{name}.json: {e}")))
    }

    /// `send "SCREENSHOT ..."` then `check_screenshot` (at least 2000 bytes).
    fn shot(&mut self, file: &str, label: &str) -> io::Result<()> {
        let path = self.path(file);
        self.send(&format!("SCREENSHOT {}", path.display()))?;
        wait_for_file(&path);
        match fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() < 2000 => {
                failed(&format!("{label}: screenshot suspiciously small ({} bytes)", m.len()));
            }
            Ok(m) if m.is_file() => pass(&format!("{label}: screenshot looks reasonable ({} bytes)", m.len())),
            _ => failed(&format!("{label}: screenshot not created ({})", path.display())),
        }
        Ok(())
    }

    /// `check_state`: exports NAME.json and passes if `f` holds; the PASS line carries the focused tab's URL.
    fn check_state(&mut self, name: &str, label: &str, expr: &str, f: impl Fn(&View) -> bool) -> io::Result<State> {
        let s = self.state(name)?;
        let v = View::new(&s);
        if f(&v) {
            pass(&format!("{label} ({})", v.url));
        } else {
            let tab = if v.url.is_empty() { "none" } else { v.url };
            failed(&format!("{label}: `{expr}` is false (focused tab: {tab})"));
        }
        Ok(s)
    }

    /// `wait_state`: polls (30 x 0.5 s) until `f` holds.
    fn wait_state(&mut self, name: &str, label: &str, expr: &str, f: impl Fn(&View) -> bool) -> io::Result<()> {
        for _ in 0..30 {
            let s = self.state(name)?;
            if f(&View::new(&s)) {
                pass(label);
                return Ok(());
            }
            sleep(STATE_POLL);
        }
        failed(&format!("{label}: `{expr}` still false after ~15 s"));
        Ok(())
    }

    /// `settle`: exports until two in a row agree on every card's rect; returns the last. A layout that never stops
    /// is a FAIL line and the run goes on.
    fn settle(&mut self, name: &str) -> io::Result<State> {
        let (last, settled) = State::settle(|| self.state(name), 40, SETTLE_POLL)?;
        if !settled {
            failed(&format!("{name}: the layout never stopped moving"));
        }
        Ok(last)
    }

    /// `drag X0 Y0 X1 Y1 [button]`: a press, six moves and a release with no sleep between them.
    fn drag(&mut self, from: (i64, i64), to: (i64, i64), button: Option<&str>) -> io::Result<()> {
        let b = button.map_or(String::new(), |b| format!(" {b}"));
        self.send(&format!("MOUSEDOWN {} {}{b}", from.0, from.1))?;
        for i in 1..=6 {
            self.send(&format!("MOUSEMOVE {} {}", from.0 + (to.0 - from.0) * i / 6, from.1 + (to.1 - from.1) * i / 6))?;
        }
        self.send(&format!("MOUSEUP {} {}{b}", to.0, to.1))
    }

    fn write(&self, file: &str, bytes: &[u8]) -> io::Result<()> {
        fs::write(self.path(file), bytes)
    }
}

/// check_query: passes if `f` holds over the already-exported state `s` (named `name`, for the message).
fn check_query(s: &State, name: &str, label: &str, expr: &str, f: impl Fn(&State) -> bool) {
    if f(s) {
        pass(label);
    } else {
        failed(&format!("{label}: `{expr}` is false in {name}.json (saw: {})", saw(s)));
    }
}

/// `check_query` of `'ID' != 'none'`.
fn check_found(s: &State, name: &str, label: &str, id: &str) {
    check_query(s, name, label, &format!("'{id}' != 'none'"), |_| id != "none");
}

const PAIR_GROUP: &str = r#"{"nodes": [
 {"id": "g", "type": "application/vnd.manifold.group", "name": "Pair"},
 {"id": "a", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "view_kind": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "view_kind": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
"#;

/// `nine`: a group "Nine" of nine tabs, for IMPORTGROUP.
fn nine_group() -> Value {
    let tab = |i: usize| {
        serde_json::json!({"id": format!("t{i}"), "type": TAB, "url": "manifold://welcome", "view_kind": "webview2",
            "group": "g"})
    };
    let mut nodes = vec![serde_json::json!({"id": "g", "type": "application/vnd.manifold.group", "name": "Nine"})];
    nodes.extend((0..9).map(tab));
    serde_json::json!({ "nodes": nodes })
}

/// The 11 sections after setup.
fn drive(sm: &mut Smoke, m: &mut Marks) -> io::Result<()> {
    m.mark("launch");
    // Pin a known window size so the click coordinates below stay valid.
    sm.send("RESIZE 816 639")?;
    // The root tab spawns asynchronously (30 s or more on a loaded machine).
    sm.send("WAITTABS")?;

    m.mark("address-bar");
    // This DOUBLECLICK is the first command after the tab spawns; keep it first (see the script's comment).
    sm.send("DOUBLECLICK 407 332")?;
    sm.send("WAITIDLE")?;
    sm.check_state("tab_view", "clicking the just-spawned root node enters Tab view", r#"s["tab_view"]"#, |v| {
        truthy_key(v.s, "tab_view")
    })?;

    sm.send("ADDRESSBAR weather")?;
    sm.wait_state("weather_rows", "suggestions for \"weather\" arrive", r#"s["suggestions"]"#, |v| {
        truthy_key(v.s, "suggestions")
    })?;
    sm.shot("dropdown.png", "autocomplete dropdown")?;
    let first_row = sm.state("weather_rows")?.0["suggestions"][0].as_str().unwrap_or_default().to_string();

    // First suggestion row's position below the address bar, at the 816x639 size pinned above.
    sm.send("CLICK 283 42")?;
    sm.wait_state("after_click", "clicking the first suggestion searches it", "q == row", |v| v.q == first_row)?;
    sm.send("WAITIDLE")?;
    sm.shot("after_click.png", "post-click navigation")?;

    // Arrow-key highlight + Enter-accept: two ARROWDOWNs highlight the 2nd row.
    sm.send("ADDRESSBAR sourdough bread")?;
    sm.wait_state(
        "sourdough_rows",
        "suggestions for \"sourdough bread\" arrive",
        r#"len(s["suggestions"]) >= 2"#,
        |v| v.s.0["suggestions"].as_array().is_some_and(|a| a.len() >= 2),
    )?;
    sm.send("ARROWDOWN")?;
    sm.send("ARROWDOWN")?;
    sm.check_state("highlighted", "two ARROWDOWNs highlight the 2nd row", r#"s["highlighted"] == 1"#, |v| {
        key(v.s, "highlighted").and_then(Value::as_f64) == Some(1.0)
    })?;
    let second_row = sm.state("highlighted")?.0["suggestions"][1].as_str().unwrap_or_default().to_string();
    sm.send("ENTER")?;
    sm.wait_state("arrow_nav", "Enter searches the highlighted row", "q == row", |v| v.q == second_row)?;
    sm.send("WAITIDLE")?;
    sm.shot("arrow_nav.png", "arrow-key navigation")?;

    // Escape dismisses the dropdown without leaving Tab view, and navigates nowhere.
    sm.send("ADDRESSBAR pancake recipe")?;
    sm.wait_state("pancake_rows", "suggestions for \"pancake recipe\" arrive", r#"s["suggestions"]"#, |v| {
        truthy_key(v.s, "suggestions")
    })?;
    sm.send("ESCAPE")?;
    sm.shot("escape_dismiss.png", "Escape dismiss")?;
    sm.check_state(
        "escape_dismiss",
        "Escape closes the dropdown, stays in Tab view, navigates nowhere",
        r#"s["tab_view"] and not s["suggestions"] and q == row"#,
        |v| truthy_key(v.s, "tab_view") && falsy_key(v.s, "suggestions") && v.q == second_row,
    )?;

    m.mark("group-drill");
    // A second Escape (no dropdown now) leaves to Graph view, NEWGROUP adds a group card, DOUBLECLICK enters it and
    // the toolbar's back button leaves it; Reset View first brings every card into view.
    sm.send("ESCAPE")?;
    sm.send("NEWGROUP")?;
    sm.send("WAITIDLE")?;
    sm.shot("group_card.png", "group card")?;
    // A "Saved <path>" toast would cover the bottom-left of the window.
    sm.check_state("group_card", "a headless screenshot shows no toast", "s['toast'] is None", |v| {
        null_key(v.s, "toast")
    })?;
    sm.send("CLICK 326 17")?;
    let s = sm.settle("group_card")?;
    let first_group = first(&s.groups(), "group card")?.clone();
    sm.send(&format!("DOUBLECLICK {}", xy(&s, &first_group)?))?;
    sm.check_state(
        "group_in",
        "double-clicking the group card enters the group",
        r#"not s["tab_view"] and s["current_group"]"#,
        |v| falsy_key(v.s, "tab_view") && key(v.s, "current_group").is_some_and(truthy),
    )?;
    sm.send("CLICK 22 17")?;
    let group_out =
        sm.check_state("group_out", "the back button leaves the group", r#"s["current_group"] is None"#, |v| {
            null_key(v.s, "current_group")
        })?;

    m.mark("group-drops");
    // A two-tab group from a file, entered by double-clicking its card.
    sm.write("pair.group.json", PAIR_GROUP.as_bytes())?;
    // `query group_out` reads the file the check above wrote
    let before = group_out.groups();
    sm.send(&format!("IMPORTGROUP {}", sm.win("pair.group.json")))?;
    sm.send("WAITTABS")?;
    // Reset View (fit all) brings the imported card into view.
    sm.send("CLICK 326 17")?;
    let pair_top = sm.settle("pair_top")?;
    let pair_id = new_group(&pair_top, &before).ok_or_else(|| bad("no imported group card".into()))?;
    sm.send(&format!("DOUBLECLICK {}", xy(&pair_top, &pair_id)?))?;
    let pair_in = sm.settle("pair_in")?;
    check_query(
        &pair_in,
        "pair_in",
        "double-clicking the imported group's card enters it",
        &format!("s['current_group'] == '{pair_id}' and len(tabs) == 2"),
        |s| in_group(s, &pair_id) && s.tabs().len() == 2,
    );
    let tabs = pair_in.tabs();
    let (m1, m2) = match &tabs[..] {
        [a, b, ..] => (a.clone(), b.clone()),
        _ => return Err(bad(format!("the imported group shows {} tabs, not 2", tabs.len()))),
    };

    sm.drag(pair(&xy(&pair_in, &m1)?)?, pair(&xy(&pair_in, &m2)?)?, None)?;
    let reparented = sm.settle("reparented")?;
    check_query(
        &reparented,
        "reparented",
        "a tab dropped on a sibling tab inside a group is reparented under it",
        &format!("cards['{m1}']['parent'] == '{m2}'"),
        |s| s.card(&m1).is_some_and(|c| c["parent"] == m2.as_str()),
    );
    // The pair now overlaps exactly, and which is on top follows spawn order, so move whichever the press picks up
    // onto empty canvas first, toward the window's middle so it stays in view.
    let (x, y) = pair(&xy(&reparented, &m1)?)?;
    sm.drag((x, y), (if x > 408 { x - 250 } else { x + 250 }, if y > 340 { y - 150 } else { y + 150 }), None)?;
    let apart_s = sm.settle("apart")?;
    sm.drag(pair(&xy(&apart_s, &m2)?)?, pair(&xy(&apart_s, &m1)?)?, None)?;
    let cycle = sm.settle("cycle")?;
    let prev = screen_of(&apart_s, &m2);
    check_query(
        &cycle,
        "cycle",
        "dropping a tab onto its own child is refused",
        &format!(
            "cards['{m2}']['screen'] != eval(env['PREV']) and cards['{m2}']['parent'] is None and cards['{m1}']['parent'] == '{m2}'"
        ),
        |s| {
            screen_of(s, &m2).is_some_and(|now| prev.as_ref().is_some_and(|p| *p != now))
                && s.card(&m2).is_some_and(|c| c.get("parent").is_some_and(Value::is_null))
                && s.card(&m1).is_some_and(|c| c["parent"] == m2.as_str())
        },
    );

    // A right-drag marquee selects both; ejecting one through its button takes it out of the selection too.
    let bbox = ints(&cycle.bbox())?;
    let [bx0, by0, bx1, by1] = bbox[..] else { return Err(bad(format!("no tab bounding box: {bbox:?}"))) };
    sm.drag((bx0, by0), (bx1, by1), Some("right"))?;
    let selected_s = sm.settle("selected")?;
    check_query(&selected_s, "selected", "a right-drag marquee selects both tabs", "s['selected'] == tabs", |s| {
        selected(s).is_some_and(|sel| sel == s.tabs())
    });
    let (px, py) = pair(&xy(&selected_s, &m1)?)?;
    let lt = selected_s.lt(&m1).ok_or_else(|| bad(format!("no card {m1:?}")))?;
    let (ex, ey) = pair(&lt)?;
    sm.send(&format!("MOUSEMOVE {px} {py}"))?;
    sm.send(&format!("MOUSEMOVE {ex} {ey}"))?;
    sm.send(&format!("CLICK {ex} {ey}"))?;
    let ejected = sm.settle("ejected")?;
    check_query(
        &ejected,
        "ejected",
        "ejecting a selected tab also drops it from the selection",
        "len(tabs) == 1 and s['selected'] == tabs",
        |s| s.tabs().len() == 1 && selected(s).is_some_and(|sel| sel == s.tabs()),
    );

    // Ctrl-click toggles the selected tab out and back in without opening it; a plain click on empty canvas (the
    // marquee's start corner) then clears the selection.
    let only = first(&ejected.tabs(), "tab left after the eject")?.clone();
    let (x, y) = pair(&xy(&ejected, &only)?)?;
    let bb = ints(&ejected.bbox())?;
    let [bx, by, ..] = bb[..] else { return Err(bad(format!("no tab bounding box: {bb:?}"))) };
    sm.send(&format!("MOUSEMOVE {x} {y}"))?;
    sm.send(&format!("CLICK {x} {y} ctrl"))?;
    let ctrl_out = sm.settle("ctrl_out")?;
    check_query(
        &ctrl_out,
        "ctrl_out",
        "a Ctrl-click on a selected tab takes it out of the selection without opening it",
        "s['selected'] == [] and not s['tab_view']",
        |s| selected(s).is_some_and(|sel| sel.is_empty()) && falsy_key(s, "tab_view"),
    );
    sm.send(&format!("CLICK {x} {y} ctrl"))?;
    let ctrl_in = sm.settle("ctrl_in")?;
    check_query(
        &ctrl_in,
        "ctrl_in",
        "a Ctrl-click on an unselected tab selects it without opening it",
        "s['selected'] == tabs and not s['tab_view']",
        |s| selected(s).is_some_and(|sel| sel == s.tabs()) && falsy_key(s, "tab_view"),
    );
    sm.send(&format!("MOUSEMOVE {bx} {by}"))?;
    sm.send(&format!("CLICK {bx} {by}"))?;
    let deselected = sm.settle("deselected")?;
    check_query(
        &deselected,
        "deselected",
        "a click on empty canvas clears the selection",
        "s['selected'] == []",
        |s| selected(s).is_some_and(|sel| sel.is_empty()),
    );

    m.mark("borders");
    // Border colors, read off a screenshot: a group of more than 8 members rests on DANGER_COLOR, and the empty
    // group from above on ACCENT (both read from EXPORTSTATE's `theme`). The mouse waits in an empty corner.
    sm.send("CLICK 22 17")?;
    sm.write("nine.group.json", nine_group().to_string().as_bytes())?;
    let top = sm.settle("top")?;
    let before_nine = top.groups();
    sm.send(&format!("IMPORTGROUP {}", sm.win("nine.group.json")))?;
    sm.send("WAITTABS")?;
    sm.send("CLICK 326 17")?;
    sm.send("MOUSEMOVE 806 629")?;
    let nine_s = sm.settle("nine")?;
    let nine = new_group(&nine_s, &before_nine).unwrap_or_default();
    let empty = before.first().cloned().unwrap_or_default();
    sm.shot("borders.png", "group borders")?;
    let png = sm.path("borders.png");
    let color = |name: &str| {
        nine_s.theme_color(name).unwrap_or_else(|| {
            failed(&format!("EXPORTSTATE has no theme color {name}"));
            [0, 0, 0]
        })
    };
    let (danger, accent) = (color("danger"), color("accent"));
    if border_near(&nine_s, &png, &nine, danger) {
        pass("a group of 9 has a DANGER_COLOR border");
    } else {
        failed(&format!("the 9-member group {nine}'s border isn't DANGER_COLOR (see borders.png)"));
    }
    if border_near(&nine_s, &png, &empty, accent) && !border_near(&nine_s, &png, &empty, danger) {
        pass("an empty group has an ACCENT border");
    } else {
        failed(&format!("the empty group {empty}'s border isn't ACCENT (see borders.png)"));
    }

    // A confirm dialog's main button is filled by what it does: DANGER_COLOR when destructive, ACCENT when not.
    for (kind, name, want) in [("destructive", "danger", danger), ("safe", "accent", accent)] {
        sm.send(&format!("CONFIRM {kind}"))?;
        let open = sm.settle(&format!("confirm_{kind}"))?;
        let file = format!("confirm_{kind}.png");
        sm.shot(&file, &format!("{kind} confirm"))?;
        if fill_pixels(&open, &sm.path(&file), want) >= 200 {
            pass(&format!("a {kind} confirm fills its button with {name}"));
        } else {
            failed(&format!("a {kind} confirm has no {name} button fill (see {file})"));
        }
        sm.send("ESCAPE")?;
        // The dialog is modal until the frame after the key: wait it out so the next click is not swallowed.
        sm.send("WAITIDLE")?;
    }

    m.mark("reopen-nested");
    // Recently Closed keeps nesting: fill the empty group with an image and the Pair group, close it from the top
    // level, and REOPENTAB brings the same group back with its whole subtree, still nested.
    sm.send(&format!("DOUBLECLICK {}", xy(&nine_s, &empty)?))?;
    sm.settle("outer_in")?;
    sm.send(&format!("ADDFILE {}", sm.repo.join(r"crates\manifold\assets\icon\icon-64.png").display()))?;
    sm.send(&format!("IMPORTGROUP {}", sm.win("pair.group.json")))?;
    sm.send("WAITTABS")?;
    let built = sm.settle("outer_built")?;
    check_query(
        &built,
        "outer_built",
        "a group can hold an image and a nested group",
        &format!("s['current_group'] == '{empty}' and len(groups) == 1 and sum(card_is(c, IMAGE) for c in cards.values()) == 1"),
        |s| in_group(s, &empty) && s.groups().len() == 1 && count_of(s, IMAGE) == 1,
    );
    sm.send("CLICK 22 17")?;
    sm.settle("outer_back")?;
    sm.send(&format!("CLOSENODE {empty}"))?;
    let outer_closed = sm.settle("outer_closed")?;
    check_query(
        &outer_closed,
        "outer_closed",
        "closing the outer group removes it",
        &format!("'{empty}' not in groups"),
        |s| !s.groups().contains(&empty),
    );
    sm.send("REOPENTAB")?;
    sm.send("WAITTABS")?;
    let reopened = sm.settle("reopened")?;
    check_query(&reopened, "reopened", "REOPENTAB brings the same group back", &format!("'{empty}' in groups"), |s| {
        s.groups().contains(&empty)
    });
    let outer = empty.clone();
    sm.send(&format!("DOUBLECLICK {}", xy(&reopened, &outer)?))?;
    let reopened_in = sm.settle("reopened_in")?;
    check_query(
        &reopened_in,
        "reopened_in",
        "the reopened group has its image and nested group back",
        &format!(
            "s['current_group'] == '{outer}' and len(groups) == 1 and sum(card_is(c, IMAGE) for c in cards.values()) == 1"
        ),
        |s| in_group(s, &outer) && s.groups().len() == 1 && count_of(s, IMAGE) == 1,
    );
    let inner = first(&reopened_in.groups(), "nested group")?.clone();
    sm.send(&format!("DOUBLECLICK {}", xy(&reopened_in, &inner)?))?;
    let reopened_inner = sm.settle("reopened_inner")?;
    check_query(
        &reopened_inner,
        "reopened_inner",
        "the nested group comes back with both tabs",
        &format!("s['current_group'] == '{inner}' and len(tabs) == 2"),
        |s| in_group(s, &inner) && s.tabs().len() == 2,
    );

    m.mark("file-drops");
    // Drag and drop: `ADDFILE <path> X Y` places a file the way a real drop does, in Graph view here.
    sm.write("drop.txt", b"a dropped note\n")?;
    sm.write("drop.json", b"{\"a\": 1}\n")?;
    let before_drop = sorted_ids(&sm.state("before_drop")?);
    sm.send(&format!("ADDFILE {} 400 300", sm.win("drop.txt")))?;
    sm.send(&format!("ADDFILE {} 400 300", sm.win("drop.json")))?;
    let after_drop = sm.settle("after_drop")?;
    let note_id = new_card(&after_drop, NOTE, None, &before_drop);
    let json_id = new_card(&after_drop, JSON, None, &before_drop);
    check_found(&after_drop, "after_drop", "a dropped .txt becomes a Note card", &note_id);
    check_found(&after_drop, "after_drop", "a dropped .json becomes a Json card", &json_id);
    // Both land on one point, so the layout eases them about a card apart: each moves half a card or so, not
    // off-canvas.
    check_query(
        &after_drop,
        "after_drop",
        "the dropped note's screen center is within 200px of (400, 300)",
        &format!(
            "abs(cards['{note_id}']['screen'][0] - 400) < 200 and abs(cards['{note_id}']['screen'][1] - 300) < 200"
        ),
        |s| within_200(s, &note_id, 400.0, 300.0),
    );
    check_query(
        &after_drop,
        "after_drop",
        "the dropped json's screen center is within 200px of (400, 300)",
        &format!(
            "abs(cards['{json_id}']['screen'][0] - 400) < 200 and abs(cards['{json_id}']['screen'][1] - 300) < 200"
        ),
        |s| within_200(s, &json_id, 400.0, 300.0),
    );
    check_query(
        &after_drop,
        "after_drop",
        "the two cards dropped on one point no longer overlap",
        &format!(
            "(lambda a, b: a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1])(cards['{note_id}']['rect'], cards['{json_id}']['rect'])"
        ),
        |s| apart(s, &note_id, &json_id),
    );

    m.mark("json-level");
    // Entering the dropped JSON file: its level, driven by the JSON commands, then back out.
    sm.send(&format!("DOUBLECLICK {}", xy(&after_drop, &json_id)?))?;
    let json_in = sm.settle("json_in")?;
    check_query(
        &json_in,
        "json_in",
        "double-clicking a Json card enters its level",
        &format!("s['json_level'] and s['json_level']['root'] == '{json_id}' and s['current_group'] == '{json_id}'"),
        |s| truthy_key(s, "json_level") && json_level(s)["root"] == json_id.as_str() && in_group(s, &json_id),
    );
    check_query(
        &json_in,
        "json_in",
        "the JSON level shows the root card",
        "[c['pointer'] for c in s['json_level']['cards']] == [''] and [c for c in cards if c['json_card']]",
        |s| {
            let pointers: Option<Vec<&str>> =
                json_level(s)["cards"].as_array().and_then(|a| a.iter().map(|c| c["pointer"].as_str()).collect());
            pointers == Some(vec![""]) && s.all_cards().iter().any(|c| truthy(&c["json_card"]))
        },
    );
    sm.send("JSONSELECT /a")?;
    sm.send("JSONSEARCH a")?;
    sm.check_state(
        "json_search",
        "JSONSELECT and JSONSEARCH reach the level",
        "s['json_level']['selected'] == '/a' and s['json_level']['matches'] == ['/a']",
        |v| json_level(v.s)["selected"] == "/a" && json_level(v.s)["matches"] == serde_json::json!(["/a"]),
    )?;
    sm.send("CLICK 22 17")?;
    sm.check_state(
        "json_out",
        "the back button leaves the JSON level",
        &format!("s['json_level'] is None and s['current_group'] != '{json_id}'"),
        |v| null_key(v.s, "json_level") && !in_group(v.s, &json_id),
    )?;

    // Creating from a JSON level leaves it first: a new note lands in the level the user then sees.
    let json_out2 = sm.settle("json_out2")?;
    sm.send(&format!("DOUBLECLICK {}", xy(&json_out2, &json_id)?))?;
    let json_in2 = sm.settle("json_in2")?;
    check_query(
        &json_in2,
        "json_in2",
        "re-entering the JSON level",
        &format!("s['json_level'] and s['current_group'] == '{json_id}'"),
        |s| truthy_key(s, "json_level") && in_group(s, &json_id),
    );
    let notes_before = count_of(&json_out2, NOTE);
    sm.send("NEWNOTE")?;
    let json_note = sm.settle("json_note")?;
    check_query(
        &json_note,
        "json_note",
        "NEWNOTE from a JSON level leaves it",
        &format!("s['json_level'] is None and s['current_group'] != '{json_id}'"),
        |s| null_key(s, "json_level") && !in_group(s, &json_id),
    );
    check_query(
        &json_note,
        "json_note",
        "the new note is a card on the level now shown",
        "len([i for i, c in cards.items() if card_is(c, NOTE)]) == len(eval('''$notes_before''')) + 1",
        |s| count_of(s, NOTE) == notes_before + 1,
    );

    // Rename and Duplicate work on any node: the new note takes a name, and its copy keeps it.
    let note_id = new_card(&json_note, NOTE, None, &sorted_ids(&json_out2));
    sm.send(&format!("RENAMEGROUP {note_id} Groceries"))?;
    let renamed = sm.settle("note_renamed")?;
    check_query(
        &renamed,
        "note_renamed",
        "RENAMEGROUP names a note",
        &format!("cards['{note_id}']['name'] == 'Groceries'"),
        |s| s.card(&note_id).is_some_and(|c| c["name"] == "Groceries"),
    );
    sm.send(&format!("DUPLICATENODE {note_id}"))?;
    let duplicated = sm.settle("note_duplicated")?;
    let copy_id = new_card(&duplicated, NOTE, None, &sorted_ids(&renamed));
    check_found(&duplicated, "note_duplicated", "DUPLICATENODE adds a second note", &copy_id);
    check_query(
        &duplicated,
        "note_duplicated",
        "the copy keeps the name and the original stays",
        &format!("cards['{copy_id}']['name'] == 'Groceries' and '{note_id}' in cards"),
        |s| s.card(&copy_id).is_some_and(|c| c["name"] == "Groceries") && s.card(&note_id).is_some(),
    );

    // Jars are cards: make one, link a tab to it, rename and duplicate it, then close and reopen a group holding
    // all of them: the same group, tab and jars come back, and the tab's edge still names its jar.
    let before = sorted_ids(&duplicated);
    sm.send("NEWJAR Personal")?;
    let made = sm.settle("jar_made")?;
    let jar_id = new_card(&made, JAR, None, &before);
    check_found(&made, "jar_made", "NEWJAR adds a jar card", &jar_id);
    let tab_id = made
        .all_cards()
        .iter()
        .find(|c| State::card_is(c, TAB, None))
        .and_then(|c| c["id"].as_str())
        .map(str::to_string);
    let Some(tab_id) = tab_id else { return Err(io::Error::other("no tab card on the level for the jar section")) };
    sm.send(&format!("USEJAR {tab_id} {jar_id}"))?;
    let used = sm.settle("jar_used")?;
    check_query(
        &used,
        "jar_used",
        "USEJAR draws the tab's jar edge",
        &format!("cards['{tab_id}']['jar'] == '{jar_id}' and cards['{tab_id}']['effective_jar'] == '{jar_id}'"),
        |s| s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str() && c["effective_jar"] == jar_id.as_str()),
    );
    sm.send(&format!("RENAMEGROUP {jar_id} Work"))?;
    let renamed_jar = sm.settle("jar_renamed")?;
    check_query(
        &renamed_jar,
        "jar_renamed",
        "a jar card renames like any card",
        &format!("cards['{jar_id}']['name'] == 'Work'"),
        |s| s.card(&jar_id).is_some_and(|c| c["name"] == "Work"),
    );
    let before = sorted_ids(&renamed_jar);
    sm.send(&format!("DUPLICATENODE {jar_id}"))?;
    let dup_jar = sm.settle("jar_duplicated")?;
    let copy_jar = new_card(&dup_jar, JAR, None, &before);
    check_query(
        &dup_jar,
        "jar_duplicated",
        "the jar copy keeps the name; the tab keeps the original",
        &format!("cards['{copy_jar}']['name'] == 'Work' and cards['{tab_id}']['jar'] == '{jar_id}'"),
        |s| {
            s.card(&copy_jar).is_some_and(|c| c["name"] == "Work")
                && s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str())
        },
    );
    let groups_before = dup_jar.groups();
    sm.send("NEWGROUP")?;
    let grouped = sm.settle("jar_group_made")?;
    let group_id = new_group(&grouped, &groups_before).unwrap_or_else(|| "none".into());
    check_found(&grouped, "jar_group_made", "NEWGROUP adds a group for the jar section", &group_id);
    for id in [&tab_id, &jar_id, &copy_jar] {
        sm.send(&format!("MOVETOGROUP {id} {group_id}"))?;
    }
    let moved = sm.settle("jar_moved")?;
    check_query(
        &moved,
        "jar_moved",
        "a tab and its jars move into the group together",
        &format!("'{tab_id}' not in cards and '{jar_id}' not in cards and '{copy_jar}' not in cards"),
        |s| s.card(&tab_id).is_none() && s.card(&jar_id).is_none() && s.card(&copy_jar).is_none(),
    );
    sm.send(&format!("CLOSENODE {group_id}"))?;
    let closed = sm.settle("jar_group_closed")?;
    check_query(
        &closed,
        "jar_group_closed",
        "closing the group removes it and its jars from the level",
        &format!("'{group_id}' not in groups"),
        |s| !s.groups().contains(&group_id),
    );
    sm.send("REOPENTAB")?;
    sm.send("WAITTABS")?;
    let reopened = sm.settle("jar_group_reopened")?;
    check_found(&reopened, "jar_group_reopened", "REOPENTAB brings the jar group back", &group_id);
    let outer = group_id.clone();
    // The reopened group keeps the spot it had when closed, which can sit under the toolbar (seen once at y = 24,
    // where the double-click missed it): drag it to the world origin, the middle of the canvas, first.
    sm.send(&format!("MOVENODE {outer} 0 0"))?;
    let placed = sm.settle("jar_group_placed")?;
    sm.send(&format!("DOUBLECLICK {}", xy(&placed, &outer)?))?;
    let inside = sm.settle("jar_group_in")?;
    check_query(
        &inside,
        "jar_group_in",
        "the reopened tab is the same card and its edge names the same jar",
        &format!("cards['{tab_id}']['jar'] == '{jar_id}' and len(jars) == 2"),
        |s| s.card(&tab_id).is_some_and(|c| c["jar"] == jar_id.as_str()) && count_of(s, JAR) == 2,
    );
    sm.send("CLICK 22 17")?;
    sm.settle("jar_group_out")?;
    sm.send(&format!("CLOSENODE {outer}"))?;
    sm.settle("jar_group_gone")?;

    // A far reveal moves a card's window (skip > 0, a `+ N earlier` row under the header); a real CLICK on that row
    // grows the window up by one page.
    let big = (0..1000).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    sm.write("big.json", format!("{{\"items\": [{big}]}}\n").as_bytes())?;
    let before_big = sorted_ids(&sm.state("before_big")?);
    sm.send(&format!("ADDFILE {} 400 300", sm.win("big.json")))?;
    let after_big = sm.settle("after_big")?;
    let big_id = new_card(&after_big, JSON, None, &before_big);
    sm.send(&format!("DOUBLECLICK {}", xy(&after_big, &big_id)?))?;
    sm.settle("big_in")?;
    sm.send("JSONSELECT /items/900")?;
    sm.send("WAITIDLE")?;
    let big_far = sm.settle("big_far")?;
    check_query(
        &big_far,
        "big_far",
        "a far JSONSELECT moves the card's window (skip 900) and gives the earlier row a click point",
        &format!("s['json_level']['root'] == '{big_id}' and ITEMS['skip'] == 900 and ITEMS['earlier'] is not None"),
        |s| {
            json_level(s)["root"] == big_id.as_str()
                && skip_is(s, 900.0)
                && items_card(s).is_some_and(|c| c.get("earlier").is_some_and(|e| !e.is_null()))
        },
    );
    let earlier = earlier_point(&big_far).ok_or_else(|| bad("no earlier row to click".into()))?;
    sm.send(&format!("CLICK {earlier}"))?;
    sm.send("WAITIDLE")?;
    let big_earlier = sm.settle("big_earlier")?;
    check_query(
        &big_earlier,
        "big_earlier",
        "clicking the + N earlier row grows the window up by one page (skip 880)",
        "ITEMS['skip'] == 880",
        |s| skip_is(s, 880.0),
    );

    // JSONMORE reveals one more page (20 rows) of the card; JSONTOGGLE collapses it (no rows) and expands it again;
    // JSONRELAYOUT keeps the cards.
    let (rows0, more0, _) = items_rows_more(&big_earlier).unwrap_or_default();
    sm.send("JSONMORE /items")?;
    sm.send("WAITIDLE")?;
    let big_more = sm.settle("big_more")?;
    check_query(
        &big_more,
        "big_more",
        "JSONMORE /items reveals one more page (20 rows)",
        "ITEMS['rows'] == rows0 + 20 and ITEMS['more'] == more0 - 20",
        |s| items_rows_more(s).is_some_and(|(r, m, _)| r == rows0 + 20.0 && m == more0 - 20.0),
    );
    sm.send("JSONTOGGLE /items")?;
    sm.send("WAITIDLE")?;
    let big_folded = sm.settle("big_folded")?;
    check_query(
        &big_folded,
        "big_folded",
        "JSONTOGGLE /items collapses the card (no rows)",
        "ITEMS['collapsed'] and ITEMS['rows'] == 0",
        |s| items_rows_more(s).is_some_and(|(r, _, c)| c && r == 0.0),
    );
    sm.send("JSONTOGGLE /items")?;
    sm.send("JSONRELAYOUT")?;
    sm.send("WAITIDLE")?;
    let big_unfolded = sm.settle("big_unfolded")?;
    check_query(
        &big_unfolded,
        "big_unfolded",
        "a second JSONTOGGLE expands it again with its rows, and JSONRELAYOUT keeps the level",
        "not ITEMS['collapsed'] and ITEMS['rows'] == rows0 + 20 and s['json_level']['root'] == big_id",
        |s| {
            items_rows_more(s).is_some_and(|(r, _, c)| !c && r == rows0 + 20.0)
                && json_level(s)["root"] == big_id.as_str()
        },
    );
    sm.send("CLICK 22 17")?;
    sm.check_state("big_out", "the back button leaves the big JSON level", "s['json_level'] is None", |v| {
        null_key(v.s, "json_level")
    })?;

    m.mark("content-kinds");
    // The remaining `ADDFILE` kinds: a PDF, a `.bin` (a Download card) and a `.md` (a Note by its bytes, not its
    // extension). `.html` becomes a live Tab, which is double-clicked into Tab view to check its `file://` url.
    sm.write("doc.pdf", b"%PDF-1.4\n")?;
    sm.write("note.md", b"# a markdown note\n")?;
    sm.write("data.bin", b"\xffbinary\xfedata")?;
    sm.write("page.html", b"<!doctype html><html><body>hi</body></html>")?;
    let before_kinds = sorted_ids(&sm.state("before_kinds")?);
    for f in ["doc.pdf", "note.md", "data.bin", "page.html"] {
        sm.send(&format!("ADDFILE {} 600 500", sm.win(f)))?;
    }
    sm.send("WAITTABS")?;
    let after_kinds = sm.settle("after_kinds")?;
    let pdf_id = new_card(&after_kinds, PDF, None, &before_kinds);
    let md_id = new_card(&after_kinds, NOTE, None, &before_kinds);
    let bin_id = new_card(&after_kinds, DOWNLOAD, None, &before_kinds);
    let html_id = new_card(&after_kinds, TAB, Some(true), &before_kinds);
    check_found(&after_kinds, "after_kinds", "a dropped .pdf becomes a Pdf card", &pdf_id);
    check_found(&after_kinds, "after_kinds", "a dropped .md becomes a Note card", &md_id);
    check_found(&after_kinds, "after_kinds", "a dropped .bin becomes a Download card", &bin_id);
    check_found(&after_kinds, "after_kinds", "a dropped .html becomes a Tab card", &html_id);
    if html_id != "none" {
        sm.send(&format!("DOUBLECLICK {}", xy(&after_kinds, &html_id)?))?;
        sm.wait_state(
            "html_tab",
            "the dropped .html tab's url is a file:// URL",
            r#"url.startswith("file://")"#,
            |v| v.url.starts_with("file://"),
        )?;
    }

    m.mark("conversion");
    // Engine-side conversion, no network: a tab typed to a local JSON or PDF file goes, and a node of that kind
    // takes its place, keeping the file:// url. Checked on EXPORTSESSION, which carries each node's url.
    sm.write("conv.json", b"{\"converted\": [1, 2]}\n")?;
    sm.write("conv.pdf", b"%PDF-1.4\n")?;
    convert_focused_tab(sm, "conv.json", "json")?;
    convert_focused_tab(sm, "conv.pdf", "pdf")?;

    match sm.inst.quit(QUIT_TIMEOUT) {
        Ok(status) if status.success() => pass("process exited cleanly"),
        Ok(status) => failed(&format!("process did not exit cleanly (exit code {})", status.code().unwrap_or(-1))),
        Err(e) => failed(&format!("process did not exit cleanly ({e})")),
    }
    Ok(())
}

/// `convert_focused_tab FILE KIND`. NEWTAB doesn't focus the tab it opens, and a conversion drops to Graph view with
/// nothing focused, so this types into whichever tab NEXTTAB lands on.
fn convert_focused_tab(sm: &mut Smoke, file: &str, kind: &str) -> io::Result<()> {
    let name = format!("conv_{kind}");
    let url = file_url(&sm.path(file));
    sm.send("WAITTABS")?;
    if !truthy_key(&sm.state(&format!("focus_{kind}"))?, "tab_view") {
        sm.send("NEXTTAB")?;
    }
    // Let the tab's own load settle, or its address-bar update overwrites the typed url before ENTER.
    sm.send("WAITIDLE")?;
    let s = sm.state(&format!("focus_{kind}"))?;
    if !(truthy_key(&s, "tab_view") && !s.focused_url().is_empty()) {
        failed(&format!("no tab focused to type {file} into (see focus_{kind}.json)"));
        return Ok(());
    }
    sm.send(&format!("ADDRESSBAR {url}"))?;
    sm.send("WAITIDLE")?;
    sm.send("ENTER")?;
    for _ in 0..150 {
        let path = sm.export("EXPORTSESSION", &format!("{name}.json"))?;
        let session: Value = serde_json::from_slice(&fs::read(path)?).map_err(|e| bad(format!("{name}.json: {e}")))?;
        if converted(&session, file, kind) {
            pass(&format!("a tab at a local {file} becomes a {kind} node with its file:// url"));
            return Ok(());
        }
        sleep(SETTLE_POLL);
    }
    failed(&format!("a tab at a local {file} never became a {kind} node (see {name}.json)"));
    Ok(())
}

/// The checkout this exe was built in: the nearest ancestor holding the icon the reopen test drops.
fn repo_root(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|d| d.join(r"crates\manifold\assets\icon\icon-64.png").is_file()).map(Path::to_path_buf)
}

pub fn smoke(parent: Option<PathBuf>) -> ExitCode {
    let mut marks = Marks::start("setup");
    let fatal = |e: String| {
        eprintln!("{e}");
        ExitCode::from(1)
    };
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => return fatal(e),
    };
    let Some(repo) = repo_root(&exe) else { return fatal(format!("no checkout found above {}", exe.display())) };
    let out =
        std::env::var_os("HEADLESS_TEST_OUT").map_or_else(|| repo.join(r"target\headless-test-output"), PathBuf::from);
    let setup = fs::create_dir_all(&out).and_then(|()| {
        for e in fs::read_dir(&out)?.flatten() {
            if e.path().extension().is_some_and(|x| x == "png" || x == "json") {
                fs::remove_file(e.path())?;
            }
        }
        Ok(())
    });
    if let Err(e) = setup {
        return fatal(format!("{}: {e}", out.display()));
    }
    // A fresh profile of its own, so the real %APPDATA%\manifold is never read or written. `has_shown_first_launch_demo`
    // skips the demo scene, so the app starts with just the root tab.
    let work = match work_dir(parent, "smoke") {
        Ok(w) => w,
        Err(e) => return fatal(format!("work dir: {e}")),
    };
    let profile = work.join("profile");
    // Address-bar suggestions come from a local server, not DuckDuckGo, so those checks don't depend on the network.
    let started = fs::create_dir_all(&profile)
        .and_then(|()| fs::write(profile.join("settings.json"), "{\"has_shown_first_launch_demo\": true}\n"))
        .and_then(|()| Server::start())
        .and_then(|server| {
            let url = server.url();
            let inst = Instance::launch_env(&exe, &profile, &out, "smoke.log", &[("MANIFOLD_SUGGESTIONS_URL", &url)])?;
            Ok((server, inst))
        });
    let (server, inst) = match started {
        Ok(x) => x,
        Err(e) => return fatal(format!("launch: {e}")),
    };
    let log = inst.log.clone();
    let mut sm = Smoke { inst, out: out.clone(), repo };
    if let Err(e) = drive(&mut sm, &mut marks) {
        // Not a verdict line: gate all counts those words, and this failure is counted here.
        println!("ERROR: smoke stopped early: {e}");
        note_failure();
    }
    marks.end();
    drop(sm); // kills the instance if it is still up
    drop(server);
    if failures() > 0 {
        println!("--- tail of {}\n{}", log.display(), no_verdict_words(&log_tail(&log, 30)));
    }
    println!();
    if failures() == 0 {
        println!("All checks passed. Screenshots in {}", out.display());
    } else {
        println!("Some checks failed. Screenshots in {}", out.display());
    }
    if std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(&work);
    }
    exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(json: &str) -> State {
        State::parse(json).unwrap()
    }

    #[test]
    fn view_reads_the_search_query_out_of_the_url() {
        let s = state(r#"{"focused_url":"https://duckduckgo.com/?q=sourdough+bread+two&ia=web"}"#);
        let v = View::new(&s);
        assert_eq!(v.q, "sourdough bread two");
        let s = state(r#"{"focused_url":null}"#);
        assert_eq!((View::new(&s).url, View::new(&s).q.as_str()), ("", ""));
    }

    #[test]
    fn settle_key_ignores_card_order_and_changes_with_a_tenth_of_a_pixel() {
        let a = state(r#"{"cards":[{"id":"b","rect":[1.0,2.0,3.0,4.04]},{"id":"a","rect":[0.0,0.0,5.0,5.0]}]}"#);
        let same = state(r#"{"cards":[{"id":"a","rect":[0.0,0.0,5.0,5.0]},{"id":"b","rect":[1.0,2.0,3.0,4.0]}]}"#);
        let moved = state(r#"{"cards":[{"id":"a","rect":[0.0,0.0,5.0,5.0]},{"id":"b","rect":[1.0,2.0,3.0,4.1]}]}"#);
        assert_eq!(a.rect_key(), same.rect_key()); // 4.04 rounds to 4.0
        assert_ne!(a.rect_key(), moved.rect_key());
    }

    /// Values python's `round(v, 1)` rounds as `0.2 0.1 0.3 0.5 -0.0 1.0` (recorded from python 3.14:
    /// `[round(v, 1) for v in (0.25, 0.15, 0.35, 0.45, -0.04, 0.95)]` is `[0.2, 0.1, 0.3, 0.5, -0.0, 0.9]`): the key
    /// must agree wherever python's repr does.
    #[test]
    fn rect_key_rounds_like_python() {
        for (v, want) in [(0.25, "0.2"), (0.15, "0.1"), (0.35, "0.3"), (0.45, "0.5"), (-0.04, "-0.0"), (0.95, "0.9")] {
            let s = state(&format!(r#"{{"cards":[{{"id":"a","rect":[{v:?},0.0,0.0,0.0]}}]}}"#));
            assert_eq!(s.rect_key(), format!("a:{want},0.0,0.0,0.0"), "round({v}, 1)");
        }
    }

    #[test]
    fn queries_match_the_python_expressions() {
        let s = state(
            r#"{"tab_view":false,"current_group":"g","selected":["t1"],"suggestions":[],"toast":null,
                "highlighted":1,"json_level":null,
                "cards":[{"id":"t1","type":"application/vnd.manifold.tab","loaded":false,"json_card":false,
                           "parent":null,"screen":[400.0,300.0],"rect":[0.0,0.0,10.0,10.0]},
                          {"id":"t2","type":"application/vnd.manifold.tab","loaded":true,"json_card":false,
                           "parent":"t1","screen":[700.0,300.0],"rect":[20.0,0.0,30.0,10.0]},
                          {"id":"n","type":"text/markdown","loaded":false,"json_card":false,
                           "screen":[100.0,100.0],"rect":[5.0,5.0,15.0,15.0]}]}"#,
        );
        assert!(falsy_key(&s, "tab_view") && falsy_key(&s, "suggestions") && !truthy_key(&s, "json_level"));
        assert!(null_key(&s, "toast") && null_key(&s, "json_level") && !null_key(&s, "missing"));
        assert!(!falsy_key(&s, "missing") && !truthy_key(&s, "missing"));
        assert_eq!(selected(&s), Some(vec!["t1".to_string()]));
        assert!(within_200(&s, "t1", 400.0, 300.0) && !within_200(&s, "t2", 400.0, 300.0));
        assert!(!within_200(&s, "nope", 400.0, 300.0));
        assert!(apart(&s, "t1", "t2") && !apart(&s, "t1", "n"));
        assert_eq!(new_card(&s, NOTE, None, &[]), "n");
        assert_eq!(new_card(&s, NOTE, None, &["n".to_string()]), "none");
        assert_eq!(new_card(&s, TAB, Some(true), &[]), "t2");
        assert_eq!(sorted_ids(&s), ["n", "t1", "t2"]);
        assert_eq!(count_of(&s, TAB), 2);
        assert_eq!(saw(&s), "tab_view=False group=g tabs=['t1', 't2'] groups=[]");
    }

    #[test]
    fn the_json_level_helpers() {
        let s = state(
            r#"{"json_level":{"root":"j","cards":[{"pointer":"","skip":0},
                {"pointer":"/items","skip":900.0,"earlier":[12.4,99.5]}]}}"#,
        );
        assert!(skip_is(&s, 900.0) && !skip_is(&s, 880.0));
        assert_eq!(earlier_point(&s).as_deref(), Some("12 100")); // 99.5 ties to even
        assert_eq!(items_card(&state(r#"{"json_level":null}"#)), None);
    }

    #[test]
    fn conversion_needs_the_node_and_no_tab() {
        let node = |ty: &str, url: &str| serde_json::json!({"id": "n", "type": ty, "url": url});
        let doc = |nodes: Vec<Value>| serde_json::json!({ "nodes": nodes });
        let json = node(JSON, "file:///C:/x/conv.json");
        assert!(converted(&doc(vec![json.clone()]), "conv.json", "json"));
        assert!(!converted(&doc(vec![json.clone()]), "conv.json", "pdf"));
        assert!(!converted(&doc(vec![json.clone()]), "other.json", "json"));
        // A tab still at the file means the conversion has not happened.
        assert!(!converted(&doc(vec![json, node(TAB, "file:///C:/x/conv.json")]), "conv.json", "json"));
        assert!(!converted(&doc(vec![node(JSON, "https://x/conv.json")]), "conv.json", "json"));
        assert!(!converted(&doc(vec![]), "conv.json", "json"));
    }

    #[test]
    fn border_pixels_near_the_card_edge() {
        let dir = std::env::temp_dir().join(format!("gate-border-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let png = dir.join("b.png");
        let mut img = image::RgbImage::from_pixel(50, 50, image::Rgb([10, 10, 10]));
        img.put_pixel(20, 25, image::Rgb([200, 90, 60])); // 1 px stroke at the left edge, mid-height
        img.save(&png).unwrap();
        let s =
            state(r#"{"cards":[{"id":"a","rect":[21.0,10.0,40.0,40.0]},{"id":"far","rect":[45.0,10.0,60.0,40.0]}]}"#);
        // Pixels x 20..=23 at y 25 are checked: 20 is the stroke, within 40 of (196, 87, 58).
        assert!(border_near(&s, &png, "a", [196, 87, 58]));
        assert!(!border_near(&s, &png, "a", [215, 162, 74]));
        assert!(!border_near(&s, &png, "nope", [196, 87, 58]));
        // A card wholly past the image has no pixel to sample and fails; one partly past it is judged by the pixels
        // that are there.
        assert!(!border_near(
            &state(r#"{"cards":[{"id":"o","rect":[60.0,10.0,70.0,40.0]}]}"#),
            &png,
            "o",
            [10, 10, 10]
        ));
        assert!(border_near(&s, &png, "far", [10, 10, 10]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn xy_pairs() {
        assert_eq!(pair("407 332").unwrap(), (407, 332));
        assert!(pair("407").is_err() && pair("a b").is_err());
        assert_eq!(py_list(&["a".into(), "b".into()]), "['a', 'b']");
        assert_eq!(py_list(&[]), "[]");
    }

    #[test]
    fn nine_tabs_and_a_group() {
        let v = nine_group();
        let nodes = v["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 10);
        assert_eq!(nodes[0]["name"], "Nine");
        assert!(nodes[1..].iter().all(|n| n["group"] == "g" && n["type"] == TAB));
    }

    #[test]
    fn the_pair_group_is_valid_json() {
        let v: Value = serde_json::from_str(PAIR_GROUP).unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), 3);
    }
}
