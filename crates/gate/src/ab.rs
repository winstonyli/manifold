//! `gate ab NAME SCENARIO [SECONDS] [--repeats N] [--quiet PCT] ARM...`: times `gate profile SCENARIO SECONDS
//! --visible` once per arm per repeat, interleaved (arm a, arm b, arm a, ...) so load from other sessions hits every
//! arm alike, and prints the median frame time and CPU of each phase per arm. An ARM is `label:VAR=value[,VAR=value]`:
//! the environment the run gets on top of this process's own (`GRAPH_NOTES=400` is shared by setting it before
//! `gate ab`). An arm is an environment switch, so two builds of the code are not an A/B here; put the switch in the code.
//!
//! Each run goes through `~/.claude/runs/run.ps1` as `ab-NAME-LABEL-rN` (RUNS.md), which records its conditions and
//! takes the headline `gate profile` writes. A run waits for the machine to be quiet first (total CPU under PCT, default
//! 15, for up to 10 minutes); one that starts anyway says so in the summary. Needs the release exe `gate profile` needs.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::trace::{phase_stats, Phase};

const QUIET_DEADLINE: Duration = Duration::from_secs(600);
const QUIET_WINDOW: Duration = Duration::from_secs(2);

struct Arm {
    label: String,
    env: Vec<(String, String)>,
}

fn parse_arm(text: &str) -> Result<Arm, String> {
    let (label, vars) =
        text.split_once(':').ok_or_else(|| format!("ARM {text:?} is not label:VAR=value[,VAR=value]"))?;
    let valid = !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(format!("ARM label {label:?} must be letters, digits, . _ -"));
    }
    let env = vars
        .split(',')
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.split_once('=')
                .map(|(k, val)| (k.to_owned(), val.to_owned()))
                .ok_or_else(|| format!("ARM {text:?}: {v:?} is not VAR=value"))
        })
        .collect::<Result<_, _>>()?;
    Ok(Arm { label: label.to_owned(), env })
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Per phase, per arm: median frame50 with its range over the repeats, and median CPU.
fn summary(arms: &[(String, Vec<Vec<Phase>>)]) -> String {
    let mut out = format!("{:<16}", "frame50 [min-max] / cpu");
    for (label, _) in arms {
        out.push_str(&format!(" | {label:<28}"));
    }
    out.push('\n');
    let first = arms.first().and_then(|(_, runs)| runs.first());
    let names: Vec<String> = first.map_or(vec![], |r| r.iter().map(|p| p.name.clone()).collect());
    for name in &names {
        out.push_str(&format!("{name:<16}"));
        for (_, runs) in arms {
            let of = |f: &dyn Fn(&Phase) -> Option<f64>| -> Vec<f64> {
                runs.iter().filter_map(|r| r.iter().find(|p| &p.name == name)).filter_map(f).collect()
            };
            let f50 = of(&|p| Some(p.frame50));
            let cpu = of(&|p| p.cpu);
            let lo = f50.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = f50.iter().copied().fold(0.0, f64::max);
            let cell = format!("{:.2} [{lo:.1}-{hi:.1}] / {:.0}", median(&f50), median(&cpu));
            out.push_str(&format!(" | {cell:<28}"));
        }
        out.push('\n');
    }
    out
}

/// Waits (up to `QUIET_DEADLINE`) until the machine's CPU is under `quiet` percent; the last reading, if it never was.
fn wait_quiet(quiet: f64) -> Option<f64> {
    let deadline = Instant::now() + QUIET_DEADLINE;
    let mut load = crate::all::cpu_over(QUIET_WINDOW, &AtomicBool::new(false));
    while load.is_some_and(|l| l > quiet) && Instant::now() < deadline {
        load = crate::all::cpu_over(QUIET_WINDOW, &AtomicBool::new(false));
    }
    load
}

