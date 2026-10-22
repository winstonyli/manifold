
#[test]
fn detail_levels_say_what_a_card_draws() {
    assert!(Detail::Full.content() && Detail::Full.link_handles());
    assert!(!Detail::Simple.content() && !Detail::Simple.link_handles());
    assert_eq!(Detail::Full.glow_layers(false), 2);
    assert_eq!(Detail::Simple.glow_layers(false), 0);
    assert_eq!(Detail::Simple.glow_layers(true), 2, "hover brings the glow back");
}
