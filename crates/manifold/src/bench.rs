//! Timing helpers and fixtures shared by the `#[ignore]` timing tests (wave 2 "Measuring"):
//! `cargo test --release -j 8 -p manifold --bins <name> -- --ignored --nocapture`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::node::{
    DownloadPayload, ImagePayload, JsonPayload, NotePayload, PdfPayload, Runtime, TabPayload, UnloadedState,
    UnloadedTabPayload,
};
use crate::process_manager::{EngineKind, EngineTab};
use crate::store::model::Kind;
use crate::App;

/// The best and the median of `times`.
pub(crate) fn best_median(mut times: Vec<Duration>) -> (Duration, Duration) {
    times.sort();
    (times[0], times[times.len() / 2])
}

/// Runs `f` `n` times: the best and the median.
pub(crate) fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    best_median(
        (0..n)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect(),
    )
}

pub(crate) fn report(what: &str, (best, median): (Duration, Duration)) {
    println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
}

/// Spreads each level's cards on a grid, 30 to a row, 300 by 200 apart, so a settle starts from distinct positions.
pub(crate) fn spread(f: &mut Fixture) {
    let mut next: HashMap<Option<String>, usize> = HashMap::new();
    for id in f.groups.iter().chain(&f.nodes) {
        let k = next.entry(f.app.mirror.group(id).map(str::to_owned)).or_default();
        let pos = egui::vec2((*k % 30) as f32 * 300.0, (*k / 30) as f32 * 200.0);
        *k += 1;
        f.app.snap_layout(id, pos);
    }
}

/// `level_fixture`'s app, with its ids.
pub(crate) struct Fixture {
    pub(crate) app: App,
    /// The eight groups, in creation order.
    pub(crate) groups: Vec<String>,
    /// The 2000 nodes, in creation order.
    pub(crate) nodes: Vec<String>,
    /// The live tabs' peers, kept so their sockets stay open.
    _peers: Vec<ipc_protocol::MessageChannel<std::net::TcpStream>>,
}

/// 2000 nodes of every kind, with store ids (`alloc_node_id`): every third at the top level, the rest spread over
/// eight groups. Each node's kind cycles through the constructible payloads. With `live_tabs`, every 280th (8 of
/// 2000) is a live tab (`EngineTab::fake`); without, those are sleeping tabs too.
pub(crate) fn level_fixture(live_tabs: bool) -> Fixture {
    let mut app = App::default();
    let mut peers = Vec::new();
    let groups: Vec<String> = (0..8)
        .map(|_| {
            let id = app.alloc_node_id();
            app.test_node(&id, Kind::Group, Runtime::None);
            id
        })
        .collect();
    let mut nodes = Vec::new();
    // `usize` spelled out: a method call (`is_multiple_of`) on a still-ambiguous integer is error E0689.
    for i in 0..2000usize {
        let id = app.alloc_node_id();
        let (kind, payload) = match i % 7 {
            0 if live_tabs && i.is_multiple_of(40) => {
                let (tab, peer) = EngineTab::fake(EngineKind::Servo);
                peers.push(peer);
                let live = TabPayload {
                    tab,
                    url: "https://a/".into(),
                    title: String::new(),
                    can_go_back: false,
                    can_go_forward: false,
                    loading: false,
                    audio_playing: false,
                    muted: false,
                    zoom_factor: 1.0,
                    swap_target: None,
                };
                (Kind::Tab, Runtime::Tab(live))
            }
            0 => {
                let sleeping = UnloadedTabPayload {
                    url: "https://s/".into(),
                    title: String::new(),
                    muted: false,
                    zoom_factor: 1.0,
                    state: UnloadedState::Sleeping,
                };
                (Kind::Tab, Runtime::Unloaded(sleeping))
            }
            1 => {
                let download = DownloadPayload {
                    path: "f.bin".into(),
                    bytes_received: 0,
                    total_bytes: None,
                    completed: true,
                    failed: false,
                };
                (Kind::Download, Runtime::Download(download))
            }
            2 => (Kind::Note, Runtime::Note(NotePayload { text: "t".into(), editing: false })),
            3 => (Kind::Image, Runtime::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None })),
            4 => (Kind::Pdf, Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None })),
            5 => {
                let json = JsonPayload { path: "a.json".into(), value: Default::default(), leaf: false, error: None };
                (Kind::Json, Runtime::Json(json))
            }
            _ => (Kind::Group, Runtime::None),
        };
        app.test_node(&id, kind, payload);
        if !i.is_multiple_of(3) {
            app.edit_group(&id, Some(groups[i % 8].clone()));
        }
        nodes.push(id);
    }
    Fixture { app, groups, nodes, _peers: peers }
}
