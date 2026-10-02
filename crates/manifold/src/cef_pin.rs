//! The CEF build the app downloads on demand.

use crate::engine_install::Pin;

// Later install tasks use it.
#[allow(dead_code)]
pub(crate) const CEF_PIN: Pin = Pin {
    version: "152.0.6+g708dc14+chromium-152.0.7977.83",
    url: "https://cef-builds.spotifycdn.com/cef_binary_152.0.6%2Bg708dc14%2Bchromium-152.0.7977.83_windows64_minimal.tar.bz2",
    size: 171_619_649,
    sha1: "e5e3020627f4528bd43e22f4c4970000b0458e99",
    // Placeholder: scripts/cef-pin.sh writes the real SHA-256 (Task 4 replaces it).
    sha256: "0000000000000000000000000000000000000000000000000000000000000000",
};
