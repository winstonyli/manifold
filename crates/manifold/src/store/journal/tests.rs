use super::*;
use crate::store::model::{encode_field, field, FieldValue, Kind, Node, RegKey, FIELDS};
use crate::store::testutil::{file_with_version, shown, temp_dir, XorShift};
use crate::store::tree::{Leaf, Seg, TreePath};

fn create_tab(txn: &mut Txn, url: &str) -> (NodeId, OpBody) {
    let node = txn.next_id();
    let fields = encode_field(field::URL, &FieldValue::Str(url.into()));
    (node, OpBody::create(node, Kind::Tab.type_str().unwrap(), fields))
}

fn url(doc: &Doc, id: NodeId) -> FieldValue {
    doc.nodes[&id].get(field::URL)
}

#[test]
fn frames_round_trip_and_damage_stops_the_scan() {
    let mut bytes = Vec::new();
    encode_frame(&mut bytes, frame::SNAPSHOT, b"one");
    encode_frame(&mut bytes, frame::OPS, b"");
    let scan1 = scan(&bytes, 0);
    assert_eq!(scan1.frames.len(), 2);
    assert_eq!((scan1.frames[0].ty, scan1.frames[0].message), (frame::SNAPSHOT, &b"one"[..]));
    assert_eq!((scan1.end, scan1.stop), (bytes.len(), None));

    let mut flipped = bytes.clone();
    flipped[9] ^= 1;
    assert_eq!(scan(&flipped, 0).stop, Some(Stop::BadCrc));
    assert_eq!(scan(&[0u8; 64], 0).stop, Some(Stop::ZeroLength));
    assert!(scan(&[0u8; 64], 0).frames.is_empty());
    let mut zero_type = Vec::new();
    encode_frame(&mut zero_type, 0, b"x");
    assert_eq!(scan(&zero_type, 0).stop, Some(Stop::ZeroType));
    assert_eq!(scan(&bytes[..bytes.len() - 1], 0).stop, Some(Stop::Overrun));
}

#[test]
fn a_cut_at_any_byte_keeps_exactly_the_whole_frames_before_it() {
    let dir = temp_dir("torn");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    for i in 0..5 {
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, &format!("https://example.com/{i}"));
        txn.commit(vec![create]).unwrap();
    }
    let full = std::fs::read(&path).unwrap();
    let header = read_header(&full).unwrap();
    let whole = scan(&full[header.len..], header.len).frames;
    for cut in header.len..=full.len() {
        let got = scan(&full[header.len..cut], header.len);
        let expected = whole.iter().filter(|f| f.offset + f.size <= cut).count();
        assert_eq!(got.frames.len(), expected, "cut at {cut}");
        // Each commit is one ops frame creating one tab.
        let whole_ops = whole.iter().filter(|f| f.ty == frame::OPS && f.offset + f.size <= cut).count();
        let doc = Journal::read(&full[..cut]).unwrap();
        assert_eq!(doc.live().count(), whole_ops, "cut at {cut}");
    }
}

#[test]
fn reopening_gives_back_the_document() {
    let dir = temp_dir("reopen");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://a.example/");
    txn.commit(vec![create]).unwrap();
    let doc_id = journal.doc().id;
    drop(journal);

    let mut reopened = Journal::open(&path, ActorId(2)).unwrap();
    assert_eq!(reopened.doc().id, doc_id);
    assert_eq!(url(reopened.doc(), tab), FieldValue::Str("https://a.example/".into()));
    // The new replica's ops are stamped after everything in the file.
    let seen = reopened.doc().clock;
    let txn = reopened.begin().unwrap();
    txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
    assert!(reopened.doc().clock > seen);
}

#[test]
fn a_torn_tail_is_cut_before_the_next_append() {
    let dir = temp_dir("cut");
    let path = dir.join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let clean_len = std::fs::metadata(&path).unwrap().len();
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&[7, 0, 0, 0, 1, 2]).unwrap(); // half a frame
    drop(file);

    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    assert!(journal.take_recovery().is_none()); // a torn tail is no damage
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://b.example/");
    txn.commit(vec![create]).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let header = read_header(&bytes).unwrap();
    let scanned = scan(&bytes[header.len..], header.len);
    assert_eq!((scanned.end, scanned.stop), (bytes.len(), None));
    // The new frames start where the half frame did: it was cut.
    assert!(scanned.frames.iter().any(|f| f.offset as u64 == clean_len));
    let doc = Journal::read(&bytes).unwrap();
    assert!(doc.nodes[&tab].is_live());
}

#[test]
fn the_tail_read_skips_everything_before_end() {
    let dir = temp_dir("tail");
    let path = dir.join("session.mnf");
    let j = Journal::open(&path, ActorId(1)).unwrap();
    let len = std::fs::metadata(&path).unwrap().len();
    let (header, file_len, tail) = read_tail(&path, j.end).unwrap();
    assert_eq!(header.generation, j.generation);
    assert_eq!(file_len, len);
    assert_eq!(tail.len() as u64, len - j.end);
    let (_, _, past) = read_tail(&path, len + 10).unwrap();
    assert!(past.is_empty(), "a file shorter than `end` gives no tail; catch_up reloads it");
}

#[test]
fn a_held_lock_times_out_with_a_busy_error() {
    let dir = temp_dir("lock_wait");
    let path = dir.join("session.mnf");
    let held = lock_file(&path).unwrap();
    let started = Instant::now();
    match lock_file_within(&path, Duration::from_millis(100)) {
        Err(JournalError::Io(e)) => {
            assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
            assert!(e.to_string().contains("busy"), "{e}");
        }
        Err(other) => panic!("expected a busy error, got {other}"),
        Ok(_) => panic!("a second handle got a held lock"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(held);
    assert!(lock_file_within(&path, Duration::from_millis(100)).is_ok(), "free once released");
}

#[test]
fn resync_finds_a_frame_after_noise() {
    let mut rng = XorShift(3);
    let mut buf: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();
    let at = buf.len();
    encode_frame(&mut buf, 1, b"after the noise");
    assert_eq!(resync(&buf, 0), Some(at));
}

#[test]
fn two_replicas_on_one_file_merge_each_others_ops() {
    let dir = temp_dir("two");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();

    let mut txn = a.begin().unwrap();
    let group = txn.next_id();
    txn.commit(vec![OpBody::create(group, Kind::Group.type_str().unwrap(), Vec::new())]).unwrap();
    let mut txn = b.begin().unwrap();
    assert!(txn.remote.contains(&group));
    let (tab, create) = create_tab(&mut txn, "https://c.example/");
    txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }]).unwrap();

    assert!(!b.changed_on_disk() && a.changed_on_disk());
    let txn = a.begin().unwrap();
    assert!(txn.remote.contains(&tab));
    drop(txn);
    assert_eq!(a.doc().nodes, b.doc().nodes);
    assert_eq!(a.doc().nodes[&tab].group, Some(group));
}

#[test]
fn a_compaction_by_one_replica_is_picked_up_by_the_other() {
    let dir = temp_dir("compact");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();
    let mut txn = a.begin().unwrap();
    let (one, create) = create_tab(&mut txn, "https://one.example/");
    txn.commit(vec![create]).unwrap();
    a.close().unwrap(); // compacts while `b` has the file open

    let mut txn = b.begin().unwrap();
    assert!(txn.remote.contains(&one));
    let (two, create) = create_tab(&mut txn, "https://two.example/");
    txn.commit(vec![create]).unwrap();

    let reopened = Journal::open(&path, ActorId(3)).unwrap();
    assert!(reopened.doc().nodes[&one].is_live() && reopened.doc().nodes[&two].is_live());
}

#[test]
fn enough_appends_compact_and_nothing_is_lost() {
    let dir = temp_dir("many");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://0.example/");
    txn.commit(vec![create]).unwrap();
    let long = "x".repeat(1000);
    for i in 0..200 {
        let text = FieldValue::Str(format!("{i}{long}"));
        let txn = journal.begin().unwrap();
        txn.commit(vec![OpBody::set(tab, field::URL, encode_field(field::URL, &text))]).unwrap();
    }
    // 200 KB of appends against a tiny snapshot: compacted at least once.
    assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(url(&doc, tab), FieldValue::Str(format!("199{long}")));
}

#[test]
fn a_failed_compaction_does_not_fail_the_commit() {
    let dir = temp_dir("compact-fail");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    crate::store::atomic::block_for_test(&path, true); // the rewrite can't replace the file
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://0.example/");
    txn.commit(vec![create]).unwrap();
    let long = "x".repeat(1000);
    for i in 0..200 {
        let text = FieldValue::Str(format!("{i}{long}"));
        let txn = journal.begin().unwrap();
        txn.commit(vec![OpBody::set(tab, field::URL, encode_field(field::URL, &text))]).unwrap();
    }
    assert!(std::fs::metadata(&path).unwrap().len() > 150 * 1024); // never compacted
    assert!(journal.close().is_err());
    crate::store::atomic::block_for_test(&path, false);
    journal.close().unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(url(&doc, tab), FieldValue::Str(format!("199{long}")));
}

