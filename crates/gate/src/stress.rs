//! The two probes that stress an engine host or a big session, as `gate` stages (they were bash scripts, and a Git Bash
//! fork costs seconds on this machine). PowerShell runs only where Windows has no plain API in reach: suspending a
//! process and reading a process tree's memory.
//!
//! - `unresponsive` (was scripts/unresponsive-probe.sh): a suspended WebView2 host reads "not responding"
//! - `scale TABS...` (was scripts/scale-test.sh): what restoring a big session costs

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::check::{check, check_eq, Section};
use crate::http::{self, response};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, setup, tab_node};
use crate::state::{State, TAB};

/// Runs `script` in PowerShell (a file, so no quoting) and returns its non-empty output lines.
pub(crate) fn powershell(work: &Path, name: &str, script: &str, args: &[&str]) -> io::Result<Vec<String>> {
    let file = work.join(format!("{name}.ps1"));
    fs::write(&file, script)?;
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&file)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect())
}

/// Suspends (NtSuspendProcess) the WebView2 engine hosts whose parent is `parent`; returns their pids.
fn suspend_engines(work: &Path, parent: u32) -> io::Result<Vec<String>> {
    const SCRIPT: &str = "param([int]$Parent)
Add-Type -Namespace P -Name N -MemberDefinition '[DllImport(\"ntdll.dll\")] public static extern int NtSuspendProcess(IntPtr h);'
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='manifold-engine-webview2.exe'\" | ForEach-Object {
    $null = [P.N]::NtSuspendProcess((Get-Process -Id $_.ProcessId).Handle)
    $_.ProcessId
}
";
    powershell(work, "suspend", SCRIPT, &["-Parent", &parent.to_string()])
}

/// "unresponsive" and "loaded" of the card `id`.
fn flags(state: &State, id: &str) -> (bool, bool) {
    let card = state.card(id);
    (
        card.is_some_and(|c| c["unresponsive"].as_bool() == Some(true)),
        card.is_some_and(|c| c["loaded"].as_bool() == Some(true)),
    )
}

/// A seeded tab's engine host is suspended and 1000 characters typed into the page, so the app's writer for it blocks
/// and its queue fills. The card must then read `unresponsive` with a "Not responding" toast while the app keeps
/// answering; `RESTARTVIEW` must bring the tab back live and responsive and end the suspended host; and with a second
/// host suspended `QUIT` must end the app within 10 s: closing a stuck tab never waits on it.
pub(crate) fn unresponsive(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "unresponsive") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let _t = Section::start("unresponsive");
    guarded("unresponsive", || {
        let profile = work.join("appdata");
        seed(&exe, &profile, "webview2", &[tab_node(Some("about:blank"), "webview2")])?;
        let mut inst = Instance::launch(&exe, &profile, &work, "app.log")?;
        for line in ["WAITTABS", "SLEEP 4"] {
            inst.send(line)?;
        }
        let live = inst.export_state("live")?;
        // The import may give the node a new id: the one card's id.
        let id = live.cards().first().and_then(|c| c["id"].as_str()).unwrap_or("t1").to_owned();
        check_eq("the tab is live", true, flags(&live, &id).1);

        // Into Tab view, so typing reaches the page; then suspend its engine.
        for line in ["GRAPHDOWN", "GRAPHOPEN", "SLEEP 4"] {
            inst.send(line)?;
        }
        inst.export_state("open")?;
        let first = suspend_engines(&work, inst.pid())?;
        if !check("an engine host was suspended", !first.is_empty()) {
            return Ok(());
        }
        // The stall reads after UNRESPONSIVE_AFTER (2 s) of a blocked write, so wait 3 s; then back in Graph view before
        // exporting (the cards are the graph's), and within the toast's 4 s.
        for line in [format!("TYPE {}", "a".repeat(1000)), "SLEEP 3".into(), "ESCAPE".into(), "SLEEP 1".into()] {
            inst.send(&line)?;
        }
        let stuck = inst.export_state("stuck")?;
        check_eq("the card reads unresponsive", true, flags(&stuck, &id).0);
        let toast = stuck.toast().unwrap_or("").to_owned();
        if !check("the toast says not responding", toast.starts_with("Not responding: ")) {
            println!("  toast: [{toast}]");
        }

        for line in [format!("RESTARTVIEW {id}"), "WAITTABS".into(), "SLEEP 4".into()] {
            inst.send(&line)?;
        }
        let restarted = inst.export_state("restarted")?;
        check_eq("restarted: live", true, flags(&restarted, &id).1);
        check_eq("restarted: responsive", false, flags(&restarted, &id).0);
        const ALIVE: &str = "param([int]$P)
if (Get-Process -Id $P -ErrorAction SilentlyContinue) { 'alive' } else { 'gone' }
";
        let alive = powershell(&work, "alive", ALIVE, &["-P", &first[0]])?;
        check_eq("the suspended host was ended", "gone", alive.first().map_or("?", String::as_str));

        // A second stuck engine, then quit: exit must not wait on it.
        for line in ["GRAPHDOWN", "GRAPHOPEN", "SLEEP 4"] {
            inst.send(line)?;
        }
        inst.export_state("open2")?;
        if !check("a second engine host was suspended", !suspend_engines(&work, inst.pid())?.is_empty()) {
            return Ok(());
        }
        for line in [format!("TYPE {}", "b".repeat(1000)), "SLEEP 3".into(), "ESCAPE".into(), "SLEEP 1".into()] {
            inst.send(&line)?;
        }
        let stuck2 = inst.export_state("stuck2")?;
        check_eq("the second host is stuck too", true, flags(&stuck2, &id).0);
        let quit_at = Instant::now();
        let status = inst.quit(Duration::from_secs(15));
        let took = quit_at.elapsed();
        println!("quit took about {} s", took.as_secs());
        check("QUIT with a stuck tab ends within 10 s", status.is_ok() && took <= Duration::from_secs(10));
        Ok(())
    });
    finish(&work)
}

