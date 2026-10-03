//! On Linux the host links `libcef.so`, but the dynamic loader never looks in the executable's own
//! directory (Windows does, for `libcef.dll`). An rpath of `$ORIGIN` makes it look there, so a host copied
//! beside a runtime (`process_manager::ensure_host_copy`) or built beside one (`target/<profile>/`) finds
//! that runtime's `libcef.so` without `LD_LIBRARY_PATH`. No effect on other targets.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