#[test]
fn a_newer_builds_frames_and_fields_survive() {
    let dir = temp_dir("newer");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://d.example/");
    let mut future = Writer::new();
    future.str(99, "from the future");
    let future = future.into_bytes();
    txn.commit(vec![create, OpBody::set(tab, 99, future.clone())]).unwrap();
    journal.close().unwrap(); // compacts: the unknown field is written back

    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc.nodes[&tab].fields[&RegKey::of(99)].enc, future);

    // A frame type from the future: kept, and the file never compacts.
    let mut frame = Vec::new();
    encode_frame(&mut frame, 50, b"new");
    OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    let txn = journal.begin().unwrap();
    txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
    journal.close().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let header = read_header(&bytes).unwrap();
    assert!(scan(&bytes[header.len..], header.len).frames.iter().any(|f| f.ty == 50));
}

#[test]
fn a_type_this_build_does_not_know_survives_snapshots_and_compaction() {
    let dir = temp_dir("future-type");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let node = txn.next_id();
    let ty = encode_field(field::TYPE, &FieldValue::Str("application/x-from-the-future".into()));
    let mut fields = ty.clone();
    fields.extend(encode_field(field::URL, &FieldValue::Str("https://f.example/".into())));
    txn.commit(vec![OpBody::Create { node, fields }]).unwrap();
    let doc = journal.doc().clone();
    let mut bytes = header_bytes(1, 0);
    bytes.extend(snapshot_bytes(&doc));
    assert_eq!(Journal::read(&bytes).unwrap().nodes, doc.nodes, "a snapshot reads back equal");
    journal.close().unwrap(); // compacts

    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc.nodes[&node].fields[&RegKey::of(field::TYPE)].enc, ty, "the raw `type` register is kept");
    assert_eq!(doc.nodes[&node].kind(), Kind::Unknown);
    assert!(doc.nodes[&node].is_live(), "a type this build doesn't know is a card");
}

/// The retired JSON leaf, written as an older build did (type string, `key` 12, `value` 13, group `from_json` 9):
/// a document that holds them reads as unknown (a card) and keeps every byte through a snapshot and compaction.
#[test]
fn a_retired_json_leaf_and_its_group_flag_survive_compaction() {
    let dir = temp_dir("retired-leaf");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (group, leaf) = (txn.next_id(), txn.next_id());
    let mut group_fields = encode_field(field::TYPE, &FieldValue::Str("application/vnd.manifold.group".into()));
    group_fields.extend(encode_field(9, &FieldValue::Bool(true)));
    let mut leaf_fields = encode_field(field::TYPE, &FieldValue::Str("application/vnd.manifold.json-leaf".into()));
    leaf_fields.extend(encode_field(field::PARENT, &FieldValue::Id(Some(group))));
    leaf_fields.extend(encode_field(12, &FieldValue::Str("k".into())));
    leaf_fields.extend(encode_field(13, &FieldValue::Str("[1,2]".into())));
    txn.commit(vec![
        OpBody::Create { node: group, fields: group_fields },
        OpBody::Create { node: leaf, fields: leaf_fields },
    ])
    .unwrap();
    let before = journal.doc().clone();
    journal.close().unwrap();

    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc.nodes, before.nodes, "every field, known or not, is kept");
    assert_eq!(doc.nodes[&leaf].kind(), Kind::Unknown);
    assert!(doc.nodes[&leaf].is_live(), "an unknown type is a card now");
    assert_eq!(doc.nodes[&group].kind(), Kind::Group);
    assert!(doc.nodes[&group].is_live());
}

#[test]
fn a_create_op_carries_its_type_in_its_fields_and_no_kind_slot() {
    let node = NodeId { actor: ActorId(1), counter: 3 };
    let fields = encode_field(field::URL, &FieldValue::Str("https://c.example/".into()));
    let op = Op {
        id: NodeId { actor: ActorId(1), counter: 4 },
        hlc: Hlc { ms: 10, logical: 0 },
        body: OpBody::create(node, Kind::Tab.type_str().unwrap(), fields),
    };
    let message = ops_message(std::slice::from_ref(&op));
    assert_eq!(decode_ops(&message).unwrap(), vec![op]);
    let op_bytes = Reader::new(&message).map(Result::unwrap).find(|f| f.no == 2).unwrap().value.bytes().unwrap();
    let body = Reader::new(op_bytes).map(Result::unwrap).find(|f| f.no == OP_CREATE).unwrap().value.bytes().unwrap();
    let slots: Vec<u32> = Reader::new(body).map(|f| f.unwrap().no).collect();
    assert_eq!(slots, [1, 3], "node and fields only");
}

