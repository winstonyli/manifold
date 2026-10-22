
#[test]
fn detail_levels_say_what_a_card_draws() {
    assert!(Detail::Full.content() && Detail::Full.link_handles());
    assert!(!Detail::Simple.content() && !Detail::Simple.link_handles());
    assert_eq!(Detail::Full.glow_layers(false), 2);
    assert_eq!(Detail::Simple.glow_layers(false), 0);
    assert_eq!(Detail::Simple.glow_layers(true), 2, "hover brings the glow back");
}

#[test]
fn detail_flips_with_a_hysteresis_gap() {
    assert_eq!(Detail::Full.next(LOD_SIMPLIFY_COUNT), Detail::Full, "at the threshold: not yet");
    assert_eq!(Detail::Full.next(LOD_SIMPLIFY_COUNT + 1), Detail::Simple);
    assert_eq!(Detail::Simple.next(LOD_RESTORE_COUNT), Detail::Simple, "in the gap it stays");
    assert_eq!(Detail::Full.next(LOD_RESTORE_COUNT), Detail::Full);
    assert_eq!(Detail::Simple.next(LOD_RESTORE_COUNT - 1), Detail::Full);
}
