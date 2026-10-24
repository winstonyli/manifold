//! `gate arrange`: the six notes of `suggest-groups` (three about cats and three about rust) start in a row, cat and
//! rust alternating. "Arrange by topic" (`ARRANGE`) puts the cats nearer each other than to the rust notes (stored
//! positions in `EXPORTSESSION`; the screen ones move with the camera), and one undo (`UNDOKEY`) puts every note back.
//! No page loads, so no engine is needed. Build first: `cargo build -p manifold`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, import_session, quit, setup};

const NOTE: &str = "text/markdown";
/// In position order along the row: cat, rust, cat, rust, cat, rust.
const TEXTS: [&str; 6] = [
    "cat milk fur purr cat",
    "rust cargo crate compile rust",
    "cat fur kitten milk",
    "cargo crate rust build",
    "kitten purr milk cat fur",
    "compile build crate rust cargo",
];

/// The stored position of each note, in import order (a note at the origin has no `pos`).
fn positions(session: &Value) -> Vec<[f64; 2]> {
    let nodes = session["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    nodes.iter().map(|n| [n["pos"][0].as_f64().unwrap_or(0.0), n["pos"][1].as_f64().unwrap_or(0.0)]).collect()
}

fn mean_distance(spots: &[[f64; 2]], pairs: &[(usize, usize)]) -> f64 {
    let d =
        |&(a, b): &(usize, usize)| ((spots[a][0] - spots[b][0]).powi(2) + (spots[a][1] - spots[b][1]).powi(2)).sqrt();
    pairs.iter().map(d).sum::<f64>() / pairs.len() as f64
}

pub(crate) fn arrange(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "arrange") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let _t = Section::start("arrange");
    guarded("arrange", || {
        let profile = work.join("appdata-arrange");
        std::fs::create_dir_all(&profile)?;
        let nodes: Vec<_> = TEXTS
            .iter()
            .enumerate()
            .map(|(i, text)| json!({"id": format!("n{i}"), "type": NOTE, "text": text, "pos": [i as f64 * 300.0, 0.0]}))
            .collect();
        import_session(&exe, &profile, &nodes)?;
        let mut inst = Instance::launch(&exe, &profile, &work, "arrange.log")?;
        inst.send("WAITIDLE")?;
        let before = positions(&inst.export_session("before")?);
        inst.send("ARRANGE")?;
        inst.send("WAITIDLE")?;
        let after = positions(&inst.export_session("arranged")?);
        inst.send("UNDOKEY")?;
        inst.send("WAITIDLE")?;
        let undone = positions(&inst.export_session("undone")?);
        quit(&mut inst, "arrange");
        let within: Vec<(usize, usize)> = vec![(0, 2), (0, 4), (2, 4), (1, 3), (1, 5), (3, 5)];
        let across: Vec<(usize, usize)> = [0, 2, 4].iter().flat_map(|&c| [1, 3, 5].map(move |r| (c, r))).collect();
        let (w0, a0) = (mean_distance(&before, &within), mean_distance(&before, &across));
        let (w1, a1) = (mean_distance(&after, &within), mean_distance(&after, &across));
        check(&format!("the row starts interleaved (within {w0:.0}, across {a0:.0})"), w0 > a0);
        check(&format!("arranging puts same-topic notes closer (within {w1:.0}, across {a1:.0})"), w1 < 0.6 * a1);
        check(&format!("one undo puts every note back ({undone:?})"), undone == before);
        Ok(())
    });
    finish(&work)
}