#[test]
fn a_version_one_file_is_refused_and_left_alone() {
    let dir = temp_dir("version-one");
    let path = dir.join("session.mnf");
    let bytes = file_with_version(&path, 1);
    assert!(!read_header(&bytes).unwrap().damaged);
    assert!(matches!(Journal::read(&bytes), Err(JournalError::NotAJournal)));
    assert!(matches!(Journal::open(&path, ActorId(2)), Err(JournalError::NotAJournal)));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_version_one_file_is_set_aside_and_a_fresh_session_starts() {
    let dir = temp_dir("version-one-aside");
    let path = dir.join("session.mnf");
    let bytes = file_with_version(&path, 1);
    let (mut journal, aside) = Journal::open_or_set_aside(&path, ActorId(2)).unwrap();
    let aside = aside.expect("moved aside");
    assert_eq!(std::fs::read(&aside).unwrap(), bytes, "kept as it was");
    assert!(aside.file_name().unwrap().to_string_lossy().starts_with("session.mnf.unreadable-"));
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://v.example/");
    txn.commit(vec![create]).unwrap();
    drop(journal);
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(doc.nodes[&tab].is_live(), "the fresh session saves");
}

#[test]
fn a_newer_builds_structural_fields_block_compaction() {
    let dir = temp_dir("structural");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://e.example/");
    txn.commit(vec![create]).unwrap();
    drop(journal);

    // An ops frame whose top level carries a field this build doesn't know.
    let op = Op {
        id: NodeId { actor: ActorId(1), counter: 100 },
        hlc: Hlc { ms: now_ms() + 1000, logical: 0 },
        body: OpBody::Delete { node: tab },
    };
    let mut message = ops_message(&[op]);
    let mut extra = Writer::new();
    extra.varint(9, 1);
    message.extend(extra.into_bytes());
    let mut frame = Vec::new();
    encode_frame(&mut frame, frame::OPS, &message);
    OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    assert!(journal.doc().nodes[&tab].deleted);
    journal.close().unwrap();
    assert!(std::fs::read(&path).unwrap().ends_with(&frame));

    // A snapshot with the reserved Move-log tail (field 4) filled in.
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    let mut bytes = header_bytes(7, 0);
    let mut snapshot = snapshot_message(&doc);
    let mut tail = Writer::new();
    tail.bytes(4, b"tail");
    snapshot.extend(tail.into_bytes());
    encode_frame(&mut bytes, frame::SNAPSHOT, &snapshot);
    std::fs::write(&path, &bytes).unwrap();
    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    journal.close().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_failed_commit_rereads_the_file_and_keeps_it_readable() {
    let dir = temp_dir("failed");
    let path = dir.join("session.mnf");
    let set_readonly = |readonly: bool| {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(readonly);
        std::fs::set_permissions(&path, perms).unwrap();
    };
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    set_readonly(true);
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://lost.example/");
    assert!(txn.commit(vec![create]).is_err()); // the file is read-only
    assert!(matches!(journal.save_copy(&dir.join("copy.mnf"), None), Err(JournalError::Stale)));
    set_readonly(false);
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://kept.example/");
    txn.commit(vec![create]).unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(doc.nodes.len(), 1);
    assert_eq!(url(&doc, tab), FieldValue::Str("https://kept.example/".into()));
    assert_eq!(journal.doc().nodes, doc.nodes);
}

#[test]
fn compaction_prunes_and_other_replicas_hear_of_it() {
    let dir = temp_dir("prune");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();
    let mut c = Journal::open(&path, ActorId(3)).unwrap();
    let mut txn = a.begin().unwrap();
    let (gone, create_gone) = create_tab(&mut txn, "https://gone.example/");
    let (kept, create_kept) = create_tab(&mut txn, "https://kept.example/");
    txn.commit(vec![create_gone, create_kept]).unwrap();
    drop(c.begin().unwrap()); // c holds `gone` live
    let txn = a.begin().unwrap();
    txn.commit(vec![OpBody::Delete { node: gone }]).unwrap();
    drop(b.begin().unwrap()); // b holds the tombstone
    a.close().unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(!doc.nodes.contains_key(&gone) && doc.nodes[&kept].is_live());
    let txn = c.begin().unwrap();
    assert_eq!(txn.remote, vec![gone]); // missing from the document: deleted
    assert!(!txn.doc().nodes.contains_key(&gone));
    drop(txn);
    // b saw the delete already: the prune shows it nothing new.
    let txn = b.begin().unwrap();
    assert_eq!(txn.remote, vec![]);
    assert!(!txn.doc().nodes.contains_key(&gone));
}

/// A session file holding every frame type, op kind and field type:
/// the seed `damaged_files_never_panic` mutates.
fn rich_file(path: &Path) -> Vec<u8> {
    let mut journal = Journal::open(path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let group = txn.next_id();
    let (tab, create) = create_tab(&mut txn, "https://a.example/");
    let note = txn.next_id();
    let mut note_fields = encode_field(field::POS, &FieldValue::Pos(1.5, -2.0));
    note_fields.extend(encode_field(field::COLOR, &FieldValue::Color(Some(0x12ab34))));
    note_fields.extend(encode_field(field::GROUP, &FieldValue::Id(Some(group))));
    note_fields.extend(encode_field(field::TEXT, &FieldValue::Str("hi".into())));
    let set = |node, no, value| OpBody::set(node, no, encode_field(no, &value));
    let bodies = vec![
        OpBody::create(group, Kind::Group.type_str().unwrap(), encode_field(field::NAME, &FieldValue::Str("g".into()))),
        create,
        OpBody::create(note, Kind::Note.type_str().unwrap(), note_fields),
        set(tab, field::VIEW_KIND, FieldValue::ViewCode(2)),
        set(tab, field::PINNED, FieldValue::Bool(true)),
        OpBody::Move { node: tab, group: Some(group) },
    ];
    txn.commit(bodies).unwrap();
    journal.close().unwrap(); // a snapshot

    let mut journal = Journal::open(path, ActorId(2)).unwrap();
    let mut txn = journal.begin().unwrap();
    let leaf = txn.next_id();
    let mut leaf_fields = encode_field(field::PARENT, &FieldValue::Id(Some(tab)));
    leaf_fields.extend(encode_field(field::TEXT, &FieldValue::Str("v".into())));
    let bodies = vec![
        OpBody::create(leaf, Kind::Note.type_str().unwrap(), leaf_fields),
        OpBody::Delete { node: group },
        OpBody::Move { node: note, group: None },
        set(note, field::COLOR, FieldValue::Color(None)),
    ];
    txn.commit(bodies).unwrap();
    drop(journal);
    std::fs::read(path).unwrap()
}

/// Damages one frame's message (keeping its CRC valid, so the damage
/// reaches the decoders), and sometimes the file itself: a bit, a cut, a
/// zeroed or random run, or a run copied elsewhere.
fn damage(rng: &mut XorShift, bytes: &[u8]) -> Vec<u8> {
    let header = read_header(bytes).unwrap();
    let frames = scan(&bytes[header.len..], header.len).frames;
    let target = rng.below(frames.len() as u64) as usize;
    let mut out = bytes[..header.len].to_vec();
    for (i, frame) in frames.iter().enumerate() {
        let (mut ty, mut message) = (frame.ty, frame.message.to_vec());
        for _ in 0..if i == target { 1 + rng.below(4) } else { 0 } {
            let at = rng.below(message.len() as u64 + 1) as usize;
            match rng.below(6) {
                0 if at < message.len() => message[at] ^= 1 << rng.below(8),
                1 if at < message.len() => message[at] = rng.next() as u8,
                2 => message.insert(at, rng.next() as u8),
                3 if at < message.len() => drop(message.remove(at)),
                4 => message.truncate(at),
                5 => ty = 1 + rng.below(4),
                _ => {}
            }
        }
        encode_frame(&mut out, ty, &message);
    }
    let run = |rng: &mut XorShift, len: usize, max: u64| {
        let at = rng.below(len as u64) as usize;
        (at, (at + 1 + rng.below(max) as usize).min(len))
    };
    match rng.below(12) {
        0 => {
            let at = rng.below(out.len() as u64) as usize;
            out[at] ^= 1 << rng.below(8);
        }
        1 => out.truncate(rng.below(out.len() as u64) as usize),
        2 => {
            // Zeros, as from writes persisted out of order.
            let (at, end) = run(rng, out.len(), 64);
            out[at..end].fill(0);
        }
        3 => {
            let (at, end) = run(rng, out.len(), 64);
            for byte in &mut out[at..end] {
                *byte = rng.next() as u8;
            }
        }
        4 => {
            // A run copied elsewhere, as from a misdirected write: it may
            // duplicate whole frames, snapshots included.
            let (from, end) = run(rng, out.len(), 256);
            let copied = out[from..end].to_vec();
            let at = rng.below(out.len() as u64 + 1) as usize;
            out.splice(at..at, copied);
        }
        _ => {}
    }
    out
}

/// Removes what opening `path` may leave beside it (`.prev`,
/// `.damaged-*`, `.local`, temp files), but not the lock.
fn clear_siblings(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
        let other = entry.file_name().to_string_lossy().into_owned();
        if other.starts_with(&format!("{name}.")) && !other.ends_with(".lock") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Everything a reader of `bytes` might do. Errors are fine; panics
/// are not, and what this build reads it can write and read back.
fn use_damaged(bytes: &[u8], path: &Path, open: bool) {
    let _ = inspect(bytes);
    if let Ok(doc) = Journal::read(bytes) {
        doc.with_descendants(doc.nodes.keys().copied().collect());
        for (&id, node) in &doc.nodes {
            doc.effective_group(id);
            node.message();
            for def in &FIELDS {
                node.get(def.no);
            }
        }
        let mut again = header_bytes(1, 0);
        again.extend(snapshot_bytes(&doc));
        let back = Journal::read(&again).expect("a decoded document's snapshot reads back");
        assert_eq!((&back.nodes, &back.version), (&doc.nodes, &doc.version));
        let text = serde_json::to_string(&crate::store::json::doc_to_json(&doc)).unwrap();
        let json: crate::store::json::JsonDoc = serde_json::from_str(&text).unwrap();
        let mut counter = 0;
        let fresh = || {
            counter += 1;
            NodeId { actor: ActorId(9), counter }
        };
        crate::store::json::import(&json, fresh, crate::store::json::Trust::Own).expect("an export imports");
    }
    if open {
        std::fs::write(path, bytes).unwrap();
        if let Ok(mut journal) = Journal::open(path, ActorId(3)) {
            if let Ok(mut txn) = journal.begin() {
                let (_, create) = create_tab(&mut txn, "https://z.example/");
                let _ = txn.commit(vec![create]);
            }
            let may_compact = !journal.keep_unknown && !journal.read_only;
            if journal.close().is_ok() && may_compact {
                let mut again = Journal::open(path, ActorId(4)).expect("a repaired file opens");
                assert!(again.take_recovery().is_none(), "a compacting close left damage behind");
            }
        }
        clear_siblings(path);
    }
}

#[test]
fn damaged_files_never_panic() {
    let dir = temp_dir("damaged");
    let seed = rich_file(&dir.join("seed.mnf"));
    let path = dir.join("session.mnf");
    let mut rng = XorShift(0x5EED);
    for i in 0..10_000 {
        let bytes = damage(&mut rng, &seed);
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| use_damaged(&bytes, &path, i % 20 == 0)));
        if run.is_err() {
            let keep = dir.join(format!("crash-{i}.mnf"));
            std::fs::write(&keep, &bytes).unwrap();
            panic!("damaged file {i} panicked; kept at {}", keep.display());
        }
    }
}

#[test]
fn a_newer_format_version_opens_read_only() {
    let dir = temp_dir("version");
    let path = dir.join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4] = u8::try_from(FORMAT_VERSION + 1).unwrap(); // a newer format version
    let crc = crc32c(&bytes[..21]);
    bytes[21..25].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    assert!(journal.read_only());
    let txn = journal.begin().unwrap();
    assert!(matches!(txn.commit(vec![]), Err(JournalError::ReadOnly)));
    assert!(matches!(journal.write_local(b"x"), Err(JournalError::ReadOnly)));
    assert!(!local::path_for(&path).exists());
    journal.close().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn the_header_is_checked_and_names_the_file_kept_as_prev() {
    let dir = temp_dir("header");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let first = read_header(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((first.len, first.prev_generation, first.damaged), (25, 0, false));
    assert!(!prev_path(&path).exists());
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://h.example/");
    txn.commit(vec![create]).unwrap();
    let before = std::fs::read(&path).unwrap();
    journal.close().unwrap();
    let after = std::fs::read(&path).unwrap();
    assert_eq!(read_header(&after).unwrap().prev_generation, first.generation);
    assert_eq!(std::fs::read(prev_path(&path)).unwrap(), before);

    // Save As keeps no `.prev` and links none, dropping a stale one.
    let copy = dir.join("copy.mnf");
    std::fs::write(prev_path(&copy), b"stale").unwrap();
    journal.save_copy(&copy, None).unwrap();
    assert!(!prev_path(&copy).exists());
    assert_eq!(read_header(&std::fs::read(&copy).unwrap()).unwrap().prev_generation, 0);

    // A flipped bit anywhere after the magic is caught.
    for at in 4..25 {
        let mut flipped = after.clone();
        flipped[at] ^= 0x10;
        assert!(!matches!(read_header(&flipped), Ok(h) if !h.damaged), "byte {at}");
    }
}

#[test]
fn not_a_journal_is_refused_and_left_alone() {
    let dir = temp_dir("json");
    let path = dir.join("session.mnf");
    std::fs::write(&path, b"{\"tabs\": []}").unwrap();
    assert!(matches!(Journal::open(&path, ActorId(1)), Err(JournalError::NotAJournal)));
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"tabs\": []}");
}

#[test]
fn a_file_that_isnt_a_journal_is_set_aside_and_replaced() {
    let dir = temp_dir("not-a-journal");
    let path = dir.join("session.mnf");
    std::fs::write(&path, b"this is not a session").unwrap();
    assert!(matches!(Journal::open(&path, ActorId(1)), Err(JournalError::NotAJournal)));
    let (journal, aside) = Journal::open_or_set_aside(&path, ActorId(1)).unwrap();
    let aside = aside.expect("moved aside");
    assert_eq!(std::fs::read(&aside).unwrap(), b"this is not a session");
    assert!(aside.file_name().unwrap().to_string_lossy().starts_with("session.mnf.unreadable-"));
    drop(journal);
    assert!(Journal::open(&path, ActorId(2)).is_ok(), "a real journal now");
}

/// Review fix: the set-aside file's `.prev` (its best recovery copy) and
/// `.local` go with it, so the new journal's first compaction can't
/// replace them.
#[test]
fn setting_a_file_aside_takes_its_prev_and_local_along() {
    let dir = temp_dir("aside-prev");
    let path = dir.join("session.mnf");
    let old = dir.join("old.mnf");
    let mut journal = Journal::open(&old, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://old.example/");
    txn.commit(vec![create]).unwrap();
    drop(journal);
    let prev_bytes = std::fs::read(&old).unwrap();
    std::fs::write(prev_path(&path), &prev_bytes).unwrap();
    std::fs::write(local::path_for(&path), b"old local").unwrap();
    std::fs::write(&path, b"this is not a session").unwrap();

    let (mut journal, aside) = Journal::open_or_set_aside(&path, ActorId(2)).unwrap();
    let aside = aside.expect("moved aside");
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://new.example/");
    txn.commit(vec![create]).unwrap();
    let txn = journal.begin().unwrap();
    txn.journal.compact().unwrap();
    drop(txn);

    assert_eq!(std::fs::read(prev_path(&aside)).unwrap(), prev_bytes, "the old .prev survives beside the aside");
    assert_eq!(std::fs::read(local::path_for(&aside)).unwrap(), b"old local");
    assert_ne!(std::fs::read(prev_path(&path)).unwrap(), prev_bytes, "the new journal has its own .prev");
}

/// Review fix: the re-check under the lock. A file that is a journal by
/// the time the lock is held (another instance set the bad one aside
/// first) stays where it is.
#[test]
fn a_file_that_is_a_journal_under_the_lock_is_not_set_aside() {
    let dir = temp_dir("aside-recheck");
    let path = dir.join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(set_aside_unless_journal(&path).unwrap(), None);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "only the file and its lock: {dir:?}");
    std::fs::write(&path, b"junk").unwrap();
    assert!(set_aside_unless_journal(&path).unwrap().is_some());
    assert!(!path.exists());
}

#[test]
fn every_frame_reads_on_its_own() {
    // Each frame carries its own actor table, author first, so any
    // frame decodes without the frames before it.
    let dir = temp_dir("self-contained");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();
    let mut txn = a.begin().unwrap();
    let group = txn.next_id();
    txn.commit(vec![OpBody::create(group, Kind::Group.type_str().unwrap(), Vec::new())]).unwrap();
    let mut txn = b.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://f.example/");
    txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }]).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    let header = read_header(&bytes).unwrap();
    let frames = scan(&bytes[header.len..], header.len).frames;
    assert_eq!(frames.iter().map(|f| f.ty).collect::<Vec<_>>(), [frame::SNAPSHOT, frame::OPS, frame::OPS]);
    assert!(decode_snapshot(frames[0].message).is_ok());
    let theirs = frames[2].message;
    assert_eq!(read_table(theirs, 1).unwrap().list, [ActorId(2), ActorId(1)]);
    let ops = decode_ops(theirs).unwrap();
    assert!(ops.iter().all(|op| op.id.actor == ActorId(2)));
    assert!(ops.iter().any(|op| op.body == OpBody::Move { node: tab, group: Some(group) }));

    // A frame with no table has no author.
    assert!(decode_ops(&[]).is_err());
    assert!(![frame::SNAPSHOT, frame::OPS].iter().any(|ty| frame::RETIRED.contains(ty)));
}

