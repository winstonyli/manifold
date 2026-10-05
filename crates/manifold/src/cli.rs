//! Command-line tools for session files (spec "JSON snapshots", CLI):
//! `--dump <file.mnf>` prints frames and the document as JSON, `--check
//! <file.mnf>` reports damage and a torn tail, and `--import-json <in>
//! --out <file.mnf>` builds a session file from a JSON snapshot (used by
//! drive.sh's `SEED`).

use std::path::Path;

use crate::local_state::LocalState;
use crate::store::journal::{inspect, read_readable_header, Journal};
use crate::store::json::{self, JsonDoc};
use crate::store::model::ActorId;

/// Runs the tool the arguments name: `None` when they name none.
pub(crate) fn run(args: &[String]) -> Option<i32> {
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let result = if let Some(file) = arg("--dump") {
        dump(Path::new(file))
    } else if let Some(file) = arg("--check") {
        check(Path::new(file))
    } else {
        let input = arg("--import-json")?;
        match arg("--out") {
            Some(out) => import_json(Path::new(input), Path::new(out)),
            None => Err("--import-json needs --out <file.mnf>".into()),
        }
    };
    Some(match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("manifold: {err}");
            2
        }
    })
}

fn dump(path: &Path) -> Result<i32, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = inspect(&bytes).map_err(|e| e.to_string())?;
    println!(
        "version {} generation {} prev_generation {} file_len {}",
        info.version, info.generation, info.prev_generation, info.file_len
    );
    for (offset, ty, size) in &info.frames {
        println!("frame at {offset}: type {ty}, {size} bytes");
    }
    let doc = Journal::read(&bytes).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&json::doc_to_json(&doc)).expect("JSON serializes"));
    Ok(0)
}

/// Exit status 0 when clean, 1 when damaged or torn; an error (2) for a
/// file this build won't read, an older format version included.
fn check(path: &Path) -> Result<i32, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = inspect(&bytes).map_err(|e| e.to_string())?;
    read_readable_header(&bytes).map_err(|e| format!("{e} (format version {})", info.version))?;
    if info.header_damaged {
        println!("header damaged");
    }
    for (start, end) in &info.damaged {
        println!("damaged: bytes {start}..{end}");
    }
    if let Some((at, why)) = &info.torn {
        println!("torn tail at {at}: {why:?}");
    }
    let clean = !info.header_damaged && info.damaged.is_empty() && info.torn.is_none();
    println!("{}", if clean { "clean" } else { "damaged" });
    Ok(if clean { 0 } else { 1 })
}

fn import_json(input: &Path, out: &Path) -> Result<i32, String> {
    if out.exists() {
        return Err(format!("{} exists; not overwriting it", out.display()));
    }
    let text = std::fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let json: JsonDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut journal = Journal::open(out, ActorId::random()).map_err(|e| e.to_string())?;
    let mut txn = journal.begin().map_err(|e| e.to_string())?;
    let imported = json::import(&json, || txn.next_id(), json::Trust::Foreign)?;
    txn.commit(json::creates(&imported)).map_err(|e| e.to_string())?;
    if let Some(local) = json.local.as_ref().and_then(|v| serde_json::from_value::<LocalState>(v.clone()).ok()) {
        let map = json.nodes.iter().zip(&imported).map(|(n, i)| (n.id.clone(), i.id.to_string())).collect();
        journal.write_local(&local.remap(&map).to_bytes()).map_err(|e| e.to_string())?;
    }
    journal.close().map_err(|e| e.to_string())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_json_builds_a_clean_file_that_dumps_back() {
        let dir = crate::store::testutil::temp_dir("cli");
        let input = dir.join("seed.json");
        std::fs::write(
            &input,
            r#"{"nodes":[{"id":"g","type":"application/vnd.manifold.group","name":"G"},{"id":"n","type":"text/markdown","text":"hi","group":"g"}]}"#,
        )
        .unwrap();
        let out = dir.join("session.mnf");
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&args(&["--import-json", input.to_str().unwrap(), "--out", out.to_str().unwrap()])), Some(0));
        assert_eq!(run(&args(&["--check", out.to_str().unwrap()])), Some(0));
        let doc = Journal::read(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(doc.live().count(), 2);
        assert_eq!(
            run(&args(&["--import-json", input.to_str().unwrap(), "--out", out.to_str().unwrap()])),
            Some(2),
            "never overwrites"
        );
        assert_eq!(run(&args(&["--headless"])), None);
    }

    #[test]
    fn check_does_not_call_a_version_one_file_clean() {
        let dir = crate::store::testutil::temp_dir("cli-v1");
        let path = dir.join("session.mnf");
        crate::store::testutil::version_one_file(&path);
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_ne!(run(&args(&["--check", path.to_str().unwrap()])), Some(0));
    }
}
