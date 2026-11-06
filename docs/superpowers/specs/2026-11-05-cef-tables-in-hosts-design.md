---
status: active
---

# CEF's tables move into `manifold-hosts` (CRT5)

## Why

What is true of CEF's build per OS sits in the app, in two files keyed by a `cfg`:

- `crates/manifold/src/cef_pin.rs`: the generated pins, with `pin_for(id)` and `pin_for_platform()`.
- `crates/manifold/src/cef_platform.rs`: `CefPlatform` and a `PLATFORM` const with one row per `cfg`.

`manifold-hosts` is the one copy of a host's layout, and every answer there takes the OS as an argument so one test binary checks every OS. CEF's tables are the exception. A `cfg` means Windows runs only the Windows row's tests, the gate cannot read the pins as data (`shots.rs` does `include_str!` on the app's source and scans for `version: "`), and `gate cef-pin` writes into the app by path.

The two `PLATFORM` rows also differ in only three of their six fields (`id`, `skip_suffixes`, `system_libs`), so most of the struct is constants.

Decision z1 (2026-11-05): functions, no struct. `manifold-hosts/src/cef.rs` gets `Pin`, the generated consts, `pin(os)`, `archive_name(os, v)` and `system_libs(os)`. Unpack constants stay in the app. `hash_file` leaves `gate/src/cef_pin.rs` for another gate module.

**One change from z1:** review found `archive_name` has no reason to exist, so this spec drops it (see Searched).

## Design

**`crates/manifold-hosts/src/cef.rs`** (new, `pub mod cef;` in `lib.rs`, `std` only, no `cfg`):

```rust
/// The CEF build the app downloads: where it lives and how to check it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin { pub version: &'static str, pub url: &'static str, pub size: u64, pub sha1: &'static str, pub sha256: &'static str }

// BEGIN windows64
const PIN_WINDOWS64: Pin = Pin { .. };
// END windows64

// BEGIN linux64
const PIN_LINUX64: Pin = Pin { .. };
// END linux64

/// The CEF build pinned for `os`: `None` where there is none yet.
pub fn pin(os: Os) -> Option<&'static Pin>

/// Shared libraries the host loads from the system rather than from the runtime.
pub fn system_libs(os: Os) -> &'static [&'static str]
```

The `Pin` struct moves from `view_install.rs:12`, and `Plan.runtime` stays `Option<Pin>`. `Os` has no architecture, so "Linux" means x64, as `PLATFORM` does today.

**App.**

- Readers call `manifold_hosts::cef::pin(Os::here())`: `availability.rs:129`, `settings/page.rs:487`, `view_install.rs:119`, `view_install/ui.rs:92`, `process_manager/tests.rs` (3), `view_install/tests.rs:162`.
- `view_install/unpack.rs` gets `RUNTIME_DIRS`, `SKIP_DIRS`, `SKIP_FILES` and `SKIP_SUFFIXES` as private constants beside their one reader. `SKIP_SUFFIXES` (`.lib`) now applies on every OS. That changes nothing on Linux, where the runtime folders hold no Windows import libraries (an inference from the layout noted in `cef_platform`; unchecked: no linux64 archive was at hand on 2026-11-05).
- The method `missing_system_libs` becomes the free function `missing_cef_libs()` in `view_install/state.rs`, next to the other system-library checks, and `missing_in_listing` moves with it as `pub(super)`. The name differs because `state.rs:59` already has a `missing_system_libs(kind)`. `process_manager.rs:124` points its `ViewDesc` row at the new function. The needed list comes from `cef::system_libs(Os::here())`. The 10 s cache stays; there is one caller, so it needs no key.
- `view_install/tests.rs` gets a helper `archive_json_name(version)` for the name its `archive.json` fixtures carry, replacing 8 `PLATFORM.archive_name` calls. `custom_version` (`state.rs:165-177`) ignores the platform part, so the helper spells `windows64` on every OS.
- `cef_pin.rs`, `cef_platform.rs` and their two `mod` lines are deleted.