#[test]
fn personal_state_lives_beside_the_journal() {
    let dir = temp_dir("sidecar");
    let path = dir.join("session.mnf");
    let journal = Journal::open(&path, ActorId(1)).unwrap();
    assert_eq!(journal.read_local().unwrap(), None);
    journal.write_local(b"view").unwrap();
    journal.write_local(b"view two").unwrap();
    let reopened = Journal::open(&path, ActorId(2)).unwrap();
    assert_eq!(reopened.read_local().unwrap().as_deref(), Some(&b"view two"[..]));
    let bytes = std::fs::read(&path).unwrap();
    assert!(inspect(&bytes).unwrap().frames.iter().all(|f| f.1 == frame::SNAPSHOT || f.1 == frame::OPS));
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn another_documents_personal_state_is_ignored() {
    let dir = temp_dir("sidecar-doc");
    let path = dir.join("session.mnf");
    let copy = dir.join("copy.mnf");
    let journal = Journal::open(&path, ActorId(1)).unwrap();
    journal.write_local(b"mine").unwrap();
    journal.save_copy(&copy, Some(b"copied")).unwrap();
    let copied = Journal::open(&copy, ActorId(2)).unwrap();
    assert_eq!(copied.read_local().unwrap().as_deref(), Some(&b"copied"[..]));
    // A sidecar moved beside another document isn't read.
    std::fs::copy(local::path_for(&path), local::path_for(&copy)).unwrap();
    assert_eq!(copied.read_local().unwrap(), None);
    assert_eq!(journal.read_local().unwrap().as_deref(), Some(&b"mine"[..]));
}

#[test]
fn a_damaged_or_newer_sidecar_is_not_read_and_is_replaced() {
    let dir = temp_dir("sidecar-bad");
    let path = dir.join("session.mnf");
    let side = local::path_for(&path);
    let journal = Journal::open(&path, ActorId(1)).unwrap();
    journal.write_local(b"state").unwrap();
    let clean = std::fs::read(&side).unwrap();
    let mut damaged = clean.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    std::fs::write(&side, &damaged).unwrap();
    assert!(matches!(journal.read_local(), Err(JournalError::Corrupt(_))));
    let mut newer = clean.clone();
    newer[4] = 2; // format_version 2
    std::fs::write(&side, &newer).unwrap();
    assert_eq!(journal.read_local().unwrap(), None);
    journal.write_local(b"again").unwrap();
    assert_eq!(journal.read_local().unwrap().as_deref(), Some(&b"again"[..]));
}

#[test]
fn damaged_personal_state_never_panics_or_reads_wrong() {
    let dir = temp_dir("local-damaged");
    let path = dir.join("session.mnf");
    let journal = Journal::open(&path, ActorId(1)).unwrap();
    journal.write_local(b"some personal state").unwrap();
    let clean = std::fs::read(local::path_for(&path)).unwrap();
    let mut rng = XorShift(0x10CA1);
    for _ in 0..2000 {
        let mut bytes = clean.clone();
        for _ in 0..1 + rng.below(3) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len() as u64) as usize;
            match rng.below(3) {
                0 => bytes[at] ^= 1 << rng.below(8),
                1 => bytes.truncate(at),
                _ => bytes.insert(at, rng.next() as u8),
            }
        }
        std::fs::write(local::path_for(&path), &bytes).unwrap();
        // The frame's CRC means any state read is the one written.
        if let Ok(Some(state)) = journal.read_local() {
            assert_eq!(state, b"some personal state");
        }
    }
}

#[test]
fn inspect_reports_frames_and_the_torn_tail() {
    let dir = temp_dir("inspect");
    let path = dir.join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    let clean = bytes.len();
    bytes.extend_from_slice(&[0; 16]);
    let report = inspect(&bytes).unwrap();
    assert_eq!(report.frames.iter().map(|f| f.1).collect::<Vec<_>>(), [frame::SNAPSHOT]);
    assert_eq!(report.torn, Some((clean, Stop::ZeroLength)));
}

