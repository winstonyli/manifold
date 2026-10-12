//! Embeds the WebView2 host in `manifold.exe` when `MANIFOLD_WEBVIEW2_HOST` names a file (`scripts/release.sh` sets
//! it); every other build embeds an empty stub, so dev builds, the gate and Linux CI keep using the sibling host.
//! `embedded_host.rs` reads the two outputs: `OUT_DIR/webview2-host.bin` and the `MANIFOLD_WEBVIEW2_SHA256` env
//! value (lowercase hex, empty for the stub).

use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MANIFOLD_WEBVIEW2_HOST");
    println!("cargo:rerun-if-changed=build.rs");
    // A build script's `cfg(windows)` describes the build machine; the target is what decides whether a host exists.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let host = std::env::var_os("MANIFOLD_WEBVIEW2_HOST").filter(|path| !path.is_empty() && windows);
    let (bytes, sha256) = match host {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&path).display());
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("MANIFOLD_WEBVIEW2_HOST {}: {e}", PathBuf::from(&path).display()));
            assert!(!bytes.is_empty(), "MANIFOLD_WEBVIEW2_HOST {} is empty", PathBuf::from(&path).display());
            let sha256 = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            (bytes, sha256)
        }
        None => (Vec::new(), String::new()),
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("webview2-host.bin");
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    println!("cargo:rustc-env=MANIFOLD_WEBVIEW2_SHA256={sha256}");
}