fn run(args: &[String]) -> Result<(), String> {
    let (mut repeats, mut quiet) = (3usize, 15.0f64);
    let mut pos: Vec<&str> = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repeats" => repeats = it.next().and_then(|v| v.parse().ok()).ok_or("--repeats needs a number")?,
            "--quiet" => quiet = it.next().and_then(|v| v.parse().ok()).ok_or("--quiet needs a percent")?,
            other => pos.push(other),
        }
    }
    let [name, scenario, rest @ ..] = pos.as_slice() else {
        return Err("usage: gate ab NAME SCENARIO [SECONDS] [--repeats N] [--quiet PCT] ARM...".into());
    };
    let (seconds, arm_texts) = match rest.split_first() {
        Some((first, more)) if first.parse::<f64>().is_ok() => (*first, more),
        _ => ("20", rest),
    };
    let arms: Vec<Arm> = arm_texts.iter().map(|a| parse_arm(a)).collect::<Result<_, _>>()?;
    if arms.len() < 2 {
        return Err("an A/B needs at least two ARMs".into());
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| e.to_string())?;
    let repo = repo.to_str().and_then(|s| s.strip_prefix(r"\\?\")).map_or(repo.clone(), Into::into);
    let run_ps1 = std::env::var_os("USERPROFILE").map(|h| Path::new(&h).join(".claude/runs/run.ps1"));
    let run_ps1 = run_ps1.filter(|p| p.exists()).ok_or("~/.claude/runs/run.ps1 not found: see RUNS.md")?;
    let trace = repo.join("target/profile/trace.json");
    let mut results: Vec<(String, Vec<Vec<Phase>>)> = arms.iter().map(|a| (a.label.clone(), vec![])).collect();
    let mut contended = vec![];
    for r in 1..=repeats {
        for (i, arm) in arms.iter().enumerate() {
            let load = wait_quiet(quiet);
            if load.is_none_or(|l| l > quiet) {
                contended.push(format!("{}-r{r} (cpu {})", arm.label, load.map_or("?".into(), |l| format!("{l:.0}%"))));
            }
            let id = format!("ab-{name}-{}-r{r}", arm.label);
            println!("run {id} ...");
            let _ = std::fs::remove_file(&trace);
            let status = Command::new("powershell")
                .current_dir(&repo)
                .args(["-NoProfile", "-File"])
                .arg(&run_ps1)
                .args(["-Kind", "bench", "-Name", &id, "-Priority", "Normal", "-Command"])
                .arg(format!(r"target\debug\gate.exe profile {scenario} {seconds} --visible"))
                .envs(arm.env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&trace)
                .map_err(|_| format!("{id}: no trace (exit {status}); see its out.log and err.log"))?;
            let phases = phase_stats(&text);
            if phases.is_empty() {
                return Err(format!("{id}: the trace has no phases (exit {status})"));
            }
            results[i].1.push(phases);
        }
    }
    print!("\n{}", summary(&results));
    if contended.is_empty() {
        println!("every run started with the CPU under {quiet}%");
    } else {
        println!("CONTENDED (CPU stayed over {quiet}% for 10 min): {}", contended.join(", "));
    }
    Ok(())
}

pub fn ab(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase(name: &str, frame50: f64, cpu: f64) -> Phase {
        Phase {
            name: name.into(),
            secs: 4.0,
            fps: 30.0,
            frame50,
            frame99: 0.0,
            frame_max: 0.0,
            wait50: 0.0,
            input50: 0.0,
            input99: 0.0,
            cpu: Some(cpu),
        }
    }

    #[test]
    fn an_arm_is_a_label_and_its_environment() {
        let a = parse_arm("dx12:WGPU_BACKEND=dx12,X=1").unwrap();
        assert_eq!((a.label.as_str(), a.env.len()), ("dx12", 2));
        assert_eq!(a.env[0], ("WGPU_BACKEND".to_owned(), "dx12".to_owned()));
        assert!(parse_arm("base:").unwrap().env.is_empty(), "an arm may add nothing: the baseline");
        assert!(parse_arm("no-colon").is_err() && parse_arm("a b:X=1").is_err() && parse_arm("a:X").is_err());
    }

    #[test]
    fn the_summary_takes_each_arms_median_over_its_repeats() {
        let a = vec![vec![phase("pan", 10.0, 100.0)], vec![phase("pan", 30.0, 300.0)], vec![phase("pan", 20.0, 200.0)]];
        let arms = vec![("a".to_owned(), a), ("b".to_owned(), vec![vec![phase("pan", 5.0, 50.0)]])];
        let s = summary(&arms);
        assert!(s.contains("20.00 [10.0-30.0] / 200"), "{s}");
        assert!(s.contains("5.00 [5.0-5.0] / 50"), "{s}");
    }
}