#[test]
fn closing_without_changes_leaves_the_file_alone() {
    let dir = temp_dir("idle");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://idle.example/");
    txn.commit(vec![create]).unwrap();
    journal.close().unwrap(); // changed: compacts
    let compacted = std::fs::read(&path).unwrap();
    for actor in 2..5 {
        let mut idle = Journal::open(&path, ActorId(actor)).unwrap();
        drop(idle.begin().unwrap());
        idle.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), compacted, "launch {actor} rewrote an unchanged file");
    }
    let frames =
        |path: &Path| inspect(&std::fs::read(path).unwrap()).unwrap().frames.iter().map(|f| f.1).collect::<Vec<_>>();

    // A replica that appended and died without closing: the next close compacts.
    let mut crashed = Journal::open(&path, ActorId(5)).unwrap();
    let txn = crashed.begin().unwrap();
    txn.commit(vec![OpBody::Delete { node: tab }]).unwrap();
    drop(crashed);
    Journal::open(&path, ActorId(6)).unwrap().close().unwrap();
    assert_eq!(frames(&path), [frame::SNAPSHOT]);
    assert!(!Journal::read(&std::fs::read(&path).unwrap()).unwrap().nodes.get(&tab).is_some_and(Node::is_live));

    // Another live replica's frames count too.
    let mut idle = Journal::open(&path, ActorId(7)).unwrap();
    let mut other = Journal::open(&path, ActorId(8)).unwrap();
    let mut txn = other.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://other.example/");
    txn.commit(vec![create]).unwrap();
    idle.close().unwrap();
    assert_eq!(frames(&path), [frame::SNAPSHOT]);

    // So does a torn tail.
    OpenOptions::new().append(true).open(&path).unwrap().write_all(&[7, 0, 0, 0, 1, 2]).unwrap();
    Journal::open(&path, ActorId(9)).unwrap().close().unwrap();
    assert_eq!(inspect(&std::fs::read(&path).unwrap()).unwrap().torn, None);
}

/// Three tabs compacted into a snapshot (its `.prev` holds the file
/// before), then three ops frames of one tab each from another replica.
fn layered_file(path: &Path) -> Vec<u8> {
    let mut journal = Journal::open(path, ActorId(1)).unwrap();
    for i in 0..3 {
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, &format!("https://s{i}.example/"));
        txn.commit(vec![create]).unwrap();
    }
    journal.close().unwrap();
    let mut journal = Journal::open(path, ActorId(2)).unwrap();
    for i in 0..3 {
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, &format!("https://o{i}.example/"));
        txn.commit(vec![create]).unwrap();
    }
    drop(journal);
    std::fs::read(path).unwrap()
}

/// `(offset, size)` of each whole frame.
fn frame_spans(bytes: &[u8]) -> Vec<(usize, usize)> {
    let header = read_header(bytes).unwrap();
    scan(&bytes[header.len..], header.len).frames.iter().map(|f| (f.offset, f.size)).collect()
}

#[test]
fn a_damaged_ops_frame_loses_only_its_own_ops() {
    let dir = temp_dir("resync");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let spans = frame_spans(&bytes);
    assert_eq!(spans.len(), 4);
    for &(offset, size) in &spans[1..] {
        let mut without = bytes[..offset].to_vec();
        without.extend_from_slice(&bytes[offset + size..]);
        let expected = shown(&Journal::read(&without).unwrap());
        let last = offset + size == bytes.len();
        // A flipped bit (the frame fails its CRC), or a zeroed frame
        // (writes persisted out of order).
        for zeroed in [false, true] {
            let mut damaged = bytes.clone();
            if zeroed {
                damaged[offset..offset + size].fill(0);
            } else {
                damaged[offset + size - 1] ^= 1;
            }
            std::fs::write(&path, &damaged).unwrap();
            let mut journal = Journal::open(&path, ActorId(3)).unwrap();
            assert_eq!(shown(journal.doc()), expected, "frame at {offset}, zeroed {zeroed}");
            if zeroed && last {
                // Zeros at the end are a torn tail: an append that never landed.
                assert!(journal.take_recovery().is_none());
                continue;
            }
            assert_eq!(inspect(&damaged).unwrap().damaged, [(offset, offset + size)]);
            let recovery = journal.take_recovery().expect("damage is reported");
            assert_eq!(recovery.damaged, [(offset, offset + size)]);
            let saved = recovery.saved.expect("the file as it was is kept");
            assert_eq!(std::fs::read(&saved).unwrap(), damaged);
            std::fs::remove_file(saved).unwrap();
            assert_eq!(frame_spans(&std::fs::read(&path).unwrap()).len(), 1);
            assert!(Journal::open(&path, ActorId(4)).unwrap().take_recovery().is_none());
        }
    }
}

#[test]
fn a_lost_snapshot_is_rebuilt_from_prev() {
    let dir = temp_dir("from-prev");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let original = Journal::read(&bytes).unwrap();
    let prev = std::fs::read(prev_path(&path)).unwrap();
    let (offset, size) = frame_spans(&bytes)[0];
    let mut damaged = bytes.clone();
    damaged[offset + size - 1] ^= 1;
    std::fs::write(&path, &damaged).unwrap();
    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    assert_eq!(shown(journal.doc()), shown(&original));
    assert_eq!(journal.doc().id, original.id);
    let recovery = journal.take_recovery().unwrap();
    assert!(recovery.from_prev && !recovery.new_document);
    // A repair leaves `.prev` as it was, and still linked.
    assert_eq!(std::fs::read(prev_path(&path)).unwrap(), prev);
    let header = read_header(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(header.prev_generation, read_header(&prev).unwrap().generation);

    // Without `.prev`: a new document holding what the surviving ops made.
    std::fs::remove_file(prev_path(&path)).unwrap();
    std::fs::write(&path, &damaged).unwrap();
    let mut journal = Journal::open(&path, ActorId(5)).unwrap();
    assert!(journal.take_recovery().unwrap().new_document);
    assert_ne!(journal.doc().id, original.id);
    let urls: Vec<FieldValue> = journal.doc().live().map(|(id, _)| url(journal.doc(), id)).collect();
    assert_eq!(urls, (0..3).map(|i| FieldValue::Str(format!("https://o{i}.example/"))).collect::<Vec<_>>());
}

#[test]
fn a_prev_with_the_wrong_generation_is_not_used() {
    let dir = temp_dir("prev-mismatch");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let original = Journal::read(&bytes).unwrap();
    let (offset, size) = frame_spans(&bytes)[0];
    let mut damaged = bytes.clone();
    damaged[offset + size - 1] ^= 1;
    std::fs::write(&path, &damaged).unwrap();

    // `.prev` from another journal entirely: a different generation,
    // and its tabs are foreign to this document.
    let foreign_dir = temp_dir("prev-mismatch-foreign");
    let foreign_bytes = layered_file(&foreign_dir.join("other.mnf"));
    std::fs::write(prev_path(&path), &foreign_bytes).unwrap();

    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    let recovery = journal.take_recovery().unwrap();
    assert!(!recovery.from_prev && recovery.new_document);
    assert_ne!(journal.doc().id, original.id);
    assert_ne!(journal.doc().id, Journal::read(&foreign_bytes).unwrap().id);
    let urls: Vec<FieldValue> = journal.doc().live().map(|(id, _)| url(journal.doc(), id)).collect();
    assert_eq!(urls, (0..3).map(|i| FieldValue::Str(format!("https://o{i}.example/"))).collect::<Vec<_>>());
}

#[test]
fn damage_found_while_catching_up_is_repaired_not_cut() {
    let dir = temp_dir("live-damage");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();
    let mut txn = b.begin().unwrap();
    let (x, create_x) = create_tab(&mut txn, "https://x.example/");
    txn.commit(vec![create_x]).unwrap();
    let mut txn = b.begin().unwrap();
    let (y, create_y) = create_tab(&mut txn, "https://y.example/");
    txn.commit(vec![create_y]).unwrap();
    // X's frame is damaged on disk; Y's, after it, is whole.
    let mut bytes = std::fs::read(&path).unwrap();
    let (offset, size) = frame_spans(&bytes)[1];
    bytes[offset + size - 1] ^= 1;
    std::fs::write(&path, &bytes).unwrap();

    let mut txn = a.begin().unwrap();
    assert!(txn.remote.contains(&y));
    let (z, create_z) = create_tab(&mut txn, "https://z.example/");
    txn.commit(vec![create_z]).unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(doc.nodes[&y].is_live() && doc.nodes[&z].is_live());
    assert!(!doc.nodes.contains_key(&x));
    assert!(a.take_recovery().is_some_and(|r| r.saved.is_some()));
    // B held X only in memory: it learns X is gone (spec: accepted).
    let txn = b.begin().unwrap();
    assert!(txn.remote.contains(&x) && !txn.doc().nodes.contains_key(&x));
}

/// A stop that is not a CRC failure (here a zeroed length) with a whole frame after it is damage, found in the
/// tail `catch_up` reads: it reloads and keeps the frame, rather than cutting it as a torn tail.
#[test]
fn a_zeroed_gap_in_the_tail_is_damage_not_a_torn_tail() {
    let dir = temp_dir("tail-gap");
    let path = dir.join("session.mnf");
    let mut a = Journal::open(&path, ActorId(1)).unwrap();
    let mut b = Journal::open(&path, ActorId(2)).unwrap();
    let mut txn = b.begin().unwrap();
    let (x, create_x) = create_tab(&mut txn, "https://x.example/");
    txn.commit(vec![create_x]).unwrap();
    let mut txn = b.begin().unwrap();
    let (y, create_y) = create_tab(&mut txn, "https://y.example/");
    txn.commit(vec![create_y]).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let (y_at, _) = *frame_spans(&bytes).last().unwrap();
    bytes.splice(y_at..y_at, [0u8; 8]);
    std::fs::write(&path, &bytes).unwrap();

    let mut txn = a.begin().unwrap();
    assert!(txn.remote.contains(&x) && txn.remote.contains(&y), "{:?}", txn.remote);
    let (z, create_z) = create_tab(&mut txn, "https://z.example/");
    txn.commit(vec![create_z]).unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!([x, y, z].iter().all(|id| doc.nodes[id].is_live()));
}

#[test]
fn a_frame_that_does_not_decode_is_skipped_and_kept_aside() {
    let dir = temp_dir("undecodable");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://ok.example/");
    txn.commit(vec![create]).unwrap();
    drop(journal);
    let mut bytes = std::fs::read(&path).unwrap();
    encode_frame(&mut bytes, frame::OPS, &[]); // CRC-valid, but no actor table
    std::fs::write(&path, &bytes).unwrap();
    let mut journal = Journal::open(&path, ActorId(2)).unwrap();
    assert!(journal.doc().nodes[&tab].is_live());
    let recovery = journal.take_recovery().unwrap();
    assert_eq!((recovery.skipped_frames, recovery.damaged.len()), (1, 0));
    assert_eq!(std::fs::read(recovery.saved.unwrap()).unwrap(), bytes);
}

#[test]
fn a_damaged_header_is_rewritten() {
    let dir = temp_dir("bad-header");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let expected = shown(&Journal::read(&bytes).unwrap());
    let mut damaged = bytes.clone();
    damaged[8] ^= 1; // inside the generation
    std::fs::write(&path, &damaged).unwrap();
    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    assert_eq!(shown(journal.doc()), expected);
    let recovery = journal.take_recovery().unwrap();
    assert!(recovery.header_damaged && !recovery.from_prev);
    let saved = recovery.saved.unwrap();
    assert!(saved.to_string_lossy().ends_with(&format!(".damaged-{:016x}", crc32c(&damaged))));
    assert!(!read_header(&std::fs::read(&path).unwrap()).unwrap().damaged);
}

#[test]
fn a_failed_save_blocks_rewrite_and_truncation_until_retried() {
    let dir = temp_dir("unsaved-damage");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let (offset, size) = frame_spans(&bytes)[1]; // not the file's last frame
    let mut damaged = bytes.clone();
    damaged[offset + size - 1] ^= 1;
    std::fs::write(&path, &damaged).unwrap();

    // Make `save_damaged` fail.
    let saved = damaged_path(&path, &damaged);
    crate::store::atomic::block_for_test(&saved, true);

    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), damaged); // the repair's rewrite never ran
    let recovery = journal.take_recovery().unwrap();
    assert!(recovery.saved.is_none());

    // A commit is refused rather than truncating the unsaved original away.
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://blocked.example/");
    assert!(txn.commit(vec![create]).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);

    // Once the save can succeed, the next operation retries it and proceeds.
    crate::store::atomic::block_for_test(&saved, false);
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://ok.example/");
    txn.commit(vec![create]).unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), damaged);
    assert_ne!(std::fs::read(&path).unwrap(), damaged);
}

