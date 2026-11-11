//! `gate loads`: a url, Reload, Back or Forward sent to a tab is never silently dropped, nor overridden by a load still
//! pending in the page. `CASES` picks cases (default all), `VIEWS` the view kinds, as every probe stage.
//!
//! Each case runs one instance with its own TEMP, so the host's per-tab log it reads is its own. On Servo the host logs
//! each decision (`[views] load in place`, `swap`, `history in place`, `history swap`, and `promoted` when a pending
//! view is shown), and a case checks the exact list. The app sets a tab's url when a load is typed, so "ends on" reads
//! the trail; the server's hits and, on Servo, `settled` are what show a load landed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::check::{check, check_is, Section};
use crate::instance::{poll, register_log, Instance};
use crate::probes::{guarded, host_logs, quit, seed, tab_node, views, Site};
use crate::shots_img;
use crate::state::State;
use crate::windows_main::Ctx;

const CASES: &[&str] = &[
    "dead204",
    "double",
    "body-swap",
    "live204",
    "reload",
    "reload-early",
    "paint",
    "iframe",
    "back-type",
    "forward",
    "back-pending",
    "back-back",
    "double-slow",
    "spa-back",
    "back-page",
    "back-swap",
    "zoom-swap",
];

/// The longest wait for a request, a pending view to be shown, or a screenshot to change.
const WAIT: Duration = Duration::from_secs(20);

/// The host's view decisions, in order, from the per-tab logs in `tmp`.
fn decisions(tmp: &Path) -> Vec<String> {
    views_lines(tmp)
        .into_iter()
        .filter_map(|l| {
            ["load in place", "history in place", "history swap", "swap"]
                .into_iter()
                .find(|d| l.starts_with(d))
                .map(str::to_owned)
        })
        .collect()
}

/// No view is pending: the last of the log's swaps, Backs and promotes, if any, is not a swap (an in-place Back drops
/// a pending view too).
fn settled(tmp: &Path) -> bool {
    let last = views_lines(tmp).into_iter().rfind(|l| !l.starts_with("load in place"));
    !last.is_some_and(|l| l.starts_with("swap") || l.starts_with("history swap"))
}

/// The text after `[views] ` of every such line in the host logs in `tmp`.
fn views_lines(tmp: &Path) -> Vec<String> {
    host_logs(tmp)
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .flat_map(|text| {
            text.lines().filter_map(|l| l.split_once("[views] ").map(|(_, rest)| rest.to_owned())).collect::<Vec<_>>()
        })
        .collect()
}

/// The number of pixels of `path` within 8 of `rgb` on every channel.
fn pixels(path: &Path, rgb: [u8; 3]) -> io::Result<usize> {
    let img = shots_img::load(path)?;
    Ok(img.pixels().filter(|p| p.0.iter().zip(rgb).all(|(a, b)| a.abs_diff(b) < 8)).count())
}

/// The number of pixel columns of `path` holding at least one pixel within 16 of `rgb` on every channel.
fn width_of(path: &Path, rgb: [u8; 3]) -> io::Result<u32> {
    let img = shots_img::load(path)?;
    let near =
        |x: u32| (0..img.height()).any(|y| img.get_pixel(x, y).0.iter().zip(rgb).all(|(a, b)| a.abs_diff(b) <= 16));
    Ok((0..img.width()).filter(|&x| near(x)).count() as u32)
}

/// One case's running instance, its site and its TEMP.
struct Case<'a> {
    name: &'a str,
    view: &'a str,
    site: Site,
    inst: Instance,
    tmp: PathBuf,
    shots: u32,
    work: PathBuf,
}