**Gate.**

- `cef_pin.rs` rewrites blocks only. It gains `block_of(text, id) -> Option<&str>`, `replace_block(text, id, block)` built on it, and `render_block(id, version, url, size, sha1, sha256)`. `render_cef`, `blocks_of`, `CEF_HEADER` and the generated `pin_for` go. The output path becomes `crates/manifold-hosts/src/cef.rs`, and the module doc is rewritten to match (new path, new platforms refused, the version test's new home).
- A platform with no block is refused before the index query and the download, naming what to add by hand: it needs a `pin` arm and an install layout anyway. `CEF_PLATFORMS` keeps its four names.
- `hash_file` moves to `release_hosts.rs`, with the imports only it uses (`Read`, `Path`, `to_hex`, `Sha1`, `Sha256`, `Digest`), which leave `cef_pin.rs` along with `BTreeMap`. `release_check.rs`, `release_verify.rs` and `cef_pin.rs` change their `use` line.
- `shots.rs`: `CEF_PIN`, `pin_version`, its only helper `between` and the parse assert go, replaced by `cef::pin(Os::here())`. The `_windows64_minimal` fixture literal at `:1703` stays, like `fake_cef_host`'s literal SHA: both are fixtures nothing compares to the pin. `shots.rs` shrinks from 1789 production lines, and `limits.rs` is an exact ratchet, so its `PROD_ALLOWED` ceiling is set to the measured number in the same commit.

## Behaviour change

One, and it is a fix. `PLATFORM` reads every non-Windows OS as Linux, so on macOS and other OSes `pin_for_platform()` returns the **Linux** pin today. `plan_for` still offers no download there, because it stops first at the empty host pin (`no_download_reason`), so `PARKED.md:27` is right that nothing is offered. But three direct readers do see the Linux pin: `runtime_on_disk` (`availability.rs:129`), the custom-folder verdict (`ui.rs:92`) and the newer-than-tested note (`settings/page.rs:487`). So does `plan_for` if a release ever pins a macOS CEF host.

`pin(Os::Mac)` and `pin(Os::Other)` are `None`, so those three readers take their no-pin branches: `RuntimeOnDisk::Missing`, `NO_PIN_REASON`, no note. `system_libs` is empty there too, so `missing_cef_libs` stops running `ldconfig`; its only reader is the download dialog, which those OSes never reach. Windows and Linux answers are unchanged. No macOS build exists, so nobody has met this.

## Searched

The deletion pass tried:

- **Removing `archive_name` and its id mapping.** Done, against z1. Production never builds the name: `custom_version` reads `archive.json` and takes the version, and the CDN's own `archive.json` is the authority on the format. In the app it is `#[cfg(test)]`, and the gate's fixture spells it by hand. Keeping it needed a third per-OS id (`windows64`) beside `Os::pin_platform`'s `windows-x64`. The cost is the format spelled in two fixtures, which nothing checks against the CDN today either.
- **Moving `system_libs` into the `Runtime` row** as a `fn(Os)` field with a `Host::system_libs` accessor, after `runtime_lib`. Works. Rejected: a field, an accessor and a function for one row, against one function.
- **Generating `pin`'s arms inside the function, removing the named consts.** Rejected: `render_block` would need an id-to-`Os` name in the gate, which cancels the gain.
- **Generating all of `pin`.** Rejected: that is why `render_cef` rewrites the whole file. A hand-written match lets the generator touch only blocks. A block added by hand with no arm fails lint as a dead const.
- **Keeping `cef_platform.rs` for `missing_cef_libs`.** Rejected: a module named for a struct that no longer exists, when `state.rs` already holds that family.

Removed: `CefPlatform`, `PLATFORM`, both `cfg`s, `pin_for`, `pin_for_platform`, `archive_name`, `render_cef`, `blocks_of`, `CEF_HEADER`, `pin_version`, `between`, `CEF_PIN`.

## Tests

In `crates/manifold-hosts/src/cef/tests.rs` (`lib.rs`'s inline block is at its ceiling), written first:

- `windows_and_linux_have_pins_for_their_own_builds`: `pin` is `Some` exactly for those two of `Os::ALL`, with `_windows64_minimal` and `_linux64_minimal` in their URLs.
- `only_linux_needs_system_libraries`.

In `crates/manifold-hosts/tests/layout.rs`: `the_pin_matches_the_cef_crates_version`, moved from `view_install/tests.rs`, reading `Cargo.lock` through `repo()` and checking both pins.

In `gate/src/cef_pin.rs`: `the_checked_in_cef_table_round_trips` becomes, for `[("windows64", Os::Windows), ("linux64", Os::Linux)]`, `render_block` of `pin(os)` equals `block_of(text, id)`, and `replace_block` with it returns the file unchanged. `an_unknown_block_is_refused` is new.

Moved with their code: `a_library_is_missing_unless_the_listing_names_it` to `view_install/tests.rs`. The test at `view_install/tests.rs:635` asserts plainly that `libcef.lib` is gone.

Dropped, covered above: `the_archive_name_carries_the_platform_id`, `this_platforms_pin_is_for_this_platform`, `pin_lookup_is_none_for_a_platform_without_a_pin`.

Guard: `deleted_names.rs` gains `CefPlatform`, `cef_platform`, `pin_for_platform`; the needle `CefPlatform::runtime_lib` is subsumed. It scans `crates/manifold/src` only, comments included, so `view_install.rs:43` and `unpack.rs:53-54` are rewritten in the same commit. `layout.rs`'s leaf check already holds the crate at zero dependencies.

Checks: `cargo unit`, `cargo unit-linux` (it includes `-p manifold-hosts`), lint. `gate cef-pin` is proved without a download by the round-trip test; a real re-pin stays a release step. The gate at the tier `gate tier` advises, since `shots` changes.

## Order of work

Each step compiles and lints on its own.

1. Add `cef.rs` and its tests, with the version test in `layout.rs`.
2. Gate: generator, `hash_file`, `shots.rs`, `limits.rs`. It must precede step 3, as `shots.rs` `include_str!`s the file step 3 deletes.
3. App: readers, unpack constants, `missing_cef_libs`, the test helper, the guard, and the deletions.

The pins exist twice only between steps 1 and 3; the app's copy is unused by the generator from step 2.

## End-state fit

| Change | Lands in | Replaces | Deletes |
|---|---|---|---|
| `Pin`, consts, `pin`, `system_libs` | `manifold-hosts/src/cef.rs` | `cef_pin.rs`, `PLATFORM`'s `id` and `system_libs` | `crates/manifold/src/cef_pin.rs` |
| Unpack constants | `view_install/unpack.rs` | `PLATFORM`'s four layout fields | `CefPlatform` |
| `missing_cef_libs` | `view_install/state.rs` | the `CefPlatform` method | `crates/manifold/src/cef_platform.rs` |
| Generator | `gate/src/cef_pin.rs` | whole-file render | `render_cef`, `blocks_of`, `CEF_HEADER` |
| `hash_file` | `gate/src/release_hosts.rs` | | |
| Fixture | `gate/src/shots.rs` | `include_str!` and text scan | `CEF_PIN`, `pin_version`, `between` |

The restructuring plan's CRT5 item takes **Own plan** with a link here now, which also records that `archive_name` was dropped from z1.

Docs in the same commit: `README.md:28` ("no `CefPlatform` entry" becomes "no `cef::pin` entry"); `PARKED.md:27` (per Behaviour change, with the stale `:78` cite replaced), `:164`, `:193` (CRT5 leaves it); `docs/decisions/2026-10-18-hosts-in-the-release.md:13`; `manifold-hosts`' crate doc and description, which say "one row per view host". `README.md:113` names only the command and stays. `docs/devlog.md:495` is a dated record and stays unless it states a current path as current.