#[test]
fn a_clean_reload_clears_unsaved_damage() {
    let dir = temp_dir("unsaved-damage-cleared");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let (offset, size) = frame_spans(&bytes)[1]; // not the file's last frame
    let mut damaged = bytes.clone();
    damaged[offset + size - 1] ^= 1;
    std::fs::write(&path, &damaged).unwrap();

    // Block this journal's own save, so it opens with the damage
    // unsaved and never rewritten.
    let saved = damaged_path(&path, &damaged);
    crate::store::atomic::block_for_test(&saved, true);
    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
    journal.take_recovery();

    // Another replica repairs the file cleanly.
    crate::store::atomic::block_for_test(&saved, false);
    drop(Journal::open(&path, ActorId(4)).unwrap());
    assert_eq!(std::fs::read(&saved).unwrap(), damaged);
    assert_ne!(std::fs::read(&path).unwrap(), damaged);
    let count = |dir: &Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".damaged-"))
            .count()
    };
    let before = count(&dir);

    // The stale journal's next commit sees the file is now clean and
    // doesn't save a spurious, already-clean copy aside.
    let mut txn = journal.begin().unwrap();
    let (_, create) = create_tab(&mut txn, "https://ok.example/");
    txn.commit(vec![create]).unwrap();
    assert_eq!(count(&dir), before);
}

#[test]
fn take_recovery_returns_none_the_second_time() {
    let dir = temp_dir("recovery-once");
    let path = dir.join("session.mnf");
    let bytes = layered_file(&path);
    let (offset, size) = frame_spans(&bytes)[1];
    let mut damaged = bytes.clone();
    damaged[offset + size - 1] ^= 1;
    std::fs::write(&path, &damaged).unwrap();
    let mut journal = Journal::open(&path, ActorId(3)).unwrap();
    assert!(journal.take_recovery().is_some());
    assert!(journal.take_recovery().is_none());
}

// Tests with real processes. Each child is this test binary running the
// ignored `child_writer` test, told what to do through the environment.

/// Long enough that a few hundred commits pass `COMPACT_MIN`, so the
/// writers compact while the other one is running.
fn child_url(actor: u64, i: u64) -> String {
    format!("https://p{actor}.example/{i}/{}", "x".repeat(400))
}

