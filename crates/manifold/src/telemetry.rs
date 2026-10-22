//! Timing traces: with `MANIFOLD_TRACE=<path>` set, every `tracing` span and event is written to `<path>` as Chrome
//! trace events (one JSON object per line inside an unclosed array, which Perfetto and `chrome://tracing` accept).
//! `gate profile` reads the same file for its summary. Unset, no subscriber is installed and a span costs an atomic
//! load.
//!
//! The spans: `frame` (one `RedrawRequested`), with `egui` (the UI pass) and `render` (draw and present) inside it.
//! The events: `input` (an OS or scripted input arrived, `event_loop`), `repaint` (egui asked for another frame at
//! once, named by the call that asked), and a script's `MARK label`.

use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

struct Sink {
    out: Mutex<std::io::BufWriter<std::fs::File>>,
    epoch: Instant,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Installs the trace writer when `MANIFOLD_TRACE` names a file.
pub(crate) fn init() {
    let Some(path) = std::env::var_os("MANIFOLD_TRACE") else { return };
    let file = match std::fs::File::create(&path) {
        Ok(file) => file,
        Err(e) => return eprintln!("MANIFOLD_TRACE: cannot create {}: {e}", path.to_string_lossy()),
    };
    open(file);
    let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry().with(ChromeLayer));
}

fn open(file: std::fs::File) {
    let mut out = std::io::BufWriter::new(file);
    let _ = writeln!(out, "[");
    let _ = SINK.set(Sink { out: Mutex::new(out), epoch: Instant::now() });
}

/// Writes out what is buffered: at a clean exit.
pub(crate) fn flush() {
    if let Some(Ok(mut out)) = SINK.get().map(|sink| sink.out.lock()) {
        let _ = out.flush();
    }
}

fn micros(sink: &Sink, at: Instant) -> u128 {
    at.saturating_duration_since(sink.epoch).as_micros()
}

fn write(sink: &Sink, event: serde_json::Value) {
    if let Ok(mut out) = sink.out.lock() {
        let _ = writeln!(out, "{event},");
    }
}

/// This process's CPU time, kernel and user together, in microseconds; `None` off Windows.
#[cfg(windows)]
fn process_cpu_micros() -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle of the current process is always valid, and each pointer is to a live local.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) };
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (ok != 0).then(|| (ticks(kernel) + ticks(user)) / 10)
}

#[cfg(not(windows))]
fn process_cpu_micros() -> Option<u64> {
    None
}

#[derive(Clone, Copy)]
struct Start(Instant);

/// The `label` field of a `mark` event.
#[derive(Default)]
struct Label(Option<String>);

impl Visit for Label {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "label" {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "label" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

struct ChromeLayer;

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ChromeLayer {
    fn on_new_span(&self, _: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Start(Instant::now()));
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let (Some(sink), Some(span)) = (SINK.get(), ctx.span(&id)) else { return };
        let Some(start) = span.extensions().get::<Start>().copied() else { return };
        let (ts, dur) = (micros(sink, start.0), start.0.elapsed().as_micros());
        write(sink, serde_json::json!({"name": span.name(), "ph": "X", "ts": ts, "dur": dur, "pid": 1, "tid": 1}));
    }

    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let Some(sink) = SINK.get() else { return };
        let mut label = Label::default();
        event.record(&mut label);
        let cat = event.metadata().target();
        let name = label.0.unwrap_or_else(|| cat.to_owned());
        let ts = micros(sink, Instant::now());
        // A mark also carries the process's CPU so far, which is what a phase's CPU cost is read from.
        let args =
            if cat == "mark" { serde_json::json!({"cpu_us": process_cpu_micros()}) } else { serde_json::Value::Null };
        write(
            sink,
            serde_json::json!({"name": name, "cat": cat, "ph": "i", "s": "g", "ts": ts, "pid": 1, "tid": 1, "args": args}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_events_become_chrome_trace_lines() {
        let path = std::env::temp_dir().join(format!("manifold-trace-{}.json", std::process::id()));
        open(std::fs::File::create(&path).unwrap());
        tracing::subscriber::with_default(tracing_subscriber::registry().with(ChromeLayer), || {
            let frame = tracing::info_span!("frame").entered();
            tracing::info!(target: "mark", label = "phase one");
            drop(frame);
        });
        flush();
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> =
            text.lines().skip(1).map(|l| serde_json::from_str(l.trim_end_matches(',')).unwrap()).collect();
        assert_eq!(rows.len(), 2, "{text}");
        assert_eq!((rows[0]["name"].as_str(), rows[0]["ph"].as_str()), (Some("phase one"), Some("i")));
        assert_eq!((rows[1]["name"].as_str(), rows[1]["ph"].as_str()), (Some("frame"), Some("X")));
        let _ = std::fs::remove_file(&path);
    }
}