impl<'a> Case<'a> {
    /// Starts a site and an instance whose one tab first loads `first`, and waits until the site has seen it (no idle
    /// wait: a case may race that load).
    fn start(ctx: &Ctx, name: &'a str, view: &'a str, first: &str) -> io::Result<Case<'a>> {
        let site = Site::start()?;
        let work = ctx.work.join(format!("loads-{name}-{view}"));
        let tmp = work.join("tmp");
        fs::create_dir_all(&tmp)?;
        let profile = work.join("appdata");
        seed(&ctx.exe, &profile, view, &[tab_node(Some(&site.url(first)), view)])?;
        let tmp_s = tmp.display().to_string();
        let mut inst = Instance::launch_env(&ctx.exe, &profile, &work, "log", &[("TEMP", &tmp_s), ("TMP", &tmp_s)])?;
        inst.send("WAITTABS")?;
        inst.send("GRAPHDOWN")?;
        inst.send("GRAPHOPEN")?;
        poll(first, WAIT, || site.hits(first) > 0)?;
        Ok(Case { name, view, site, inst, tmp, shots: 0, work })
    }

    fn label(&self, claim: &str) -> String {
        format!("loads {}: {}: {claim}", self.view, self.name)
    }

    fn hits(&self, path: &str) -> usize {
        self.site.hits(path)
    }

    /// Waits for `path`'s request, an error past `WAIT`.
    fn requested(&self, path: &str) -> io::Result<()> {
        poll(path, WAIT, || self.site.hits(path) > 0)
    }

    /// Whether `path` is requested `n` times within `WAIT`.
    fn reaches(&self, path: &str, n: usize) -> bool {
        poll(path, WAIT, || self.site.hits(path) >= n).is_ok()
    }

    /// Types `path` into the address bar. Not `WAITIDLE` first: it would wait out the pending load a case races. One
    /// frame lets the typed text land.
    fn type_url(&mut self, path: &str) -> io::Result<()> {
        self.inst.send(&format!("ADDRESSBAR {}", self.site.url(path)))?;
        self.inst.send("SLEEP 1")?;
        self.inst.send("ENTER")
    }

    fn send(&mut self, line: &str) -> io::Result<()> {
        self.inst.send(line)?;
        self.inst.barrier()
    }

    /// The app is idle and, on Servo, no view is pending (a pending view is never marked loading, so `WAITIDLE` alone
    /// can return before it is shown); then idle again, for the view just shown.
    fn then(&mut self) -> io::Result<()> {
        self.send("WAITIDLE")?;
        if self.view == "servo" && poll("the pending view", WAIT, || settled(&self.tmp)).is_err() {
            check(&self.label("the swapped-in view is shown"), false);
        }
        self.send("WAITIDLE")
    }

    fn state(&mut self, label: &str) -> io::Result<State> {
        let name = format!("{}-{label}", self.name);
        self.inst.export_state(&name)
    }

    fn until(&mut self, label: &str, want: impl FnMut(&State) -> bool) -> io::Result<State> {
        let name = format!("{}-{label}", self.name);
        self.inst.export_until(&name, want)
    }

    fn at(&self, path: &str) -> String {
        self.site.url(path)
    }

    fn shot(&mut self, label: &str) -> io::Result<PathBuf> {
        self.shots += 1;
        let path = self.work.join(format!("{}-{label}.png", self.shots));
        self.inst.screenshot_to(&path)?;
        Ok(path)
    }

    /// Quits, then on Servo checks the host's decisions against `want`.
    fn finish(mut self, want: &[&str]) {
        quit(&mut self.inst, self.name);
        let logs = host_logs(&self.tmp);
        logs.iter().for_each(|p| register_log(p));
        if self.view == "servo" {
            check_is(
                &self.label("the view decisions"),
                want.to_vec(),
                decisions(&self.tmp).iter().map(String::as_str).collect(),
            );
        }
    }
}