fn spawn_writer(path: &Path, actor: u64, count: u64) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["store::journal::tests::child_writer", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("MANIFOLD_STORE_CHILD", path)
        .env("MANIFOLD_STORE_ACTOR", actor.to_string())
        .env("MANIFOLD_STORE_COUNT", count.to_string())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

fn live_urls(doc: &Doc) -> std::collections::BTreeSet<String> {
    doc.live()
        .filter_map(|(_, node)| match node.get(field::URL) {
            FieldValue::Str(url) => Some(url),
            _ => None,
        })
        .collect()
}

/// Commits one tab at a time and prints `committed <i>` after each
/// commit returns; `count` 0 means until killed. Run only as a child.
#[test]
#[ignore = "a child process of the multi-process tests"]
fn child_writer() {
    let var = |name| std::env::var(name).unwrap();
    let path = PathBuf::from(var("MANIFOLD_STORE_CHILD"));
    let actor: u64 = var("MANIFOLD_STORE_ACTOR").parse().unwrap();
    let count: u64 = var("MANIFOLD_STORE_COUNT").parse().unwrap();
    let mut journal = Journal::open(&path, ActorId(actor)).unwrap();
    let mut i = 0;
    while count == 0 || i < count {
        let mut txn = journal.begin().unwrap();
        let (_, create) = create_tab(&mut txn, &child_url(actor, i));
        txn.commit(vec![create]).unwrap();
        println!("committed {i}");
        i += 1;
    }
    journal.close().unwrap();
}

#[test]
fn two_processes_writing_one_file_lose_nothing() {
    let dir = temp_dir("processes");
    let path = dir.join("session.mnf");
    Journal::open(&path, ActorId(1)).unwrap().close().unwrap();
    let children = [spawn_writer(&path, 2, 200), spawn_writer(&path, 3, 200)];
    for child in children {
        assert!(child.wait_with_output().unwrap().status.success());
    }
    let mut journal = Journal::open(&path, ActorId(4)).unwrap();
    assert_eq!(journal.take_recovery(), None);
    let expected = (0..200).flat_map(|i| [child_url(2, i), child_url(3, i)]).collect();
    assert_eq!(live_urls(journal.doc()), expected);
    // 400 tabs of ~450 bytes: both writers compacted along the way.
    assert!(prev_path(&path).exists());
}

#[test]
fn a_writer_killed_mid_commit_loses_at_most_that_commit() {
    use std::io::BufRead;
    let dir = temp_dir("killed");
    let path = dir.join("session.mnf");
    let mut rng = XorShift(0x4B11);
    let mut committed = std::collections::BTreeSet::new();
    for round in 0..8u64 {
        let actor = 10 + round;
        let mut child = spawn_writer(&path, actor, 0);
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        // The harness prints `test child_writer ... ` without a newline, so
        // `committed 0` shares its line: parse indices rather than count lines.
        let index = |line: &str| line.rsplit_once("committed ").and_then(|(_, i)| i.parse::<u64>().ok());
        let mut reported = 0;
        let stop_after = 1 + rng.below(40);
        while reported < stop_after {
            reported = reported.max(index(&lines.next().unwrap().unwrap()).map_or(0, |i| i + 1));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        // Commits printed after `stop_after` but before the process died.
        reported = lines.map_while(Result::ok).filter_map(|l| index(&l)).fold(reported, |n, i| n.max(i + 1));
        committed.extend((0..reported).map(|i| child_url(actor, i)));

        let mut journal = Journal::open(&path, ActorId(99)).unwrap();
        let urls = live_urls(journal.doc());
        assert!(urls.is_superset(&committed), "round {round}: a reported commit is missing");
        assert!(urls.len() <= committed.len() + 1, "round {round}: more than the in-flight commit landed");
        // The in-flight commit, if it landed, stays.
        committed = urls;
        // A kill leaves at most a torn tail, never damage worth a report.
        assert_eq!(journal.take_recovery(), None, "round {round}");
        journal.close().unwrap();
    }
}

// --- Save-path costs (Plan B measures these before wiring saves) ---

/// Best and median of `n` timings of `f`.
fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    let mut times: Vec<Duration> = (0..n)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort();
    (times[0], times[n / 2])
}

fn report(what: &str, (best, median): (Duration, Duration)) {
    println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
}

/// What each save costs at realistic session sizes, with the file at
/// its largest (just below compaction): `begin` reads the header and
/// the tail past `end`, and the whole file when it reloads. Timing, not correctness, so ignored:
/// `cargo test --release -j 12 -p manifold -- --ignored --nocapture save_path_costs`
#[test]
#[ignore]
fn save_path_costs() {
    const N: usize = 40;
    for tabs in [300usize, 3000] {
        let dir = temp_dir(&format!("cost-{tabs}"));
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId::random()).unwrap();
        let mut txn = a.begin().unwrap();
        let groups: Vec<NodeId> = (0..tabs / 10).map(|_| txn.next_id()).collect();
        let mut bodies: Vec<OpBody> = groups
            .iter()
            .enumerate()
            .map(|(i, &node)| {
                let fields = encode_field(field::NAME, &FieldValue::Str(format!("Group {i}")));
                OpBody::create(node, Kind::Group.type_str().unwrap(), fields)
            })
            .collect();
        let mut ids = Vec::new();
        for i in 0..tabs {
            let node = txn.next_id();
            let url = format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
            let mut fields = encode_field(field::URL, &FieldValue::Str(url));
            fields.extend(encode_field(field::POS, &FieldValue::Pos(i as f32 * 3.5, i as f32 * -2.25)));
            fields.extend(encode_field(field::VIEW_KIND, &FieldValue::ViewCode(1 + (i % 4) as u64)));
            fields.extend(encode_field(field::GROUP, &FieldValue::Id(Some(groups[i % groups.len()]))));
            bodies.push(OpBody::create(node, Kind::Tab.type_str().unwrap(), fields));
            ids.push(node);
        }
        txn.commit(bodies).unwrap();
        let txn = a.begin().unwrap();
        txn.journal.compact().unwrap();
        drop(txn);
        let snapshot = std::fs::metadata(&path).unwrap().len();

        // Grow the tail with single-op saves (a drag end each) to just
        // below the compaction threshold: the largest file `begin` reads.
        let set_pos =
            |node: NodeId, x: f32| OpBody::set(node, field::POS, encode_field(field::POS, &FieldValue::Pos(x, x)));
        let margin = 8 * 1024;
        let mut k = 0;
        while a.since_snapshot + margin < (2 * a.snapshot_len).max(COMPACT_MIN) {
            let txn = a.begin().unwrap();
            txn.commit(vec![set_pos(ids[k % tabs], k as f32)]).unwrap();
            k += 1;
        }
        let largest = std::fs::metadata(&path).unwrap().len();
        println!("{tabs} tabs: snapshot {snapshot} B, largest file {largest} B ({k} saves in the tail)");

        report("open (startup)", timed(10, || drop(Journal::open(&path, ActorId::random()).unwrap())));
        let largest_bytes = std::fs::read(&path).unwrap();
        report("changed_on_disk, unchanged (per frame)", timed(N, || assert!(!a.changed_on_disk())));
        report("begin, nothing new (per save)", timed(N, || drop(a.begin().unwrap())));
        // Stay below compaction: ~30 B per op against an 8 KB margin.
        let mut x = 0.0;
        report(
            "begin + commit one Set, fsync debounced",
            timed(N, || {
                x += 1.0;
                a.begin().unwrap().commit(vec![set_pos(ids[0], x)]).unwrap();
            }),
        );
        let mut synced: Vec<Duration> = (0..5)
            .map(|_| {
                std::thread::sleep(SYNC_DEBOUNCE); // the next append fsyncs
                x += 1.0;
                let start = Instant::now();
                a.begin().unwrap().commit(vec![set_pos(ids[1], x)]).unwrap();
                start.elapsed()
            })
            .collect();
        synced.sort();
        report("begin + commit one Set, with fsync", (synced[0], synced[2]));

        // Another instance's edits: B appends one op, A catches up.
        let mut b = Journal::open(&path, ActorId::random()).unwrap();
        let mut incremental = Vec::new();
        let mut reload = Vec::new();
        let mut compact = Vec::new();
        for i in 0..10 {
            x += 1.0;
            b.begin().unwrap().commit(vec![set_pos(ids[2], x)]).unwrap();
            let start = Instant::now();
            let txn = a.begin().unwrap();
            incremental.push(start.elapsed());
            assert_eq!(txn.remote, vec![ids[2]], "sample {i}");
            drop(txn);

            // B compacts: A reloads the whole file.
            let txn = b.begin().unwrap();
            let start = Instant::now();
            txn.journal.compact().unwrap();
            compact.push(start.elapsed());
            drop(txn);
            let start = Instant::now();
            let txn = a.begin().unwrap();
            reload.push(start.elapsed());
            assert_eq!(txn.remote, vec![], "a compaction alone changes nothing A shows");
        }
        for (what, mut v) in [
            ("begin after another appended one op", incremental),
            ("compact (under the lock)", compact),
            ("begin after another compacted (reload)", reload),
        ] {
            v.sort();
            report(what, (v[0], v[v.len() / 2]));
        }
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        let snapshot = &scan_all(&bytes[header.len..], header.len).frames[0];
        report("  of which: decode the snapshot", timed(10, || drop(decode_snapshot(snapshot.message).unwrap())));
        let doc = decode_snapshot(snapshot.message).unwrap();
        report("  of which: re-encode it (unknown-field check)", timed(10, || drop(snapshot_message(&doc))));

        // `open`'s share: the tail it replays on top of the snapshot.
        let header = read_header(&largest_bytes).unwrap();
        let frames = scan_all(&largest_bytes[header.len..], header.len).frames;
        report("open: read the file", timed(10, || drop(std::fs::read(&path).unwrap())));
        report(
            "open: load (all but read, lock, repair)",
            timed(10, || Journal::blank(&path, ActorId(9)).load(&largest_bytes).unwrap()),
        );
        report("open: lock", timed(10, || drop(lock_file(&path).unwrap())));
        report("open: scan frames (CRC)", timed(10, || drop(scan_all(&largest_bytes[header.len..], header.len))));
        let tail = &frames[1..];
        let decoded: Vec<Vec<Op>> = tail.iter().map(|f| decode_ops(f.message).unwrap()).collect();
        report("open: decode tail ops", timed(10, || tail.iter().for_each(|f| drop(decode_ops(f.message).unwrap()))));
        report(
            "open: re-encode tail ops (unknown check)",
            timed(10, || decoded.iter().for_each(|o| drop(ops_message(o)))),
        );
        let base = decode_snapshot(frames[0].message).unwrap();
        report(
            "open: apply tail ops",
            timed(10, || {
                let mut doc = base.clone();
                decoded.iter().flatten().for_each(|op| drop(doc.apply(op)));
            }),
        );
        report("  (clone of the base doc, included above)", timed(10, || drop(base.clone())));
        println!("  tail: {} frames, {} ops", tail.len(), decoded.iter().map(Vec::len).sum::<usize>());
    }
}

fn props_path(keys: &[&str]) -> TreePath {
    TreePath::of(&keys.iter().map(|k| Seg::Key((*k).to_owned())).collect::<Vec<_>>())
}

fn set_at(node: NodeId, path: TreePath, leaf: Leaf) -> OpBody {
    OpBody::Set { node, field: field::PROPS, path, enc: leaf.encode() }
}

fn props(doc: &Doc, node: NodeId) -> Vec<(TreePath, Leaf)> {
    doc.nodes[&node]
        .fields
        .iter()
        .filter(|(k, r)| k.field == field::PROPS && !r.enc.is_empty())
        .map(|(k, r)| (k.path.clone(), Leaf::decode(&r.enc).unwrap().unwrap()))
        .collect()
}