/// A medium page (~60 KB of text and a script that retitles it every second): a light stand-in for a real site.
fn medium_page() -> String {
    let para = format!("<p>{}</p>\n", "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(20));
    format!(
        "<!doctype html><title>page</title><body>{}<script>let n = 0; setInterval(function () {{ \
         document.title = 'page ' + (++n); }}, 1000);</script>",
        para.repeat(50)
    )
}

/// A process tree's size: process count, private memory and working set, then private memory per process name.
const MEMORY: &str = "param([int]$Root)
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
$ids = @($Root); $i = 0
while ($i -lt $ids.Count) { $p = $ids[$i]; $ids += @($all | Where-Object { $_.ParentProcessId -eq $p -and $ids -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId }); $i++ }
$procs = $ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
$mb = { param($g) [math]::Round((($g | Measure-Object PrivateMemorySize64 -Sum).Sum) / 1MB) }
'  {0} processes, {1} MB private, {2} MB working set' -f $procs.Count, (& $mb $procs), [math]::Round((($procs | Measure-Object WorkingSet64 -Sum).Sum) / 1MB)
$procs | Group-Object ProcessName | ForEach-Object { '    {0} x{1} {2} MB' -f $_.Name, $_.Count, (& $mb $_.Group) }
";

/// For each count in `counts`, one instance restores that many tabs of a local page and prints how long until the
/// restore settled (`WAITTABS`; the focused tab must be live and the rest asleep) and, 15 s later, its process tree's
/// memory. `ENGINE` (default webview2) is the engine every tab uses.
pub(crate) fn scale(counts: &[String]) -> ExitCode {
    let counts: Vec<usize> = counts.iter().filter_map(|c| c.parse().ok()).filter(|n| *n > 0).collect();
    if counts.is_empty() {
        eprintln!("usage: [ENGINE=cef] gate scale <tabs>...");
        return ExitCode::from(2);
    }
    let (exe, work) = match setup(None, "scale") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let engine = std::env::var("ENGINE").unwrap_or_else(|_| "webview2".into());
    let page = medium_page();
    let server =
        match http::Server::start(Arc::new(move |_: &str| response("200 OK", &[], "text/html", page.as_bytes()))) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("site: {e}");
                return ExitCode::from(1);
            }
        };
    for n in counts {
        let _t = Section::start(&format!("scale-{n}"));
        guarded(&format!("{n} tabs"), || {
            let nodes: Vec<Value> = (1..=n)
                .map(|i| {
                    json!({"id": format!("t{i}"), "type": TAB, "url": format!("{}/p?i={i}", server.base()),
                           "view_kind": engine, "pos": [(i % 10 * 300) as f64, (i / 10 * 200) as f64]})
                })
                .collect();
            let profile = work.join(format!("appdata-{n}"));
            seed(&exe, &profile, &engine, &nodes)?;
            let start = Instant::now();
            let mut inst = Instance::launch(&exe, &profile, &work, &format!("scale-{n}.log"))?;
            inst.send("WAITTABS")?;
            let spawned = inst.export_state(&format!("spawned-{n}"))?;
            let elapsed = start.elapsed().as_secs_f64();
            let count = |loaded| spawned.cards().iter().filter(|c| State::card_is(c, TAB, Some(loaded))).count();
            let (live, asleep) = (count(true), count(false));
            let want = format!("{n} tabs: {live} live and {asleep} asleep, expected 1 and {}", n - 1);
            if !check(&want, live == 1 && live + asleep == n) {
                return Ok(());
            }
            println!("{engine}, {n} tabs: restored in {elapsed:.1}s, 1 live and {asleep} asleep");
            inst.send("SLEEP 15")?;
            inst.export_state(&format!("settled-{n}"))?;
            for line in powershell(&work, "memory", MEMORY, &["-Root", &inst.pid().to_string()])? {
                println!("{line}");
            }
            quit(&mut inst, &format!("{n} tabs"));
            Ok(())
        });
    }
    finish(&work)
}