fn run_case(ctx: &Ctx, name: &str, view: &str) -> io::Result<()> {
    match name {
        "dead204" => {
            let mut c = Case::start(ctx, name, view, "/k")?;
            c.send("WAITIDLE")?;
            // Well after the 204 abort has been handled.
            c.send("SLEEP 3")?;
            c.type_url("/p2")?;
            let reached = c.reaches("/p2", 1);
            c.then()?;
            check(&c.label("a url typed after a 204 first load reaches the server"), reached);
            c.finish(&["swap"]);
        }
        "double" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            c.type_url("/p2")?;
            let reached = c.reaches("/p2", 1);
            c.then()?;
            // Past /slow's headers and any trail revert the app makes on a dropped move.
            let end = c.until("end", |s| s.focused_url().ends_with("/p2"))?;
            check(&c.label("a url typed over a pending load reaches the server"), reached);
            check(&c.label("the tab ends focused on /p2"), end.focused_url().ends_with("/p2"));
            c.finish(&["load in place", "swap"]);
        }
        "body-swap" => {
            let mut c = Case::start(ctx, name, view, "/swap")?;
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            // The page's body swap, while /slow holds its headers back.
            c.requested("/swapped")?;
            c.send("SLEEP 0.3")?;
            c.type_url("/p2")?;
            let reached = c.reaches("/p2", 1);
            c.then()?;
            let end = c.until("end", |s| s.focused_url().ends_with("/p2"))?;
            check(
                &c.label("a url typed over a pending load reaches the server while the page swaps its body"),
                reached,
            );
            check(&c.label("the tab ends focused on /p2"), end.focused_url().ends_with("/p2"));
            c.finish(&["load in place", "swap"]);
        }
        "live204" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/k")?;
            c.requested("/k")?;
            let t = Instant::now();
            c.send("WAITIDLE 15")?;
            let idle = t.elapsed();
            // Past the trail's deadline, which reverts a move no report answered.
            let mid = c.until("mid", |s| !s.focused_url().ends_with("/k"))?;
            c.then()?;
            c.type_url("/p2")?;
            let reached = c.reaches("/p2", 1);
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            c.type_url("/k")?;
            let twice = c.reaches("/k", 2);
            let t = Instant::now();
            c.send("WAITIDLE 15")?;
            let idle_again = t.elapsed();
            let end = c.until("end", |s| !s.focused_url().ends_with("/k"))?;
            check(&c.label("a 204 on a live page goes idle within 14 s"), idle < Duration::from_secs(14));
            check(&c.label("the tab stays off /k"), !mid.focused_url().ends_with("/k"));
            check(&c.label("a later typed url reaches the server"), reached);
            check(&c.label("a 204 typed over a pending load reaches the server"), twice);
            check(&c.label("the tab ends off /k"), !end.focused_url().ends_with("/k"));
            check(&c.label("the tab goes idle within 14 s after it"), idle_again < Duration::from_secs(14));
            if view == "servo" {
                // The second /k's pending view never commits (it is replaced by the next load).
                check(&c.label("the 204's pending view stays pending"), !settled(&c.tmp));
            }
            c.finish(&["load in place", "swap", "load in place", "swap"]);
        }
        "reload" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            let before = c.hits("/");
            c.send("RELOAD")?;
            let reloaded = c.reaches("/", before + 1);
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            let before = c.hits("/");
            c.send("RELOAD")?;
            let again = c.reaches("/", before + 1);
            c.then()?;
            let root = c.at("/");
            let end = c.until("end", |s| s.focused_url() == root)?;
            check(&c.label("Reload refetches the page"), reloaded);
            check(&c.label("a Reload over a pending load refetches the page"), again);
            check(&c.label("the tab ends on /"), end.focused_url() == root);
            c.finish(&["load in place", "load in place", "swap"]);
        }
        "reload-early" => {
            let mut c = Case::start(ctx, name, view, "/slow")?;
            c.send("RELOAD")?;
            c.send("SLEEP 10")?;
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("a Reload before the first commit refetches /slow"), c.hits("/slow") == 2);
            check(&c.label("the tab ends on /slow"), end.focused_url() == c.at("/slow"));
            c.finish(&["swap"]);
        }
        "paint" => {
            // The shown pixels across loads: red, then typed blue, then a slow page whose wait keeps blue on screen
            // (no blank frame), then the slow page.
            let mut c = Case::start(ctx, name, view, "/color?c=ff0000")?;
            c.then()?;
            let count = |path: &Path| -> io::Result<(usize, usize)> {
                Ok((pixels(path, [255, 0, 0])?, pixels(path, [0, 0, 255])?))
            };
            let red = count(&c.shot("red")?)?;
            c.type_url("/color?c=0000ff")?;
            c.requested("/color?c=0000ff")?;
            c.then()?;
            c.send("SLEEP 1")?;
            let blue = count(&c.shot("blue")?)?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            let waiting = count(&c.shot("waiting")?)?;
            c.send("SLEEP 6")?;
            c.then()?;
            let slow = count(&c.shot("slow")?)?;
            println!("    red {red:?}, blue {blue:?}, waiting {waiting:?}, slow {slow:?} (red, blue pixels)");
            const MANY: usize = 100_000;
            check(&c.label("the red page shows"), red.0 > MANY && red.1 == 0);
            check(&c.label("the typed blue page replaces it"), blue.1 > MANY && blue.0 == 0);
            check(&c.label("a pending load keeps the blue page shown"), waiting.1 > MANY);
            check(&c.label("the slow page replaces it"), slow == (0, 0));
            c.finish(&["load in place", "load in place"]);
        }
        "iframe" => {
            let mut c = Case::start(ctx, name, view, "/hop")?;
            let moved = poll("the frame's own navigation", WAIT, || c.hits("/color?c=ff00ff") > 0).is_ok();
            c.send("SLEEP 2")?;
            c.type_url("/p2")?;
            let reached = c.reaches("/p2", 1);
            c.then()?;
            check(&c.label("the frame navigates itself"), moved);
            check(&c.label("a url typed after an iframe navigates reaches the server"), reached);
            c.finish(&["swap"]);
        }
        "back-type" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/p2")?;
            c.requested("/p2")?;
            c.then()?;
            c.inst.send("GOBACK")?;
            c.until("back", |s| !s.focused_url().ends_with("/p2"))?;
            c.send("SLEEP 3")?;
            c.type_url("/p3")?;
            let reached = c.reaches("/p3", 1);
            c.then()?;
            check(&c.label("a url typed after a Back reaches the server"), reached);
            c.finish(&["load in place", "history in place", "load in place"]);
        }
        "forward" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/p2")?;
            c.requested("/p2")?;
            c.then()?;
            c.inst.send("GOBACK")?;
            c.until("back", |s| !s.focused_url().ends_with("/p2"))?;
            c.then()?;
            c.inst.send("GOFORWARD")?;
            let end = c.until("forward", |s| s.focused_url().ends_with("/p2"))?;
            c.then()?;
            check(&c.label("Forward returns to /p2"), end.focused_url().ends_with("/p2"));
            c.finish(&["load in place", "history in place", "history in place"]);
        }
        "back-pending" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/p2")?;
            c.requested("/p2")?;
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            c.send("GOBACK")?;
            c.send("SLEEP 8")?;
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("a Back over a typed pending load ends on /"), end.focused_url() == c.at("/"));
            c.finish(&["load in place", "load in place", "history swap"]);
        }
        "back-back" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            for p in ["/p2", "/p3"] {
                c.type_url(p)?;
                c.requested(p)?;
                c.then()?;
            }
            c.type_url("/slow")?;
            c.requested("/slow")?;
            // Two Backs at once. The app holds the second until the first arrives, so on Servo it comes after the
            // swapped-in view is shown, which has no entry behind it, and the app sends it as a load.
            c.send("GOBACK")?;
            c.send("GOBACK")?;
            c.send("SLEEP 8")?;
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("two Backs over a typed pending load end on /"), end.focused_url() == c.at("/"));
            c.finish(&["load in place", "load in place", "load in place", "history swap", "load in place"]);
        }
        "double-slow" => {
            // A typed red page that waits 4 s, then a typed blue one that waits 8 s: on Servo the second swaps, and
            // the shown view's red load commits while the blue view is still pending.
            let (red, blue) = ("/slow?c=ff0000", "/slow?s=8&c=0000ff");
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url(red)?;
            c.requested(red)?;
            c.type_url(blue)?;
            c.requested(blue)?;
            let t = Instant::now();
            let mut red_seen = Vec::new();
            let mut urls = Vec::new();
            while t.elapsed() < Duration::from_secs(14) {
                let shot = c.shot("t")?;
                let (r, b) = (pixels(&shot, [255, 0, 0])?, pixels(&shot, [0, 0, 255])?);
                let url = c.state("t")?.focused_url().rsplit('/').next().unwrap_or("").to_owned();
                let s = t.elapsed().as_secs_f32();
                println!("    {s:4.1} s: red {r} px, blue {b} px, url {url}");
                if r > 10_000 {
                    red_seen.push(s);
                }
                urls.push(url);
            }
            c.then()?;
            c.send("GOBACK")?;
            c.send("SLEEP 6")?;
            c.then()?;
            let back = c.state("back")?;
            println!("    Back from blue: {}", back.focused_url());
            if view == "servo" {
                // PARKED limit 9: the shown view's red commit is painted until blue promotes.
                check(&c.label("the red page shows while blue is pending"), !red_seen.is_empty());
            } else {
                check(&c.label("the red page never shows over the pending blue one"), red_seen.is_empty());
            }
            check(&c.label("the tab's url never reads red"), !urls.iter().any(|u| u.contains("ff0000")));
            check(&c.label("Back from blue ends on /"), back.focused_url() == c.at("/"));
            c.finish(&["load in place", "swap", "load in place"]);
        }
        "spa-back" => {
            let mut c = Case::start(ctx, name, view, "/spa")?;
            c.requested("/slow")?;
            c.until("pushed", |s| s.focused_url().ends_with("/spa2"))?;
            c.send("GOBACK")?;
            let fetched = c.reaches("/fetched", 1);
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("a Back to a pushed entry leaves the page's fetch running"), fetched);
            check(&c.label("the tab ends on /spa"), end.focused_url() == c.at("/spa"));
            c.finish(&["history in place"]);
        }
        "back-page" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/go")?;
            // The page's own load.
            c.requested("/slow")?;
            c.send("GOBACK")?;
            c.send("SLEEP 8")?;
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("a Back over the page's pending load ends on /"), end.focused_url() == c.at("/"));
            c.finish(&["load in place", "history swap"]);
        }
        "back-swap" => {
            let mut c = Case::start(ctx, name, view, "/")?;
            c.then()?;
            c.type_url("/hop")?;
            poll("the frame's own navigation", WAIT, || c.hits("/color?c=ff00ff") > 0)?;
            c.then()?;
            c.type_url("/slow")?;
            c.requested("/slow")?;
            c.send("GOBACK")?;
            // Past /slow's headers: a /slow that commits over the Back replaces the page. The screen, not the app's
            // url, says which page is shown (the app shows the typed url until the trail settles). The frame's page
            // is either: WebView2's Back moves only the frame (green), Servo's swap loads /hop afresh, whose frame
            // hops again (magenta).
            c.send("SLEEP 8")?;
            let shot = c.shot("back")?;
            let frame = pixels(&shot, [0, 255, 0])? + pixels(&shot, [255, 0, 255])?;
            c.then()?;
            let end = c.state("end")?;
            check(&c.label("the Back shows /hop's frame and /slow never replaces it"), frame > 10_000);
            check(&c.label("the tab ends on /hop"), end.focused_url() == c.at("/hop"));
            c.finish(&["load in place", "swap", "history swap"]);
        }
        "zoom-swap" => {
            let (red, blue) = ([255, 0, 0], [0, 0, 255]);
            let mut c = Case::start(ctx, name, view, "/color?c=ff0000&w=100")?;
            c.then()?;
            c.send("SLEEP 1")?;
            let w0 = width_of(&c.shot("red")?, red)?;
            c.send("ZOOMIN")?;
            let deadline = Instant::now() + WAIT;
            let mut wr = w0;
            while wr == w0 && Instant::now() < deadline {
                wr = width_of(&c.shot("zoomed")?, red)?;
            }
            c.type_url("/slow")?;
            c.requested("/slow")?;
            c.type_url("/color?c=0000ff&w=100")?;
            let deadline = Instant::now() + WAIT;
            let mut shown = false;
            while !shown && Instant::now() < deadline {
                shown = width_of(&c.shot("blue-wait")?, blue)? > 0;
            }
            c.then()?;
            c.send("SLEEP 1")?;
            let wb = width_of(&c.shot("blue")?, blue)?;
            println!("    red {w0} px, zoomed {wr} px, blue after the swap {wb} px");
            check(&c.label("the blue page shows"), shown);
            check(&c.label("zoom widens the square"), wr > w0);
            check(&c.label("the page after a swap keeps the zoom"), wb.abs_diff(wr) <= 2);
            c.finish(&["load in place", "swap"]);
        }
        other => return Err(io::Error::other(format!("unknown case {other}"))),
    }
    Ok(())
}

pub(crate) fn loads(ctx: &mut Ctx) -> io::Result<()> {
    let cases = std::env::var("CASES").unwrap_or_else(|_| CASES.join(" "));
    for view in views() {
        for case in cases.split_whitespace() {
            let _t = Section::start(&format!("loads-{case}-{view}"));
            guarded(&format!("loads {view}: {case}"), || run_case(ctx, case, &view));
        }
    }
    Ok(())
}