#[test]
fn a_path_set_survives_the_log_a_snapshot_and_a_reopen() {
    let dir = temp_dir("path-set");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://p.example/");
    txn.commit(vec![
        create,
        set_at(tab, props_path(&["a"]), Leaf::Str("x".into())),
        set_at(tab, props_path(&["a", "b\0é"]), Leaf::Int(7)),
        set_at(tab, props_path(&["gone"]), Leaf::Null),
        OpBody::Set { node: tab, field: field::PROPS, path: props_path(&["gone"]), enc: Vec::new() },
    ])
    .unwrap();
    let live = Journal::read(&std::fs::read(&path).unwrap()).unwrap(); // from the log
    journal.close().unwrap(); // compacts: from a snapshot
    let snap = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    for doc in [&live, &snap] {
        assert_eq!(
            props(doc, tab),
            vec![(props_path(&["a"]), Leaf::Str("x".into())), (props_path(&["a", "b\0é"]), Leaf::Int(7))]
        );
    }
    assert_eq!(live.nodes[&tab], snap.nodes[&tab], "the tombstone at /gone is kept by the snapshot");
}

#[test]
fn a_path_on_a_known_scalar_field_is_ignored() {
    let dir = temp_dir("path-scalar");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://s.example/");
    let changed = encode_field(field::URL, &FieldValue::Str("https://changed.example/".into()));
    txn.commit(vec![create, OpBody::Set { node: tab, field: field::URL, path: props_path(&["x"]), enc: changed }])
        .unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(url(&doc, tab), FieldValue::Str("https://s.example/".into()));
    assert!(doc.nodes[&tab].fields.keys().all(|k| k.path.is_root()));
}

#[test]
fn a_delete_clears_the_tree_and_later_path_sets_are_ignored() {
    let dir = temp_dir("path-delete");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://d.example/");
    txn.commit(vec![create, set_at(tab, props_path(&["a"]), Leaf::Int(1)), OpBody::Delete { node: tab }]).unwrap();
    let txn = journal.begin().unwrap();
    txn.commit(vec![set_at(tab, props_path(&["b"]), Leaf::Int(2))]).unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap(); // the log, before compaction drops the node
    assert!(doc.nodes[&tab].deleted && doc.nodes[&tab].fields.is_empty());
    journal.close().unwrap();
}

#[test]
fn an_unknown_field_with_a_path_survives_compaction() {
    let dir = temp_dir("path-future");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://f.example/");
    let mut future = Writer::new();
    future.str(99, "from the future");
    let key = RegKey { field: 99, path: props_path(&["k"]) };
    txn.commit(vec![create, OpBody::Set { node: tab, field: 99, path: key.path.clone(), enc: future.into_bytes() }])
        .unwrap();
    journal.close().unwrap();
    let doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    assert!(doc.nodes[&tab].fields.contains_key(&key));
}

#[test]
fn a_version_three_file_is_refused_and_set_aside() {
    let dir = temp_dir("version-three");
    let path = dir.join("session.mnf");
    let bytes = file_with_version(&path, 3);
    assert!(matches!(Journal::read(&bytes), Err(JournalError::NotAJournal)));
    let (_journal, aside) = Journal::open_or_set_aside(&path, ActorId(2)).unwrap();
    assert_eq!(std::fs::read(aside.expect("moved aside")).unwrap(), bytes);
}

/// A fresh doc with `ops` applied in the given order.
fn apply_all(ops: &[Op]) -> Doc {
    let mut doc = Doc::new(DocId([0; 16]));
    for op in ops {
        doc.apply(op);
    }
    doc
}

/// What compaction writes for `doc`: pruned (hidden registers dropped), then snapshotted.
fn compacted_bytes(doc: &Doc) -> Vec<u8> {
    let mut doc = doc.clone();
    doc.forget_moves();
    doc.prune();
    snapshot_message(&doc)
}

/// A random path in the small key space the tree tests share, so writes collide and hide each other.
fn random_tree_path(rng: &mut XorShift) -> TreePath {
    let keys = ["a", "b", "c"];
    let segs: Vec<Seg> = (0..rng.below(4))
        .map(|_| match rng.below(5) {
            3 => Seg::Id(NodeId { actor: ActorId(9), counter: 1 }),
            4 => Seg::Id(NodeId { actor: ActorId(9), counter: 2 }),
            n => Seg::Key(keys[n as usize].to_owned()),
        })
        .collect();
    TreePath::of(&segs)
}

/// A `Create` of `node` and 1 to 14 PROPS writes by actors 1 to 3 (a quarter are tombstones). Their HLCs come from a
/// small range, so equal HLCs from different actors occur; no two ops share a stamp.
fn random_prop_ops(rng: &mut XorShift, node: NodeId) -> Vec<Op> {
    let mut ops = vec![Op {
        id: node,
        hlc: Hlc { ms: 1, logical: 0 },
        body: OpBody::create(node, "application/x-from-the-future", Vec::new()),
    }];
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..rng.below(14) + 1 {
        let (actor, ms, logical) = loop {
            let stamp = (1 + rng.below(3), 2 + rng.below(3), u32::try_from(rng.below(2)).unwrap());
            if seen.insert(stamp) {
                break stamp;
            }
        };
        let enc = if rng.below(4) == 0 { Vec::new() } else { Leaf::Int(rng.below(100) as i64).encode() };
        ops.push(Op {
            id: NodeId { actor: ActorId(actor), counter: 100 + i },
            hlc: Hlc { ms, logical },
            body: OpBody::Set { node, field: field::PROPS, path: random_tree_path(rng), enc },
        });
    }
    ops
}

#[test]
fn replicas_converge_whatever_the_order_and_duplicates() {
    let node = NodeId { actor: ActorId(1), counter: 0 };
    let mut rng = XorShift(0xC0DE_0001);
    for round in 0..200 {
        let ops = random_prop_ops(&mut rng, node);
        let reference = apply_all(&ops);
        for _ in 0..4 {
            let mut shuffled = ops.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let dup = shuffled[rng.below(shuffled.len() as u64) as usize].clone();
            shuffled.push(dup);
            let doc = apply_all(&shuffled);
            assert_eq!(doc.nodes, reference.nodes, "round {round}");
            assert_eq!(doc.nodes[&node].tree(field::PROPS), reference.nodes[&node].tree(field::PROPS));
            assert_eq!(compacted_bytes(&doc), compacted_bytes(&reference), "compacted bytes, round {round}");
        }
    }
}

#[test]
fn pruning_never_changes_the_tree_now_or_after_a_late_write() {
    let node = NodeId { actor: ActorId(1), counter: 0 };
    let mut rng = XorShift(0xC0DE_0002);
    for round in 0..300 {
        let mut full = apply_all(&random_prop_ops(&mut rng, node));
        let mut pruned = full.clone();
        pruned.forget_moves();
        pruned.prune();
        assert_eq!(pruned.nodes[&node].tree(field::PROPS), full.nodes[&node].tree(field::PROPS), "round {round}");
        // A replica that never saw the prune receives one more write, older or newer than any so far.
        let late = Op {
            id: NodeId { actor: ActorId(5), counter: 1 },
            hlc: Hlc { ms: 1 + rng.below(6), logical: 0 },
            body: OpBody::Set {
                node,
                field: field::PROPS,
                path: random_tree_path(&mut rng),
                enc: if rng.below(3) == 0 { Vec::new() } else { Leaf::Int(-1).encode() },
            },
        };
        full.apply(&late);
        pruned.apply(&late);
        assert_eq!(pruned.nodes[&node].tree(field::PROPS), full.nodes[&node].tree(field::PROPS), "late, round {round}");
    }
}

#[test]
fn a_hidden_register_stays_hidden_across_a_snapshot() {
    let dir = temp_dir("hidden");
    let path = dir.join("session.mnf");
    let mut journal = Journal::open(&path, ActorId(1)).unwrap();
    let mut txn = journal.begin().unwrap();
    let (tab, create) = create_tab(&mut txn, "https://h.example/");
    txn.commit(vec![create, set_at(tab, props_path(&["a", "b"]), Leaf::Int(1))]).unwrap();
    let txn = journal.begin().unwrap();
    txn.commit(vec![OpBody::Set { node: tab, field: field::PROPS, path: props_path(&["a"]), enc: Vec::new() }])
        .unwrap();
    journal.close().unwrap(); // the snapshot drops the hidden /a/b
    let mut doc = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
    let props_regs = |doc: &Doc| doc.nodes[&tab].fields.keys().filter(|k| k.field == field::PROPS).count();
    assert_eq!(props_regs(&doc), 1, "only the /a tombstone");
    // A replica that still holds an older /a/b applies it late: it must stay hidden.
    let older = Op {
        id: NodeId { actor: ActorId(7), counter: 1 },
        hlc: Hlc { ms: 1, logical: 0 },
        body: set_at(tab, props_path(&["a", "b"]), Leaf::Int(2)),
    };
    doc.apply(&older);
    assert!(doc.nodes[&tab].tree(field::PROPS).is_empty());
}
