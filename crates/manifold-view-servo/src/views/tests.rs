use super::LoadState;

#[test]
fn a_sent_load_is_not_in_place() {
    let mut s = LoadState::default();
    assert!(s.load_in_place() && s.history_in_place());
    s.load_sent();
    assert!(!s.load_in_place());
}

#[test]
fn head_parsed_clears_a_load() {
    let mut s = LoadState::default();
    s.load_sent();
    s.head_parsed();
    assert!(s.load_in_place() && s.history_in_place());
}

#[test]
fn a_pending_load_blocks_a_back_in_place() {
    let mut s = LoadState::default();
    s.load_sent();
    assert!(!s.history_in_place());
}

#[test]
fn an_open_traversal_blocks_a_load_until_complete() {
    let mut s = LoadState::default();
    s.traversal_started();
    assert!(!s.load_in_place());
    s.traversal_complete();
    assert!(s.load_in_place());
}

#[test]
fn an_open_traversal_does_not_block_a_back() {
    let mut s = LoadState::default();
    s.traversal_started();
    assert!(s.history_in_place());
}

#[test]
fn traversal_complete_never_goes_below_zero() {
    let mut s = LoadState::default();
    s.traversal_complete();
    s.traversal_started();
    assert!(!s.load_in_place(), "one open traversal after a stray complete");
}
