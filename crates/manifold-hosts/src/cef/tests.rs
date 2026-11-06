use super::*;

#[test]
fn windows_and_linux_have_pins_for_their_own_builds() {
    let urls = Os::ALL.map(|os| pin(os).map(|pin| pin.url));
    assert!(urls[0].is_some_and(|url| url.contains("_windows64_minimal")), "{urls:?}");
    assert!(urls[1].is_some_and(|url| url.contains("_linux64_minimal")), "{urls:?}");
    assert_eq!(urls[2..], [None, None]);
}

#[test]
fn only_linux_needs_system_libraries() {
    assert_eq!(system_libs(Os::Linux), ["libnss3.so", "libnspr4.so"]);
    for os in [Os::Windows, Os::Mac, Os::Other] {
        assert!(system_libs(os).is_empty(), "{os:?}");
    }
}
