//! Reads a `MANIFOLD_TRACE` file (`telemetry.rs` in the `manifold` crate: Chrome trace events, one per line) into numbers
//! per phase of a script, the phases being what its `MARK label` lines cut the run into: frame time, input latency (an
//! input's arrival to the end of the first frame that started after it, and the `wait` before that frame starts), redraws a second and CPU per second.

use serde_json::Value;
use std::fmt::Write as _;

struct Frame {
    start: f64,
    end: f64,
    egui: f64,
    render: f64,
    /// Milliseconds from the earliest input that arrived since the last frame ended to this frame's start (the
    /// loop's pacing) and to its end (the latency).
    input_wait: Option<f64>,
    input_latency: Option<f64>,
}

struct Mark {
    at: f64,
    name: String,
    cpu_us: Option<f64>,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted.get(((sorted.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0)
}

fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v
}

/// All times in milliseconds from the start of the process.
fn parse(text: &str) -> (Vec<Frame>, Vec<Mark>) {
    let (mut frames, mut egui, mut render, mut inputs, mut marks) = (vec![], vec![], vec![], vec![], vec![]);
    for row in text.lines().filter_map(|l| serde_json::from_str::<Value>(l.trim_end_matches(',')).ok()) {
        let ms = |key: &str| row[key].as_f64().unwrap_or(0.0) / 1000.0;
        match (row["ph"].as_str(), row["name"].as_str().unwrap_or(""), row["cat"].as_str()) {
            (Some("X"), "frame", _) => frames.push((ms("ts"), ms("dur"))),
            (Some("X"), "egui", _) => egui.push((ms("ts"), ms("dur"))),
            (Some("X"), "render", _) => render.push((ms("ts"), ms("dur"))),
            (Some("i"), _, Some("input")) => inputs.push(ms("ts")),
            (Some("i"), name, Some("mark")) => {
                marks.push(Mark { at: ms("ts"), name: name.to_owned(), cpu_us: row["args"]["cpu_us"].as_f64() })
            }
            _ => {}
        }
    }
    frames.sort_by(|a, b| a.0.total_cmp(&b.0));
    inputs.sort_by(f64::total_cmp);
    marks.sort_by(|a, b| a.at.total_cmp(&b.at));
    let inside = |spans: &[(f64, f64)], start: f64, end: f64| {
        spans.iter().filter(|s| s.0 >= start && s.0 <= end).map(|s| s.1).sum::<f64>()
    };
    let mut previous_end = f64::NEG_INFINITY;
    let frames = frames
        .into_iter()
        .map(|(start, dur)| {
            let input = inputs.iter().find(|&&at| at > previous_end && at <= start).copied();
            previous_end = start + dur;
            Frame {
                start,
                end: start + dur,
                egui: inside(&egui, start, start + dur),
                render: inside(&render, start, start + dur),
                input_wait: input.map(|at| start - at),
                input_latency: input.map(|at| start + dur - at),
            }
        })
        .collect();
    (frames, marks)
}

/// The per-phase table of a trace, and its slowest frames.
pub fn summarize(text: &str) -> String {
    let (frames, marks) = parse(text);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<16} {:>6} {:>6} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>8}",
        "phase", "secs", "fps", "frame50", "frame99", "frameMax", "wait50", "input50", "input99", "cpu ms/s"
    );
    let mut worst: Vec<(f64, &str, &Frame)> = vec![];
    for (i, mark) in marks.iter().enumerate() {
        let Some(next) = marks.get(i + 1) else { break };
        let phase: Vec<&Frame> = frames.iter().filter(|f| f.start >= mark.at && f.start < next.at).collect();
        let secs = (next.at - mark.at) / 1000.0;
        let durs = sorted(phase.iter().map(|f| f.end - f.start));
        let waits = sorted(phase.iter().filter_map(|f| f.input_wait));
        let latencies = sorted(phase.iter().filter_map(|f| f.input_latency));
        let cpu = mark.cpu_us.zip(next.cpu_us).map(|(a, b)| (b - a) / 1000.0 / secs);
        let _ = writeln!(
            out,
            "{:<16} {:>6.1} {:>6.1} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>9.2} {:>9.2} {:>8}",
            mark.name,
            secs,
            phase.len() as f64 / secs,
            percentile(&durs, 0.5),
            percentile(&durs, 0.99),
            durs.last().copied().unwrap_or(0.0),
            percentile(&waits, 0.5),
            percentile(&latencies, 0.5),
            percentile(&latencies, 0.99),
            cpu.map_or("-".to_owned(), |c| format!("{c:.0}")),
        );
        worst.extend(phase.into_iter().map(|f| (f.end - f.start, mark.name.as_str(), f)));
    }
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (dur, phase, f) in worst.iter().take(5) {
        let _ = writeln!(
            out,
            "slow frame {dur:.1} ms in {phase} at {:.1} s (egui {:.1}, render {:.1})",
            f.start / 1000.0,
            f.egui,
            f.render
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_split_at_marks_and_input_waits_end_at_the_next_frame() {
        let t = |ph: &str, name: &str, cat: &str, ts: u64, dur: u64, cpu: u64| {
            let args = if cat == "mark" { format!(r#"{{"cpu_us":{cpu}}}"#) } else { "null".into() };
            format!(r#"{{"name":"{name}","cat":"{cat}","ph":"{ph}","ts":{ts},"dur":{dur},"args":{args}}},"#)
        };
        let trace = [
            "[".to_owned(),
            t("i", "idle", "mark", 0, 0, 1_000),
            t("i", "input", "input", 10_000, 0, 0),
            t("X", "frame", "", 12_000, 4_000, 0),
            t("X", "egui", "", 12_500, 1_000, 0),
            t("X", "render", "", 14_000, 2_000, 0),
            t("X", "frame", "", 100_000, 2_000, 0),
            t("i", "end", "mark", 1_000_000, 0, 501_000),
        ]
        .join("\n");
        let (frames, marks) = parse(&trace);
        assert_eq!((frames.len(), marks.len()), (2, 2));
        assert_eq!(
            (frames[0].input_wait, frames[0].input_latency),
            (Some(2.0), Some(6.0)),
            "input at 10 ms, frame 12-16 ms"
        );
        assert_eq!((frames[0].egui, frames[0].render, frames[1].input_wait), (1.0, 2.0, None));
        let table = summarize(&trace);
        let row = table.lines().nth(1).unwrap();
        assert!(row.starts_with("idle") && row.contains("2.0") && row.trim_end().ends_with("500"), "{table}");
        assert!(table.contains("slow frame 4.0 ms in idle at 0.0 s"), "{table}");
    }
}
