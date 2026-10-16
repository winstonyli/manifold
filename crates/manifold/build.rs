//! Embeds the WebView2 host and the PDF view in `manifold.exe` when `MANIFOLD_WEBVIEW2_HOST` / `MANIFOLD_PDFIUM_HOST`
//! name a file (`release.yml` sets them); every other build embeds an empty stub, so dev builds, the gate and
//! Linux CI keep using the sibling exes. `embedded_host.rs` reads the outputs: `OUT_DIR/<name>-host.bin` and the
//! `MANIFOLD_<NAME>_SHA256` env value (lowercase hex, empty for the stub).

use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// (variable naming the file, output file under `OUT_DIR`, env value carrying the hash).
const HOSTS: [(&str, &str, &str); 2] = [
    ("MANIFOLD_WEBVIEW2_HOST", "webview2-host.bin", "MANIFOLD_WEBVIEW2_SHA256"),
    ("MANIFOLD_PDFIUM_HOST", "pdfium-host.bin", "MANIFOLD_PDFIUM_SHA256"),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A build script's `cfg(windows)` describes the build machine; the target is what decides whether a host exists.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    for (var, bin, sha_var) in HOSTS {
        println!("cargo:rerun-if-env-changed={var}");
        let host = std::env::var_os(var).filter(|path| !path.is_empty() && windows);
        let (bytes, sha256) = match host {
            Some(path) => {
                let path = PathBuf::from(path);
                println!("cargo:rerun-if-changed={}", path.display());
                let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{var} {}: {e}", path.display()));
                assert!(!bytes.is_empty(), "{var} {} is empty", path.display());
                let sha256 = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                (bytes, sha256)
            }
            None => (Vec::new(), String::new()),
        };
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join(bin);
        std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
        println!("cargo:rustc-env={sha_var}={sha256}");
    }
}
