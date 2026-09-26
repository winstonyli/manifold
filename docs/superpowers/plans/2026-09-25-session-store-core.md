# Session Store Core — Implementation Plan (Plan A of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the pure `store/` module: the binary op-log format, CRDT merge, journal file with locking and compaction, and JSON snapshots, fully unit-tested, with nothing in the app using it yet.

**Architecture:** Seven files under `crates/manifold/src/store/`, each one layer: `varint` and `crc32c` (primitives), `tlv` (protobuf-like messages), `model` (ids, clock, kinds, field table, document), `ops` (the four ops and their merge, including Kleppmann tree-move), `journal` (frames, file, lock, compaction), `json` (serde snapshots). No egui, no `App`. Plan B (`2026-09-25-session-store-integration.md`) wires it into the app through `session_sync.rs` and deletes `session.rs` and `group_export.rs`.

**Tech Stack:** Rust (nightly 1.100, edition 2021), `std::fs::File::lock` (stable since 1.89), `getrandom 0.3` (already in `Cargo.lock` at 0.3.4), `serde`/`serde_json` (already direct dependencies). One crate: `crates/manifold` (a bin crate).

**Spec:** `docs/superpowers/specs/2026-09-25-session-op-log-design.md`

## Global Constraints

- Work on `dev`. Never push. Commit at the end of each task with the message given, ending with the trailer line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Never run `cargo fmt`** (the crate has hand-formatted regions). Write the code exactly as given.
- Build and test with `-j 12` (leave cores free on this 16-core machine).
- Every task ends with both green:
  - `cargo test -q -p manifold -j 12 store::` (the new module's tests; the task says how many);
  - `cargo clippy -q -p manifold -j 12 --all-targets 2>&1 | grep -F 'src\store'` prints nothing (no warning in `store/`; other files' warnings are not this plan's).
- Nothing outside `store/` changes except one line in `main.rs` (`mod store;`) and one in `crates/manifold/Cargo.toml` (`getrandom`). `store/mod.rs` carries `#![allow(dead_code)]` until Plan B uses the module.
- The code below was compiled and its 41 tests run (all passing, clippy clean) in a scratch crate before this plan was written. If a test fails, the transcription is wrong or the environment differs: find out which before changing code.
- Tests that touch files use `store::testutil::temp_dir`, under the system temp directory. Nothing touches `%APPDATA%`.

## Rulings made while writing this plan

These settle points the spec left open or got wrong. Task 7 writes them into the spec.

1. **Ids inside field values carry the full 64-bit actor.** `parent` (and any future id-valued field) is encoded `{1: actor (fixed64), 2: counter}`. The actor index appears only in a frame's own structure: op targets, `Move` groups, snapshot node keys and group edges, stamps. Why: compaction renumbers the actor table, and a field this build doesn't know (kept raw) could not be renumbered.
2. **Moves apply to deleted nodes too.** The spec said "a `Move` of a tombstoned node is ignored". That breaks convergence: whether a move is ignored would depend on whether the `Delete` arrived first, and undo/redo of later moves would differ between replicas. Moves always enter the move log; display skips deleted nodes. `Set`s on deleted nodes are still ignored (they only touch registers, which a deleted node no longer has).
3. **A `Create`'s op id is separate from its node id.** The node id is drawn when the node is made (so the app can refer to it before saving); op ids are drawn when ops are stamped. Both come from the launch's one counter.
4. **Black is a colour.** `color` is `Option<u32>`; `Some(0)` is encoded (varint 0), `None` is absent. General rule: omit a field only when it holds exactly what absence decodes to.
5. **The header carries a random 64-bit generation** instead of the spec's "file id". `file = magic | format_version | generation (u64 LE) | frame*`. Each compaction writes a new one; a reader seeing a different generation (or a shorter file) re-reads from the start. Why: Windows file ids need the unstable `windows_by_handle` API.
6. **Stamps must be unique per op.** Last-writer-wins and the move log both order by `(hlc, actor)`; two different ops with equal stamps would merge in arrival order. `Clock::tick` guarantees uniqueness; test helpers must too (`logical = counter`).
7. **fsync opens the file for writing.** On Windows, `FlushFileBuffers` on a read-only handle fails with "Access is denied" (found by the `a_newer_builds_frames_and_fields_survive` test).
8. **Document id text form (decided 2026-09-25, already in the spec).** Crockford's mod-37 check symbols `* ~ $ =` contradicted the spec's claims, so the text form is 26 digits plus two check digits making the whole number 0 mod 1021; a brute-force check found no undetected single substitution or adjacent transposition. Nothing in this plan uses the text form (`--dump` prints hex).

---

### Task 1: `store/` module and varint

**Files:**
- Create: `crates/manifold/src/store/mod.rs`
- Create: `crates/manifold/src/store/varint.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod store;` after `mod session;`, line 13)

**Interfaces:**
- Produces: `varint::{write(out: &mut Vec<u8>, v: u64), read(buf: &[u8]) -> Result<(u64, usize), VarintError>, encoded_len(v: u64) -> usize, zigzag(i64) -> u64, unzigzag(u64) -> i64}`, `VarintError::{Truncated, Overflow}`; `store::testutil::{XorShift, temp_dir}` (test-only).

The varint is bijective prefix-length (spec "Varint"): the first byte's leading ones give the length, and each longer tier starts where the shorter one ends (offsets 0, 128, 16 512, 2 113 664, …), so every value has exactly one encoding. The 9-byte tier is `0xFF` then a big-endian `u64` minus the tier offset, with an overflow check.

- [ ] **Step 1: Create `store/mod.rs`** with only the `varint` module for now (later tasks add a line each, keeping them in alphabetical order):

```rust
//! The session store: a replicated operation log in a custom binary
//! format. Pure (no egui, no `App`); `session_sync` adapts it to the live
//! graph. See docs/superpowers/specs/2026-09-25-session-op-log-design.md.

// Nothing outside `store` uses it until the app integration lands.
#![allow(dead_code)]

pub mod varint;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// Seeded xorshift64*, so randomized tests repeat without a dependency.
    pub struct XorShift(pub u64);

    impl XorShift {
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        pub fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A fresh, empty directory for one test, under the system temp dir.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
```

- [ ] **Step 2: Create `store/varint.rs`:**

```rust
//! Bijective prefix varint (spec "Varint: bijective prefix, big-endian"):
//! the first byte's leading 1-bits count the extra bytes, payload bits are
//! big-endian, and each tier starts where the one below it ends, so every
//! `u64` has exactly one encoding and byte order equals numeric order.

/// The first value of each tier: tier `k` (`k` extra bytes) holds
/// `OFFSETS[k]..OFFSETS[k + 1]`, and tier 8 runs to `u64::MAX`.
const OFFSETS: [u64; 9] = {
    let mut offsets = [0u64; 9];
    let mut k = 0;
    while k < 8 {
        offsets[k + 1] = offsets[k] + (1u64 << (7 + 7 * k));
        k += 1;
    }
    offsets
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarintError {
    /// The buffer ends inside the varint.
    Truncated,
    /// A 9-byte varint whose value would pass `u64::MAX`.
    Overflow,
}

/// How many bytes `write` uses for `value`: 1 to 9.
pub fn encoded_len(value: u64) -> usize {
    (0..8).find(|&k| value < OFFSETS[k + 1]).map_or(9, |k| k + 1)
}

pub fn write(out: &mut Vec<u8>, value: u64) {
    let extra = encoded_len(value) - 1;
    let payload = (value - OFFSETS[extra]).to_be_bytes();
    if extra == 8 {
        out.push(0xFF);
        out.extend_from_slice(&payload);
        return;
    }
    // `extra` leading 1-bits, a 0, then the payload's top bits.
    let prefix = !(0xFFu8 >> extra);
    let tail = &payload[7 - extra..];
    out.push(prefix | tail[0]);
    out.extend_from_slice(&tail[1..]);
}

/// Decodes the varint at the start of `buf`: its value and its length.
pub fn read(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let first = *buf.first().ok_or(VarintError::Truncated)?;
    let extra = first.leading_ones() as usize;
    let len = extra + 1;
    let bytes = buf.get(..len).ok_or(VarintError::Truncated)?;
    if extra == 8 {
        let payload = u64::from_be_bytes(bytes[1..].try_into().expect("8 bytes"));
        let value = payload.checked_add(OFFSETS[8]).ok_or(VarintError::Overflow)?;
        return Ok((value, len));
    }
    let mut payload = u64::from(first & (0x7F >> extra));
    for &b in &bytes[1..] {
        payload = (payload << 8) | u64::from(b);
    }
    Ok((payload + OFFSETS[extra], len))
}

/// Maps signed to unsigned so small magnitudes stay short: 0, -1, 1, -2 ...
/// become 0, 1, 2, 3 ...
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

pub fn unzigzag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::XorShift;

    fn enc(v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        write(&mut out, v);
        out
    }

    fn round_trip(v: u64) {
        let bytes = enc(v);
        assert_eq!(bytes.len(), encoded_len(v), "{v}");
        assert_eq!(read(&bytes), Ok((v, bytes.len())), "{v}");
    }

    #[test]
    fn tiers_start_where_the_spec_says() {
        assert_eq!(OFFSETS[1], 128);
        assert_eq!(OFFSETS[2], 16_512);
        assert_eq!(OFFSETS[3], 2_113_664);
        for (k, &offset) in OFFSETS.iter().enumerate() {
            round_trip(offset);
            assert_eq!(encoded_len(offset), k + 1);
            if k > 0 {
                round_trip(offset - 1);
                assert_eq!(encoded_len(offset - 1), k);
            }
        }
        round_trip(u64::MAX);
        assert_eq!(enc(u64::MAX).len(), 9);
    }

    #[test]
    fn one_and_two_byte_tiers_round_trip_exhaustively() {
        for v in 0..OFFSETS[2] {
            round_trip(v);
        }
    }

    #[test]
    fn every_one_and_two_byte_string_decodes_canonically() {
        for b0 in 0u8..=0xBF {
            let strings: Vec<Vec<u8>> =
                if b0 < 0x80 { vec![vec![b0]] } else { (0..=255u8).map(|b1| vec![b0, b1]).collect() };
            for bytes in strings {
                let (v, len) = read(&bytes).unwrap();
                assert_eq!(len, bytes.len());
                assert_eq!(enc(v), bytes);
            }
        }
    }

    #[test]
    fn random_byte_strings_of_every_tier_decode_canonically() {
        let mut rng = XorShift(7);
        for _ in 0..20_000 {
            let extra = rng.below(9) as usize;
            let mut bytes: Vec<u8> = (0..=extra).map(|_| rng.next() as u8).collect();
            bytes[0] = if extra == 8 { 0xFF } else { !(0xFFu8 >> extra) | (bytes[0] & (0x7F >> extra)) };
            match read(&bytes) {
                Ok((v, len)) => {
                    assert_eq!(len, bytes.len());
                    assert_eq!(enc(v), bytes);
                }
                Err(err) => assert!(extra == 8 && err == VarintError::Overflow),
            }
        }
    }

    #[test]
    fn random_values_round_trip_and_sort_like_numbers() {
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            // Shifting spreads the values over every tier.
            let a = rng.next() >> rng.below(64);
            let b = rng.next() >> rng.below(64);
            round_trip(a);
            assert_eq!(enc(a).cmp(&enc(b)), a.cmp(&b), "{a} vs {b}");
        }
    }

    #[test]
    fn nine_byte_overflow_and_truncation_are_errors() {
        let mut bytes = [0xFFu8; 9];
        assert_eq!(read(&bytes), Err(VarintError::Overflow));
        bytes[1..].copy_from_slice(&(u64::MAX - OFFSETS[8]).to_be_bytes());
        assert_eq!(read(&bytes), Ok((u64::MAX, 9)));
        assert_eq!(read(&[]), Err(VarintError::Truncated));
        assert_eq!(read(&enc(OFFSETS[3])[..2]), Err(VarintError::Truncated));
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [0, 1, -1, 2, -2, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!((zigzag(0), zigzag(-1), zigzag(1)), (0, 1, 2));
    }
}
```

- [ ] **Step 3: Register the module.** In `crates/manifold/src/main.rs`, after the line `mod session;`, add:

```rust
mod store;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::varint`
Expected: `7 passed`.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -q -p manifold -j 12 --all-targets 2>&1 | grep -F 'src\store'`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/store/mod.rs crates/manifold/src/store/varint.rs crates/manifold/src/main.rs
git commit -m "store: bijective prefix varint" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: CRC-32C

**Files:**
- Create: `crates/manifold/src/store/crc32c.rs`
- Modify: `crates/manifold/src/store/mod.rs` (add `pub mod crc32c;` above `pub mod varint;`)

**Interfaces:**
- Consumes: `store::testutil::XorShift` (Task 1).
- Produces: `crc32c::crc32c(data: &[u8]) -> u32`.

Table-driven, with an SSE4.2 path (`_mm_crc32_u64`/`_mm_crc32_u8`) chosen at run time; a test checks the two paths agree. The check values are the standard `123456789` vector and RFC 3720 appendix B.4.

- [ ] **Step 1: Create `store/crc32c.rs`:**

```rust
//! CRC-32C (Castagnoli), table-driven, with an SSE4.2 path where the CPU
//! has it. Every journal frame carries one (spec "File and frames").

const POLY: u32 = 0x82F6_3B78; // reflected Castagnoli polynomial

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn crc32c(data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the CPU was just checked for SSE4.2.
        return unsafe { sse42(data) };
    }
    table(data)
}

fn table(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
fn sse42(data: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};
    let (words, rest) = data.as_chunks::<8>();
    let mut c = u64::from(!0u32);
    for word in words {
        c = _mm_crc32_u64(c, u64::from_le_bytes(*word));
    }
    let mut c = c as u32;
    for &b in rest {
        c = _mm_crc32_u8(c, b);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::XorShift;

    #[test]
    fn standard_check_vectors() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
        // RFC 3720 appendix B.4.
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
        let ascending: Vec<u8> = (0..32).collect();
        assert_eq!(crc32c(&ascending), 0x46DD_794E);
    }

    #[test]
    fn sse42_path_equals_table_path() {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("sse4.2") {
            let mut rng = XorShift(99);
            for len in 0..300 {
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                // SAFETY: checked for SSE4.2 above.
                assert_eq!(unsafe { sse42(&data) }, table(&data), "len {len}");
            }
        }
    }
}
```

- [ ] **Step 2: Add `pub mod crc32c;`** to `store/mod.rs`, above `pub mod varint;`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::crc32c`
Expected: `2 passed`.

- [ ] **Step 4: Clippy** as in Task 1 Step 5: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/crc32c.rs crates/manifold/src/store/mod.rs
git commit -m "store: CRC-32C with an SSE4.2 path" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: TLV messages

**Files:**
- Create: `crates/manifold/src/store/tlv.rs`
- Modify: `crates/manifold/src/store/mod.rs` (add `pub mod tlv;` above `pub mod varint;`)

**Interfaces:**
- Consumes: `varint::{read, write, VarintError}` (Task 1).
- Produces: `tlv::Writer` (`new, is_empty, into_bytes, varint(field: u32, v: u64), fixed64, fixed32, bytes(field, &[u8]), str, f32, message(field, &Writer), raw(&[u8])`), `tlv::Reader::new(&[u8])` iterating `Result<Field, TlvError>`, `Field { no: u32, value: Value, raw: &[u8] }`, `Value::{u64, u32, bool, fixed64, f32, bytes, str}` accessors, `TlvError::{Truncated, Overflow, BadWire(u8), BadField, BadUtf8, BadValue}`.

Protobuf-like (spec "TLV messages"): key varint `(field_no << 3) | wire`, wire types 0 varint, 1 fixed64 LE, 2 length-delimited, 5 fixed32 LE. Readers skip unknown fields by wire type, which is how an older build reads a newer build's messages. `Field::raw` is the whole encoded field, so unknown fields can be written back byte for byte.

- [ ] **Step 1: Create `store/tlv.rs`:**

```rust
//! Protobuf-like TLV messages (spec "TLV messages"): each field is a key
//! varint `(field_no << 3) | wire_type`, then a value. Readers skip fields
//! they don't know by wire type. Writers emit fields in ascending number,
//! each at most once, and leave out fields holding their default, so one
//! value has one encoding; that part is up to the caller.

use super::varint::{self, VarintError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Varint = 0,
    Fixed64 = 1,
    Bytes = 2,
    Fixed32 = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlvError {
    Truncated,
    Overflow,
    /// A wire type other than 0, 1, 2 or 5: its length is unknowable.
    BadWire(u8),
    /// Field number 0, or one past `u32::MAX`.
    BadField,
    BadUtf8,
    /// A value of the wrong wire type or range for its field.
    BadValue,
}

impl From<VarintError> for TlvError {
    fn from(err: VarintError) -> Self {
        match err {
            VarintError::Truncated => TlvError::Truncated,
            VarintError::Overflow => TlvError::Overflow,
        }
    }
}

impl std::fmt::Display for TlvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn key(&mut self, field: u32, wire: Wire) {
        varint::write(&mut self.buf, (u64::from(field) << 3) | wire as u64);
    }

    pub fn varint(&mut self, field: u32, v: u64) {
        self.key(field, Wire::Varint);
        varint::write(&mut self.buf, v);
    }

    pub fn fixed64(&mut self, field: u32, v: u64) {
        self.key(field, Wire::Fixed64);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn fixed32(&mut self, field: u32, v: u32) {
        self.key(field, Wire::Fixed32);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.key(field, Wire::Bytes);
        varint::write(&mut self.buf, b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn str(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }

    /// Writes `-0.0` as `0.0`, so equal values encode equally. NaN is
    /// never stored; callers drop it first.
    pub fn f32(&mut self, field: u32, v: f32) {
        debug_assert!(!v.is_nan(), "NaN is never stored");
        self.fixed32(field, if v == 0.0 { 0.0f32 } else { v }.to_bits());
    }

    pub fn message(&mut self, field: u32, m: &Writer) {
        self.bytes(field, &m.buf);
    }

    /// Appends an already encoded field, key and value, as a register holds it.
    pub fn raw(&mut self, encoded: &[u8]) {
        self.buf.extend_from_slice(encoded);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Value<'a> {
    pub fn u64(self) -> Result<u64, TlvError> {
        match self {
            Value::Varint(v) => Ok(v),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn u32(self) -> Result<u32, TlvError> {
        u32::try_from(self.u64()?).map_err(|_| TlvError::BadValue)
    }

    pub fn bool(self) -> Result<bool, TlvError> {
        Ok(self.u64()? != 0)
    }

    pub fn fixed64(self) -> Result<u64, TlvError> {
        match self {
            Value::Fixed64(v) => Ok(v),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn f32(self) -> Result<f32, TlvError> {
        match self {
            Value::Fixed32(v) => Ok(f32::from_bits(v)),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn bytes(self) -> Result<&'a [u8], TlvError> {
        match self {
            Value::Bytes(b) => Ok(b),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn str(self) -> Result<&'a str, TlvError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| TlvError::BadUtf8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field<'a> {
    pub no: u32,
    pub value: Value<'a>,
    /// The whole field as encoded: key and value.
    pub raw: &'a [u8],
}

/// Iterates a message's fields. After the first error it yields nothing.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn field(&mut self) -> Result<Field<'a>, TlvError> {
        let start = self.pos;
        let (key, n) = varint::read(&self.buf[self.pos..])?;
        self.pos += n;
        let no = u32::try_from(key >> 3).map_err(|_| TlvError::BadField)?;
        if no == 0 {
            return Err(TlvError::BadField);
        }
        let rest = &self.buf[self.pos..];
        let (value, n) = match (key & 7) as u8 {
            0 => {
                let (v, n) = varint::read(rest)?;
                (Value::Varint(v), n)
            }
            1 => (Value::Fixed64(u64::from_le_bytes(take(rest)?)), 8),
            2 => {
                let (len, n) = varint::read(rest)?;
                let end = usize::try_from(len).ok().and_then(|len| n.checked_add(len)).ok_or(TlvError::Truncated)?;
                (Value::Bytes(rest.get(n..end).ok_or(TlvError::Truncated)?), end)
            }
            5 => (Value::Fixed32(u32::from_le_bytes(take(rest)?)), 4),
            wire => return Err(TlvError::BadWire(wire)),
        };
        self.pos += n;
        Ok(Field { no, value, raw: &self.buf[start..self.pos] })
    }
}

fn take<const N: usize>(buf: &[u8]) -> Result<[u8; N], TlvError> {
    buf.get(..N).and_then(|b| b.try_into().ok()).ok_or(TlvError::Truncated)
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Field<'a>, TlvError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let field = self.field();
        if field.is_err() {
            self.pos = self.buf.len();
        }
        Some(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(buf: &[u8]) -> Vec<(u32, Value<'_>)> {
        Reader::new(buf).map(|f| f.map(|f| (f.no, f.value))).collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn every_wire_type_round_trips() {
        let mut inner = Writer::new();
        inner.varint(1, 5);
        let mut w = Writer::new();
        w.varint(1, 300);
        w.fixed64(2, u64::MAX);
        w.str(3, "héllo");
        w.f32(4, 1.5);
        w.message(5, &inner);
        let bytes = w.into_bytes();
        let got = fields(&bytes);
        assert_eq!(got[0], (1, Value::Varint(300)));
        assert_eq!(got[1], (2, Value::Fixed64(u64::MAX)));
        assert_eq!(got[2].1.str(), Ok("héllo"));
        assert_eq!(got[3].1.f32(), Ok(1.5));
        assert_eq!(fields(got[4].1.bytes().unwrap()), [(1, Value::Varint(5))]);
    }

    #[test]
    fn an_older_reader_skips_fields_it_does_not_know() {
        // A "newer" message: fields 1 and 3 are known, 2 and 9 are not.
        let mut w = Writer::new();
        w.varint(1, 7);
        w.bytes(2, b"from the future");
        w.fixed32(9, 1);
        w.str(3, "kept");
        let bytes = w.into_bytes();
        let (mut one, mut three) = (0, String::new());
        for field in Reader::new(&bytes) {
            let field = field.unwrap();
            match field.no {
                1 => one = field.value.u64().unwrap(),
                3 => three = field.value.str().unwrap().to_owned(),
                _ => {}
            }
        }
        assert_eq!((one, three.as_str()), (7, "kept"));
    }

    #[test]
    fn raw_is_the_whole_field_and_can_be_written_back() {
        let mut w = Writer::new();
        w.str(12, "x");
        w.varint(3, 1);
        let bytes = w.into_bytes();
        let mut copy = Writer::new();
        for field in Reader::new(&bytes) {
            copy.raw(field.unwrap().raw);
        }
        assert_eq!(copy.into_bytes(), bytes);
    }

    #[test]
    fn negative_zero_is_written_as_zero() {
        let (mut a, mut b) = (Writer::new(), Writer::new());
        a.f32(1, -0.0);
        b.f32(1, 0.0);
        assert_eq!(a.into_bytes(), b.into_bytes());
    }

    #[test]
    fn damaged_messages_are_errors() {
        let mut w = Writer::new();
        w.str(1, "abcdef");
        let bytes = w.into_bytes();
        let cut: Vec<_> = Reader::new(&bytes[..bytes.len() - 1]).collect();
        assert_eq!(cut, [Err(TlvError::Truncated)]);
        assert_eq!(Reader::new(&[(1 << 3) | 3]).next(), Some(Err(TlvError::BadWire(3))));
        assert_eq!(Reader::new(&[0]).next(), Some(Err(TlvError::BadField)));
    }
}
```

- [ ] **Step 2: Add `pub mod tlv;`** to `store/mod.rs`, above `pub mod varint;`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::tlv`
Expected: `5 passed`.

- [ ] **Step 4: Clippy** as in Task 1 Step 5: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/tlv.rs crates/manifold/src/store/mod.rs
git commit -m "store: TLV message reader and writer" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Data model, ops and merge

**Files:**
- Create: `crates/manifold/src/store/model.rs`
- Create: `crates/manifold/src/store/ops.rs`
- Modify: `crates/manifold/src/store/mod.rs` (add `pub mod model;` and `pub mod ops;` above `pub mod tlv;`)
- Modify: `crates/manifold/Cargo.toml` (add `getrandom = "0.3"` after `foldhash`)

One task because the two files need each other: `Doc` (model) keeps the move log as `Vec<MoveRec>` (ops), and `ops.rs` adds `Doc::apply`.

**Interfaces:**
- Consumes: `tlv::{Reader, Writer, TlvError}` (Task 3); `testutil::XorShift` (Task 1).
- Produces, in `model`:
  - `ActorId(pub u64)` with `random()`; `NodeId { actor: ActorId, counter: u64 }` with `Display` (`{actor:016x}-{counter}`) and `FromStr`; `DocId(pub [u8; 16])` with `random()`.
  - `Hlc { ms: u64, logical: u32 }`, `Stamp { hlc, actor }` (both `Ord`), `Clock { tick(now_ms) -> Hlc, observe(Hlc) }`, `now_ms() -> u64`.
  - `Kind::{Tab, Group, Note, Image, Pdf, Json, JsonLeaf, Download, Unknown(u64)}` with `code, from_code, name, from_name`; `ENGINES: [&str; 4]`.
  - `field::{POS, COLOR, GROUP, PARENT, URL, ENGINE, PINNED, NAME, FROM_JSON, TEXT, PATH, KEY, VALUE}` (1–13), `FIELDS`, `FieldDef { no, name, ty, explicit }`, `FieldType`, `RESERVED`, `field_def(no)`, `field_by_name(name)`.
  - `FieldValue::{Pos(f32, f32), Color(Option<u32>), Id(Option<NodeId>), Str(String), Bool(bool), Engine(u64)}` with `absent(ty)`; `encode_field(no, &FieldValue) -> Vec<u8>`; `decode_field(&FieldDef, &[u8]) -> Result<FieldValue, TlvError>`.
  - `Reg { stamp, enc }`; `Node { kind, fields: BTreeMap<u32, Reg>, group: Option<NodeId>, deleted }` with `placeholder, is_live, get(no) -> FieldValue, message`; `Doc { id, nodes: BTreeMap<NodeId, Node>, version: BTreeMap<ActorId, u64>, clock: Hlc, pub(crate) moves }` with `new, live, effective_group, is_ancestor`.
- Produces, in `ops`: `Op { id: NodeId, hlc: Hlc, body: OpBody }` with `stamp() -> Stamp`; `OpBody::{Create { node, kind, fields: Vec<u8> }, Set { node, field: u32, enc: Vec<u8> }, Move { node, group: Option<NodeId> }, Delete { node }}`; `pub(crate) struct MoveRec`; `Doc::apply(&Op) -> Vec<NodeId>` (the nodes it touched); `Doc::forget_moves()`.

Merge rules (spec "Semantics", amended by rulings 2 and 6): `Set` is last-writer-wins per `(node, field)` by stamp; `Delete` wins over everything and clears the registers; `Move` goes through the Kleppmann move log, sorted by stamp, undoing and redoing later moves when an earlier one arrives, and skipping a move that would make a cycle. A `Create`'s `fields` is a node message; its `group` field is applied as the node's first move.

- [ ] **Step 1: Add the dependency.** In `crates/manifold/Cargo.toml` `[dependencies]`, after the line `foldhash = "0.2"`, add (every dependency there carries a comment saying why):

```toml
# OS randomness for the session store's per-launch actor ids, document
# ids and file generations (`store::model`, `store::journal`).
getrandom = "0.3"
```

(`Cargo.lock` already has `getrandom 0.3.4` through other crates; nothing new is downloaded, but the lock file records the new direct dependency.)

- [ ] **Step 2: Create `store/model.rs`:**

```rust
//! The shared document's data model (spec "Data model"): ids, the hybrid
//! logical clock, node kinds, the field table, and nodes whose fields are
//! last-writer-wins registers holding their field's own TLV encoding.

use std::collections::BTreeMap;
use std::fmt;

use super::ops::MoveRec;
use super::tlv::{Reader, TlvError, Writer};

/// A replica's id for one launch: random, never persisted or reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(pub u64);

impl ActorId {
    pub fn random() -> Self {
        ActorId(getrandom::u64().expect("the OS random source failed"))
    }
}

/// A node or op id: the creating actor and its per-launch counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    pub actor: ActorId,
    pub counter: u64,
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}-{}", self.actor.0, self.counter)
    }
}

impl std::str::FromStr for NodeId {
    type Err = String;

    /// Parses exactly what `Display` prints.
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("not a node id: {s:?}");
        let (actor, counter) = s.split_once('-').ok_or_else(bad)?;
        let hex = actor.len() == 16 && actor.bytes().all(|b| b.is_ascii_hexdigit());
        let decimal = !counter.is_empty() && counter.bytes().all(|b| b.is_ascii_digit());
        if !hex || !decimal {
            return Err(bad());
        }
        let actor = u64::from_str_radix(actor, 16).map_err(|_| bad())?;
        let counter = counter.parse().map_err(|_| bad())?;
        Ok(NodeId { actor: ActorId(actor), counter })
    }
}

/// 128 random bits naming a document (spec "Document id"). Its text form
/// is left to sub-project 2, the first thing that shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocId(pub [u8; 16]);

impl DocId {
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random source failed");
        DocId(bytes)
    }
}

/// A hybrid logical clock reading: wall-clock milliseconds, then a counter
/// for events within one millisecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hlc {
    pub ms: u64,
    pub logical: u32,
}

/// What orders two writes: the HLC, then the writing actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub hlc: Hlc,
    pub actor: ActorId,
}

#[derive(Debug, Default)]
pub struct Clock {
    last: Hlc,
}

impl Clock {
    /// The next local event's time: never behind `now_ms` or any time seen.
    pub fn tick(&mut self, now_ms: u64) -> Hlc {
        let ms = now_ms.max(self.last.ms);
        self.last = if ms > self.last.ms {
            Hlc { ms, logical: 0 }
        } else {
            match self.last.logical.checked_add(1) {
                Some(logical) => Hlc { ms, logical },
                // 2^32 events in one millisecond: borrow the next one.
                None => Hlc { ms: ms + 1, logical: 0 },
            }
        };
        self.last
    }

    /// Moves the clock past a time read from the file or another replica.
    pub fn observe(&mut self, seen: Hlc) {
        self.last = self.last.max(seen);
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Tab,
    Group,
    Note,
    Image,
    Pdf,
    Json,
    JsonLeaf,
    Download,
    /// A kind a newer build wrote (kept, never shown), or `Unknown(0)` for
    /// a node an op named before its `Create` arrived.
    Unknown(u64),
}

const KINDS: [(Kind, u64, &str); 8] = [
    (Kind::Tab, 1, "tab"),
    (Kind::Group, 2, "group"),
    (Kind::Note, 3, "note"),
    (Kind::Image, 4, "image"),
    (Kind::Pdf, 5, "pdf"),
    (Kind::Json, 6, "json"),
    (Kind::JsonLeaf, 7, "json_leaf"),
    (Kind::Download, 8, "download"),
];

impl Kind {
    pub fn code(self) -> u64 {
        match self {
            Kind::Unknown(code) => code,
            known => KINDS.iter().find(|k| k.0 == known).map_or(0, |k| k.1),
        }
    }

    pub fn from_code(code: u64) -> Kind {
        KINDS.iter().find(|k| k.1 == code).map_or(Kind::Unknown(code), |k| k.0)
    }

    pub fn name(self) -> Option<&'static str> {
        KINDS.iter().find(|k| k.0 == self).map(|k| k.2)
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        KINDS.iter().find(|k| k.2 == name).map(|k| k.0)
    }
}

/// Engine codes for the `engine` field: code `n` is `ENGINES[n - 1]`,
/// spelled as `process_manager::engine_kind_serde` spells it.
pub const ENGINES: [&str; 4] = ["servo", "cef", "webkit", "webview2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Two `f32`: `{1: x, 2: y}`.
    Pos,
    /// `0xRRGGBB` as a varint; absent means untagged, so black is `Some(0)`.
    Color,
    /// A node id with its full actor: `{1: actor (fixed64), 2: counter}`.
    /// Never an actor-table index, which is local to one file and would
    /// break inside a field this build writes back without knowing it.
    Id,
    Str,
    Bool,
    /// An `ENGINES` code.
    Engine,
}

#[derive(Debug)]
pub struct FieldDef {
    pub no: u32,
    pub name: &'static str,
    pub ty: FieldType,
    /// Emitted only by named user actions, never by diffing (spec "Local
    /// edits become ops").
    pub explicit: bool,
}

/// Field numbers. Never reuse one; retired numbers go in `RESERVED`.
pub mod field {
    pub const POS: u32 = 1;
    pub const COLOR: u32 = 2;
    /// The tree edge: set by `Create` and changed only by `Move`.
    pub const GROUP: u32 = 3;
    pub const PARENT: u32 = 4;
    pub const URL: u32 = 5;
    pub const ENGINE: u32 = 6;
    pub const PINNED: u32 = 7;
    pub const NAME: u32 = 8;
    pub const FROM_JSON: u32 = 9;
    pub const TEXT: u32 = 10;
    pub const PATH: u32 = 11;
    pub const KEY: u32 = 12;
    pub const VALUE: u32 = 13;
}

pub static FIELDS: [FieldDef; 13] = [
    FieldDef { no: field::POS, name: "pos", ty: FieldType::Pos, explicit: true },
    FieldDef { no: field::COLOR, name: "color", ty: FieldType::Color, explicit: false },
    FieldDef { no: field::GROUP, name: "group", ty: FieldType::Id, explicit: false },
    FieldDef { no: field::PARENT, name: "parent", ty: FieldType::Id, explicit: false },
    FieldDef { no: field::URL, name: "url", ty: FieldType::Str, explicit: true },
    FieldDef { no: field::ENGINE, name: "engine", ty: FieldType::Engine, explicit: false },
    FieldDef { no: field::PINNED, name: "pinned", ty: FieldType::Bool, explicit: false },
    FieldDef { no: field::NAME, name: "name", ty: FieldType::Str, explicit: false },
    FieldDef { no: field::FROM_JSON, name: "from_json", ty: FieldType::Bool, explicit: false },
    FieldDef { no: field::TEXT, name: "text", ty: FieldType::Str, explicit: false },
    FieldDef { no: field::PATH, name: "path", ty: FieldType::Str, explicit: false },
    FieldDef { no: field::KEY, name: "key", ty: FieldType::Str, explicit: false },
    FieldDef { no: field::VALUE, name: "value", ty: FieldType::Str, explicit: false },
];

/// Retired field numbers, never to be reused (checked by a test).
pub const RESERVED: [u32; 0] = [];

pub fn field_def(no: u32) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.no == no)
}

pub fn field_by_name(name: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.name == name)
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Pos(f32, f32),
    Color(Option<u32>),
    Id(Option<NodeId>),
    Str(String),
    Bool(bool),
    Engine(u64),
}

impl FieldValue {
    /// What a missing field decodes to.
    pub fn absent(ty: FieldType) -> FieldValue {
        match ty {
            FieldType::Pos => FieldValue::Pos(0.0, 0.0),
            FieldType::Color => FieldValue::Color(None),
            FieldType::Id => FieldValue::Id(None),
            FieldType::Str => FieldValue::Str(String::new()),
            FieldType::Bool => FieldValue::Bool(false),
            FieldType::Engine => FieldValue::Engine(0),
        }
    }
}

/// `value` as field `no` of a node message, key and value; empty when it
/// holds what absence decodes to, so equal values encode equally.
pub fn encode_field(no: u32, value: &FieldValue) -> Vec<u8> {
    let mut w = Writer::new();
    match value {
        FieldValue::Pos(x, y) => {
            let mut m = Writer::new();
            if *x != 0.0 {
                m.f32(1, *x);
            }
            if *y != 0.0 {
                m.f32(2, *y);
            }
            if !m.is_empty() {
                w.message(no, &m);
            }
        }
        FieldValue::Color(Some(rgb)) => w.varint(no, u64::from(*rgb)),
        FieldValue::Id(Some(id)) => w.message(no, &id_message(*id)),
        FieldValue::Str(s) if !s.is_empty() => w.str(no, s),
        FieldValue::Bool(true) => w.varint(no, 1),
        FieldValue::Engine(code) if *code != 0 => w.varint(no, *code),
        _ => {}
    }
    w.into_bytes()
}

fn id_message(id: NodeId) -> Writer {
    let mut m = Writer::new();
    if id.actor.0 != 0 {
        m.fixed64(1, id.actor.0);
    }
    if id.counter != 0 {
        m.varint(2, id.counter);
    }
    m
}

fn decode_id(buf: &[u8]) -> Result<NodeId, TlvError> {
    let mut id = NodeId { actor: ActorId(0), counter: 0 };
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => id.actor = ActorId(field.value.fixed64()?),
            2 => id.counter = field.value.u64()?,
            _ => {}
        }
    }
    Ok(id)
}

/// Decodes `encode_field`'s output (or any encoding of field `def`; the
/// last occurrence wins).
pub fn decode_field(def: &FieldDef, encoded: &[u8]) -> Result<FieldValue, TlvError> {
    let mut value = FieldValue::absent(def.ty);
    for field in Reader::new(encoded) {
        let v = field?.value;
        value = match def.ty {
            FieldType::Pos => {
                let (mut x, mut y) = (0.0, 0.0);
                for f in Reader::new(v.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => x = f.value.f32()?,
                        2 => y = f.value.f32()?,
                        _ => {}
                    }
                }
                FieldValue::Pos(x, y)
            }
            FieldType::Color => FieldValue::Color(Some(v.u32()?)),
            FieldType::Id => FieldValue::Id(Some(decode_id(v.bytes()?)?)),
            FieldType::Str => FieldValue::Str(v.str()?.to_owned()),
            FieldType::Bool => FieldValue::Bool(v.bool()?),
            FieldType::Engine => FieldValue::Engine(v.u64()?),
        };
    }
    Ok(value)
}

/// A last-writer-wins register: the winning write's stamp, and its value
/// as `encode_field` encodes it (empty = absent).
#[derive(Debug, Clone, PartialEq)]
pub struct Reg {
    pub stamp: Stamp,
    pub enc: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    /// Every field but `group`, by number, including numbers this build
    /// doesn't know (kept raw so compaction writes them back).
    pub fields: BTreeMap<u32, Reg>,
    /// The tree edge, owned by the move log (`ops.rs`). It may name a
    /// deleted group; see `Doc::effective_group`.
    pub group: Option<NodeId>,
    pub deleted: bool,
}

impl Node {
    /// A node an op named before its `Create` arrived; shown once it does.
    pub fn placeholder() -> Node {
        Node { kind: Kind::Unknown(0), fields: BTreeMap::new(), group: None, deleted: false }
    }

    /// Shown in the graph: created, not deleted, and of a kind this build knows.
    pub fn is_live(&self) -> bool {
        !self.deleted && !matches!(self.kind, Kind::Unknown(_))
    }

    /// Field `no`'s value; its default when unset or undecodable.
    pub fn get(&self, no: u32) -> FieldValue {
        let def = field_def(no).expect("a field number from `field`");
        self.fields
            .get(&no)
            .and_then(|reg| decode_field(def, &reg.enc).ok())
            .unwrap_or_else(|| FieldValue::absent(def.ty))
    }

    /// This node as a plain node message, `group` included: what a
    /// `Create` carries.
    pub fn message(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let group = encode_field(field::GROUP, &FieldValue::Id(self.group));
        let mut group_written = false;
        for (&no, reg) in &self.fields {
            if no > field::GROUP && !group_written {
                w.raw(&group);
                group_written = true;
            }
            w.raw(&reg.enc);
        }
        if !group_written {
            w.raw(&group);
        }
        w.into_bytes()
    }
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub id: DocId,
    pub nodes: BTreeMap<NodeId, Node>,
    /// Per actor, one past the highest op counter applied.
    pub version: BTreeMap<ActorId, u64>,
    /// The latest HLC applied, so a reopened replica's clock starts past it.
    pub clock: Hlc,
    /// Applied moves in stamp order, for undo and redo (`ops.rs`).
    pub(crate) moves: Vec<MoveRec>,
}

impl Doc {
    pub fn new(id: DocId) -> Doc {
        Doc { id, nodes: BTreeMap::new(), version: BTreeMap::new(), clock: Hlc::default(), moves: Vec::new() }
    }

    /// The live nodes, in id order.
    pub fn live(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().filter(|(_, node)| node.is_live()).map(|(id, node)| (*id, node))
    }

    /// The group `id` is shown in: its own, or, when that group was
    /// deleted, its nearest live ancestor (spec "Semantics").
    pub fn effective_group(&self, id: NodeId) -> Option<NodeId> {
        let mut group = self.nodes.get(&id)?.group;
        for _ in 0..=self.nodes.len() {
            match group.and_then(|g| self.nodes.get(&g)) {
                Some(node) if node.deleted => group = node.group,
                _ => return group,
            }
        }
        unreachable!("the move log keeps the tree acyclic")
    }

    /// Whether `ancestor` is on the path from `of` to the root, `of`
    /// excluded. Walks deleted nodes too, so every replica answers alike.
    pub fn is_ancestor(&self, ancestor: NodeId, of: NodeId) -> bool {
        let mut cur = self.nodes.get(&of).and_then(|n| n.group);
        for _ in 0..=self.nodes.len() {
            match cur {
                Some(c) if c == ancestor => return true,
                Some(c) => cur = self.nodes.get(&c).and_then(|n| n.group),
                None => return false,
            }
        }
        true // a cycle; `apply_move` never makes one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_print_and_parse() {
        let id = NodeId { actor: ActorId(0x1f3a_9c00_0000_00ab), counter: 17 };
        assert_eq!(id.to_string(), "1f3a9c00000000ab-17");
        assert_eq!(id.to_string().parse(), Ok(id));
        for bad in ["", "tab-3", "1f3a9c00000000ab-", "1f3a9c00000000ab-+1", "+f3a9c00000000ab-1"] {
            assert!(bad.parse::<NodeId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_clock_never_goes_back() {
        let mut clock = Clock::default();
        assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 0 });
        assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 1 });
        assert_eq!(clock.tick(5), Hlc { ms: 10, logical: 2 });
        clock.observe(Hlc { ms: 20, logical: 4 });
        assert_eq!(clock.tick(15), Hlc { ms: 20, logical: 5 });
        clock.observe(Hlc { ms: 30, logical: u32::MAX });
        assert_eq!(clock.tick(0), Hlc { ms: 31, logical: 0 });
    }

    #[test]
    fn field_numbers_are_unique_and_never_reserved() {
        let mut seen = std::collections::BTreeSet::new();
        for def in &FIELDS {
            assert!(seen.insert(def.no), "field {} twice", def.no);
            assert!(!RESERVED.contains(&def.no), "field {} is reserved", def.no);
        }
    }

    #[test]
    fn values_round_trip_and_defaults_encode_to_nothing() {
        let id = NodeId { actor: ActorId(9), counter: 4 };
        let cases = [
            (field::POS, FieldValue::Pos(1.5, -2.0)),
            (field::POS, FieldValue::Pos(0.0, 3.0)),
            (field::COLOR, FieldValue::Color(Some(0))),
            (field::COLOR, FieldValue::Color(Some(0x12_34_56))),
            (field::PARENT, FieldValue::Id(Some(id))),
            (field::URL, FieldValue::Str("https://example.com/".into())),
            (field::PINNED, FieldValue::Bool(true)),
            (field::ENGINE, FieldValue::Engine(4)),
        ];
        for (no, value) in cases {
            let enc = encode_field(no, &value);
            assert!(!enc.is_empty(), "{value:?}");
            assert_eq!(decode_field(field_def(no).unwrap(), &enc), Ok(value));
        }
        for def in &FIELDS {
            assert!(encode_field(def.no, &FieldValue::absent(def.ty)).is_empty(), "{}", def.name);
            assert_eq!(decode_field(def, &[]), Ok(FieldValue::absent(def.ty)));
        }
        assert_eq!(encode_field(field::POS, &FieldValue::Pos(-0.0, -0.0)), Vec::<u8>::new());
    }

    #[test]
    fn kinds_have_codes_and_names() {
        for (kind, code, name) in KINDS {
            assert_eq!(Kind::from_code(code), kind);
            assert_eq!(Kind::from_name(name), Some(kind));
            assert_eq!((kind.code(), kind.name()), (code, Some(name)));
        }
        assert_eq!(Kind::from_code(40), Kind::Unknown(40));
        assert_eq!(Kind::Unknown(40).code(), 40);
    }
}
```

- [ ] **Step 3: Create `store/ops.rs`.** Its convergence test does not yet compare encoded snapshots; Task 5 adds that once `snapshot_bytes` exists.

```rust
//! Operations and their merge rules (spec "Operations and merge"). Any
//! set of ops applied in any order, any number of times, gives the same
//! document.

use super::model::{decode_field, field, field_def, Doc, FieldValue, Hlc, Kind, Node, NodeId, Reg, Stamp};
use super::tlv::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    /// `(actor, counter)` from the writer's shared counter. A `Create`'s op
    /// id is its own: the node's id was taken when the node was made.
    pub id: NodeId,
    pub hlc: Hlc,
    pub body: OpBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpBody {
    /// `fields` is a node message (`Node::message`), `group` included.
    Create { node: NodeId, kind: Kind, fields: Vec<u8> },
    /// `enc` is `encode_field(field, value)`; empty resets the field.
    Set { node: NodeId, field: u32, enc: Vec<u8> },
    Move { node: NodeId, group: Option<NodeId> },
    Delete { node: NodeId },
}

impl Op {
    pub fn stamp(&self) -> Stamp {
        Stamp { hlc: self.hlc, actor: self.id.actor }
    }
}

/// One entry of the move log (Kleppmann et al., "A highly-available move
/// operation for replicated trees").
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoveRec {
    stamp: Stamp,
    node: NodeId,
    new: Option<NodeId>,
    /// The node's group before this move, recorded when (re)applied.
    old: Option<NodeId>,
    /// `false` when applying it would have made a cycle.
    applied: bool,
}

impl Doc {
    /// Applies `op` and returns the nodes whose shown state may have changed.
    pub fn apply(&mut self, op: &Op) -> Vec<NodeId> {
        let stamp = op.stamp();
        let next = self.version.entry(op.id.actor).or_insert(0);
        *next = (*next).max(op.id.counter + 1);
        self.clock = self.clock.max(op.hlc);
        match &op.body {
            OpBody::Create { node, kind, fields } => {
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                entry.kind = *kind;
                let mut group = None;
                for field in Reader::new(fields).map_while(Result::ok) {
                    if field.no == field::GROUP {
                        let def = field_def(field::GROUP).expect("known");
                        if let Ok(FieldValue::Id(g)) = decode_field(def, field.raw) {
                            group = g;
                        }
                    } else if !entry.deleted {
                        set_reg(entry, field.no, stamp, field.raw.to_vec());
                    }
                }
                // The initial group is a move at the create's own time.
                let mut touched = vec![*node];
                touched.extend(self.apply_move(stamp, *node, group));
                touched
            }
            OpBody::Set { node, field: no, enc } => {
                if *no == field::GROUP {
                    return Vec::new(); // the tree edge changes only by `Move`
                }
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                set_reg(entry, *no, stamp, enc.clone());
                vec![*node]
            }
            OpBody::Move { node, group } => {
                self.nodes.entry(*node).or_insert_with(Node::placeholder);
                self.apply_move(stamp, *node, *group)
            }
            OpBody::Delete { node } => {
                let entry = self.nodes.entry(*node).or_insert_with(Node::placeholder);
                if entry.deleted {
                    return Vec::new();
                }
                // Delete wins: its fields are gone and later `Set`s are
                // ignored. It keeps its tree edge; see `apply_move`.
                entry.deleted = true;
                entry.fields.clear();
                // Its members now show at its nearest live ancestor.
                let mut touched = vec![*node];
                touched.extend(self.nodes.iter().filter(|(_, n)| n.group == Some(*node)).map(|(id, _)| *id));
                touched
            }
        }
    }

    /// Kleppmann's move: undo the logged moves that sort after this one,
    /// apply it, and redo them, each checked again for cycles. Deleted
    /// nodes move too, so every replica walks the same tree whatever it
    /// has deleted. In order (single-player), nothing is undone.
    fn apply_move(&mut self, stamp: Stamp, node: NodeId, group: Option<NodeId>) -> Vec<NodeId> {
        let at = self.moves.partition_point(|m| m.stamp < stamp);
        if self.moves.get(at).is_some_and(|m| m.stamp == stamp) {
            return Vec::new(); // already applied
        }
        for m in self.moves[at..].iter().rev() {
            if m.applied {
                if let Some(n) = self.nodes.get_mut(&m.node) {
                    n.group = m.old;
                }
            }
        }
        self.moves.insert(at, MoveRec { stamp, node, new: group, old: None, applied: false });
        let mut touched = Vec::new();
        for i in at..self.moves.len() {
            let MoveRec { node, new, .. } = self.moves[i];
            let old = self.nodes.get(&node).and_then(|n| n.group);
            let applied = new.is_none_or(|g| g != node && !self.is_ancestor(node, g));
            if applied {
                self.nodes.entry(node).or_insert_with(Node::placeholder).group = new;
            }
            self.moves[i].old = old;
            self.moves[i].applied = applied;
            touched.push(node);
        }
        touched
    }

    /// Drops the move log. Compaction does this: every op written after it
    /// is stamped later than everything in the file, so none is undone.
    pub fn forget_moves(&mut self) {
        self.moves.clear();
    }
}

fn set_reg(node: &mut Node, no: u32, stamp: Stamp, enc: Vec<u8>) {
    if node.fields.get(&no).is_none_or(|reg| reg.stamp < stamp) {
        node.fields.insert(no, Reg { stamp, enc });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{encode_field, ActorId, DocId};
    use crate::store::testutil::XorShift;

    fn id(actor: u64, counter: u64) -> NodeId {
        NodeId { actor: ActorId(actor), counter }
    }

    fn op(actor: u64, counter: u64, ms: u64, body: OpBody) -> Op {
        // Real stamps are unique per op (`Clock::tick`); `logical` keeps them so.
        Op { id: id(actor, counter), hlc: Hlc { ms, logical: counter as u32 }, body }
    }

    fn create(node: NodeId, kind: Kind, group: Option<NodeId>) -> OpBody {
        OpBody::Create { node, kind, fields: encode_field(field::GROUP, &FieldValue::Id(group)) }
    }

    fn name(node: NodeId, text: &str) -> OpBody {
        OpBody::Set { node, field: field::NAME, enc: encode_field(field::NAME, &FieldValue::Str(text.into())) }
    }

    fn apply_all(ops: &[Op]) -> Doc {
        let mut doc = Doc::new(DocId([0; 16]));
        for op in ops {
            doc.apply(op);
        }
        doc
    }

    #[test]
    fn the_later_write_wins_whatever_the_arrival_order() {
        let g = id(1, 0);
        let ops = [op(1, 0, 1, create(g, Kind::Group, None)), op(1, 1, 5, name(g, "late")), op(2, 0, 3, name(g, "early"))];
        for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
            let doc = apply_all(&order.map(|i| ops[i].clone()));
            assert_eq!(doc.nodes[&g].get(field::NAME), FieldValue::Str("late".into()));
        }
    }

    #[test]
    fn delete_wins_over_a_concurrent_set() {
        let t = id(1, 0);
        let doc = apply_all(&[op(1, 0, 1, create(t, Kind::Tab, None)), op(1, 1, 2, OpBody::Delete { node: t }), op(2, 0, 9, name(t, "x"))]);
        assert!(doc.nodes[&t].deleted && doc.nodes[&t].fields.is_empty());
        assert_eq!(doc.live().count(), 0);
    }

    #[test]
    fn a_move_into_its_own_subtree_is_a_no_op() {
        let (a, b) = (id(1, 0), id(1, 1));
        let doc = apply_all(&[
            op(1, 0, 1, create(a, Kind::Group, None)),
            op(1, 1, 1, create(b, Kind::Group, Some(a))),
            op(1, 2, 2, OpBody::Move { node: a, group: Some(b) }),
            op(1, 3, 3, OpBody::Move { node: a, group: Some(a) }),
        ]);
        assert_eq!((doc.nodes[&a].group, doc.nodes[&b].group), (None, Some(a)));
    }

    #[test]
    fn concurrent_moves_that_would_cycle_keep_the_earlier_one() {
        // Actor 1 puts A in B while actor 2 puts B in A. The later move
        // (actor 2's, ms 3) would close a cycle, so it is skipped, on
        // either arrival order.
        let (a, b) = (id(1, 0), id(1, 1));
        let base = [op(1, 0, 1, create(a, Kind::Group, None)), op(1, 1, 1, create(b, Kind::Group, None))];
        let one = op(1, 2, 2, OpBody::Move { node: a, group: Some(b) });
        let two = op(2, 0, 3, OpBody::Move { node: b, group: Some(a) });
        for ops in [[one.clone(), two.clone()], [two.clone(), one.clone()]] {
            let doc = apply_all(&[base.as_slice(), ops.as_slice()].concat());
            assert_eq!((doc.nodes[&a].group, doc.nodes[&b].group), (Some(b), None));
        }
    }

    #[test]
    fn a_node_moved_into_a_deleted_group_shows_at_its_nearest_live_ancestor() {
        let (outer, inner, tab) = (id(1, 0), id(1, 1), id(1, 2));
        let doc = apply_all(&[
            op(1, 0, 1, create(outer, Kind::Group, None)),
            op(1, 1, 1, create(inner, Kind::Group, Some(outer))),
            op(1, 2, 1, create(tab, Kind::Tab, None)),
            op(1, 3, 2, OpBody::Delete { node: inner }),
            op(2, 0, 3, OpBody::Move { node: tab, group: Some(inner) }),
        ]);
        assert_eq!(doc.nodes[&tab].group, Some(inner));
        assert_eq!(doc.effective_group(tab), Some(outer));
    }

    /// Random concurrent ops from three actors over a few shared nodes:
    /// field writes, moves that race into cycles, and deletes racing moves.
    fn random_ops(rng: &mut XorShift) -> Vec<Op> {
        let groups: Vec<NodeId> = (0..4).map(|c| id(1, c)).collect();
        let tabs: Vec<NodeId> = (4..8).map(|c| id(1, c)).collect();
        let mut counters = [100u64; 4];
        let mut next = |actor: u64, ms: u64, body: OpBody| {
            let counter = &mut counters[actor as usize];
            *counter += 1;
            // A distinct logical per op keeps every stamp unique.
            Op { id: id(actor, *counter), hlc: Hlc { ms, logical: *counter as u32 }, body }
        };
        let mut ops: Vec<Op> = groups.iter().map(|&g| next(1, 1, create(g, Kind::Group, None))).collect();
        ops.extend(tabs.iter().map(|&t| next(1, 1, create(t, Kind::Tab, Some(groups[0])))));
        let all: Vec<NodeId> = groups.iter().chain(&tabs).copied().collect();
        for _ in 0..60 {
            let actor = 1 + rng.below(3);
            let ms = 2 + rng.below(20);
            let node = all[rng.below(all.len() as u64) as usize];
            let body = match rng.below(10) {
                0 => OpBody::Delete { node },
                1..=4 => OpBody::Move { node, group: (rng.below(5) > 0).then(|| groups[rng.below(4) as usize]) },
                _ => name(node, &format!("n{}", rng.below(5))),
            };
            ops.push(next(actor, ms, body));
        }
        ops
    }

    #[test]
    fn replicas_converge_whatever_the_order() {
        let mut rng = XorShift(0xDEAD_BEEF);
        for _ in 0..50 {
            let ops = random_ops(&mut rng);
            let expected = apply_all(&ops);
            for id in expected.nodes.keys() {
                assert!(!expected.is_ancestor(*id, *id), "a cycle through {id}");
            }
            for _ in 0..20 {
                let mut shuffled = ops.clone();
                for i in (1..shuffled.len()).rev() {
                    shuffled.swap(i, rng.below(i as u64 + 1) as usize);
                }
                // Applying some ops twice changes nothing either.
                let again: Vec<Op> = shuffled.iter().take(10).cloned().collect();
                let doc = apply_all(&[shuffled, again].concat());
                assert_eq!(doc.nodes, expected.nodes);
                assert_eq!(doc.version, expected.version);
            }
        }
    }
}
```

- [ ] **Step 4: Add `pub mod model;` and `pub mod ops;`** to `store/mod.rs`, in that order, above `pub mod tlv;`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: `25 passed` (varint 7, crc32c 2, tlv 5, model 5, ops 6).

- [ ] **Step 6: Clippy** as in Task 1 Step 5: no output.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/Cargo.toml Cargo.lock crates/manifold/src/store/model.rs crates/manifold/src/store/ops.rs crates/manifold/src/store/mod.rs
git commit -m "store: ids, clock, field table, document and merge" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Journal

**Files:**
- Create: `crates/manifold/src/store/journal.rs`
- Modify: `crates/manifold/src/store/mod.rs` (add `pub mod journal;` above `pub mod model;`)
- Modify: `crates/manifold/src/store/ops.rs` (the convergence test also compares encoded snapshots)

**Interfaces:**
- Consumes: `crc32c`, `tlv`, `varint`, `model`, `ops` (Tasks 1–4); `testutil::temp_dir`.
- Produces:
  - Format: `MAGIC`, `FORMAT_VERSION = 1`, `frame::{ACTORS = 1, SNAPSHOT = 2, OPS = 3, LOCAL = 4}`, `encode_frame(out, ty, message)`, `scan(buf, base) -> Scan { frames: Vec<RawFrame { offset, size, ty, message }>, end, stop: Option<Stop> }`, `Stop::{Overrun, BadCrc, ZeroLength, ZeroType}`, `read_header(buf) -> Result<Header { version, generation, len }, JournalError>`, `snapshot_bytes(&Doc) -> Vec<u8>`.
  - `Journal::open(path, actor) -> Result<Journal, JournalError>` (creates an empty document if the file is missing); `Journal::read(bytes) -> Result<(Doc, Option<Vec<u8>>), JournalError>`; `doc()`, `local() -> Option<&[u8]>`, `actor()`, `read_only()`, `next_id() -> NodeId`, `begin() -> Result<Txn, JournalError>`, `changed_on_disk() -> bool`, `sync_if_due()`, `close()`, `save_copy(path)`.
  - `Txn { pub remote: Vec<NodeId>, .. }` with `doc()`, `next_id()`, `commit(self, bodies: Vec<OpBody>, local: Option<Vec<u8>>) -> Result<(), JournalError>`.
  - `JournalError::{Io(io::Error), NotAJournal, Corrupt(String), ReadOnly}` (`Display`).
  - `inspect(bytes) -> Result<Inspect { version, generation, file_len, frames: Vec<(offset, type, size)>, torn: Option<(usize, Stop)> }, JournalError>` for `--dump`/`--check`.

Message layouts are in the doc comments of `snapshot_message` and `ops_message`. The locking protocol (spec "Two instances on one profile"): every read and write happens under an exclusive lock on `session.mnf.lock`; `begin` takes it and catches up (re-reading from scratch when the generation changed), so ops are always stamped after everything in the file and the move log's undo path is never taken locally. `commit` appends (first cutting any torn tail with `set_len`), applies, and compacts when the frames since the snapshot exceed max(2 × snapshot, 64 KB). A file holding unknown frame types or op kinds, or a newer `format_version`, never compacts; a newer version opens read-only.

- [ ] **Step 1: Create `store/journal.rs`:**

```rust
//! The session file (spec "File and frames", "Journal"): a header, then
//! CRC-checked frames. Reading stops at the first damaged frame, a torn
//! tail, which the next append cuts off. Every read and write happens
//! under a lock on a separate `.lock` file, so two processes on one
//! profile take turns and each merges what the other appended.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::crc32c::crc32c;
use super::model::{now_ms, ActorId, Clock, Doc, DocId, Hlc, Kind, Node, NodeId, Reg, Stamp};
use super::ops::{Op, OpBody};
use super::tlv::{Reader, TlvError, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNF\0";
pub const FORMAT_VERSION: u64 = 1;

/// Frame types. 0 is invalid, so a zeroed region never parses.
pub mod frame {
    pub const ACTORS: u64 = 1;
    pub const SNAPSHOT: u64 = 2;
    pub const OPS: u64 = 3;
    pub const LOCAL: u64 = 4;
}

/// How long an append may wait for its fsync (spec "Journal").
const SYNC_DEBOUNCE: Duration = Duration::from_secs(1);

/// Frames appended since the last snapshot before compacting: twice the
/// snapshot, and at least this.
const COMPACT_MIN: u64 = 64 * 1024;

#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    /// No `MNF\0` magic, or the header is cut short.
    NotAJournal,
    /// A whole, CRC-valid frame whose contents don't decode: a bug.
    Corrupt(String),
    /// Written by a newer format version; this build only reads it.
    ReadOnly,
}

impl From<io::Error> for JournalError {
    fn from(err: io::Error) -> Self {
        JournalError::Io(err)
    }
}

impl From<TlvError> for JournalError {
    fn from(err: TlvError) -> Self {
        JournalError::Corrupt(err.to_string())
    }
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Io(err) => write!(f, "{err}"),
            JournalError::NotAJournal => write!(f, "not a manifold session file"),
            JournalError::Corrupt(what) => write!(f, "damaged session file: {what}"),
            JournalError::ReadOnly => write!(f, "written by a newer manifold; opened read-only"),
        }
    }
}

// ---- Frames -------------------------------------------------------------

/// Why `scan` stopped before the end of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The frame's length runs past the end of the file.
    Overrun,
    BadCrc,
    ZeroLength,
    ZeroType,
}

/// One whole frame found by `scan`.
#[derive(Debug, Clone, Copy)]
pub struct RawFrame<'a> {
    /// Offset of its first byte in the file.
    pub offset: usize,
    /// Bytes it takes, header included.
    pub size: usize,
    pub ty: u64,
    pub message: &'a [u8],
}

pub struct Scan<'a> {
    pub frames: Vec<RawFrame<'a>>,
    /// Where the whole frames end: the end of the file, or a torn tail.
    pub end: usize,
    /// Why reading stopped at `end`, when that isn't the end of the file.
    pub stop: Option<Stop>,
}

pub fn encode_frame(out: &mut Vec<u8>, ty: u64, message: &[u8]) {
    let mut body = Vec::with_capacity(message.len() + 1);
    varint::write(&mut body, ty);
    body.extend_from_slice(message);
    let len = u32::try_from(body.len()).expect("a frame under 4 GiB");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&frame_crc(len, &body).to_le_bytes());
    out.extend_from_slice(&body);
}

/// CRC-32C over `len ‖ frame_type ‖ message`, so a zeroed region fails.
fn frame_crc(len: u32, body: &[u8]) -> u32 {
    let mut input = Vec::with_capacity(4 + body.len());
    input.extend_from_slice(&len.to_le_bytes());
    input.extend_from_slice(body);
    crc32c(&input)
}

fn frame_at(buf: &[u8]) -> Result<(u64, &[u8], usize), Stop> {
    let word = |at: usize| buf.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    let (Some(len), Some(crc)) = (word(0), word(4)) else { return Err(Stop::Overrun) };
    let body = buf.get(8..8 + len as usize).ok_or(Stop::Overrun)?;
    if len == 0 {
        return Err(Stop::ZeroLength);
    }
    if frame_crc(len, body) != crc {
        return Err(Stop::BadCrc);
    }
    let (ty, n) = varint::read(body).map_err(|_| Stop::ZeroType)?;
    if ty == 0 {
        return Err(Stop::ZeroType);
    }
    Ok((ty, &body[n..], 8 + len as usize))
}

/// Splits `buf` (the file from offset `base` on) into whole frames.
pub fn scan(buf: &[u8], base: usize) -> Scan<'_> {
    let mut frames = Vec::new();
    let mut pos = 0;
    let stop = loop {
        if pos == buf.len() {
            break None;
        }
        match frame_at(&buf[pos..]) {
            Ok((ty, message, size)) => {
                frames.push(RawFrame { offset: base + pos, size, ty, message });
                pos += size;
            }
            Err(stop) => break Some(stop),
        }
    };
    Scan { frames, end: base + pos, stop }
}

// ---- Header ---------------------------------------------------------------

pub struct Header {
    pub version: u64,
    /// Random per compacted file: a changed generation means another
    /// process replaced the file, so it must be read again from the start.
    pub generation: u64,
    pub len: usize,
}

pub fn read_header(buf: &[u8]) -> Result<Header, JournalError> {
    if buf.get(..4) != Some(&MAGIC[..]) {
        return Err(JournalError::NotAJournal);
    }
    let (version, n) = varint::read(&buf[4..]).map_err(|_| JournalError::NotAJournal)?;
    let at = 4 + n;
    let generation = buf.get(at..at + 8).ok_or(JournalError::NotAJournal)?;
    Ok(Header { version, generation: u64::from_le_bytes(generation.try_into().expect("8 bytes")), len: at + 8 })
}

fn header_bytes(generation: u64) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    varint::write(&mut out, FORMAT_VERSION);
    out.extend_from_slice(&generation.to_le_bytes());
    out
}

// ---- Messages -------------------------------------------------------------

/// The file's actor table: frames name actors by their index here.
#[derive(Debug, Clone, Default)]
pub struct Actors {
    list: Vec<ActorId>,
    index: BTreeMap<ActorId, u64>,
}

impl Actors {
    fn push(&mut self, actor: ActorId) {
        if !self.index.contains_key(&actor) {
            self.index.insert(actor, self.list.len() as u64);
            self.list.push(actor);
        }
    }

    fn index(&self, actor: ActorId) -> u64 {
        self.index[&actor]
    }

    fn get(&self, index: u64) -> Result<ActorId, JournalError> {
        let bad = || JournalError::Corrupt(format!("actor index {index} not in the table"));
        self.list.get(usize::try_from(index).map_err(|_| bad())?).copied().ok_or_else(bad)
    }
}

fn actors_message(actors: &[ActorId]) -> Vec<u8> {
    let mut w = Writer::new();
    for actor in actors {
        w.fixed64(1, actor.0);
    }
    w.into_bytes()
}

fn decode_actors(buf: &[u8]) -> Result<Vec<ActorId>, JournalError> {
    let mut actors = Vec::new();
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 1 {
            actors.push(ActorId(field.value.fixed64()?));
        }
    }
    Ok(actors)
}

/// A node id in a frame's own structure: `{1: actor index, 2: counter}`.
fn id_message(id: NodeId, actors: &Actors) -> Writer {
    let mut m = Writer::new();
    let index = actors.index(id.actor);
    if index != 0 {
        m.varint(1, index);
    }
    if id.counter != 0 {
        m.varint(2, id.counter);
    }
    m
}

fn decode_id(buf: &[u8], actors: &Actors) -> Result<NodeId, JournalError> {
    let (mut index, mut counter) = (0, 0);
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => index = field.value.u64()?,
            2 => counter = field.value.u64()?,
            _ => {}
        }
    }
    Ok(NodeId { actor: actors.get(index)?, counter })
}

fn hlc_message(hlc: Hlc) -> Writer {
    let mut m = Writer::new();
    if hlc.ms != 0 {
        m.varint(1, hlc.ms);
    }
    if hlc.logical != 0 {
        m.varint(2, u64::from(hlc.logical));
    }
    m
}

fn decode_hlc(buf: &[u8]) -> Result<Hlc, JournalError> {
    let mut hlc = Hlc::default();
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => hlc.ms = field.value.u64()?,
            2 => hlc.logical = field.value.u32()?,
            _ => {}
        }
    }
    Ok(hlc)
}

/// `{1: ms, 2: logical, 3: actor index}`.
fn stamp_message(stamp: Stamp, actors: &Actors) -> Writer {
    let mut m = hlc_message(stamp.hlc);
    let index = actors.index(stamp.actor);
    if index != 0 {
        m.varint(3, index);
    }
    m
}

fn decode_stamp(buf: &[u8], actors: &Actors) -> Result<Stamp, JournalError> {
    let hlc = decode_hlc(buf)?;
    let mut index = 0;
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 3 {
            index = field.value.u64()?;
        }
    }
    Ok(Stamp { hlc, actor: actors.get(index)? })
}

/// Every actor `doc`'s structure names, sorted: a compacted file's table.
fn doc_actors(doc: &Doc) -> Actors {
    let mut set = BTreeSet::new();
    set.extend(doc.version.keys().copied());
    for (id, node) in &doc.nodes {
        set.insert(id.actor);
        set.extend(node.group.map(|g| g.actor));
        set.extend(node.fields.values().map(|reg| reg.stamp.actor));
    }
    let mut actors = Actors::default();
    for actor in set {
        actors.push(actor);
    }
    actors
}

/// Snapshot: `{1: doc id, 2: node*, 3: version*, 4: reserved for a
/// retained Move-log tail, 5: clock}`. A node is `{1: id, 2: kind,
/// 3: deleted, 4: group, 5: register*}` and a register `{1: field,
/// 2: stamp, 3: value encoding}`. Nodes and registers are in id and
/// field order, so equal documents encode to equal bytes.
fn snapshot_message(doc: &Doc, actors: &Actors) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(1, &doc.id.0);
    for (id, node) in &doc.nodes {
        let mut n = Writer::new();
        n.message(1, &id_message(*id, actors));
        if node.kind.code() != 0 {
            n.varint(2, node.kind.code());
        }
        if node.deleted {
            n.varint(3, 1);
        }
        if let Some(group) = node.group {
            n.message(4, &id_message(group, actors));
        }
        for (no, reg) in &node.fields {
            let mut r = Writer::new();
            r.varint(1, u64::from(*no));
            r.message(2, &stamp_message(reg.stamp, actors));
            if !reg.enc.is_empty() {
                r.bytes(3, &reg.enc);
            }
            n.message(5, &r);
        }
        w.message(2, &n);
    }
    for (actor, next) in &doc.version {
        let mut v = Writer::new();
        let index = actors.index(*actor);
        if index != 0 {
            v.varint(1, index);
        }
        if *next != 0 {
            v.varint(2, *next);
        }
        w.message(3, &v);
    }
    let clock = hlc_message(doc.clock);
    if !clock.is_empty() {
        w.message(5, &clock);
    }
    w.into_bytes()
}

fn decode_snapshot(buf: &[u8], actors: &Actors) -> Result<Doc, JournalError> {
    let mut doc = Doc::new(DocId([0; 16]));
    for field in Reader::new(buf) {
        let field = field?;
        match field.no {
            1 => {
                let bytes = field.value.bytes()?;
                doc.id = DocId(bytes.try_into().map_err(|_| JournalError::Corrupt("document id length".into()))?);
            }
            2 => {
                let mut id = None;
                let mut node = Node::placeholder();
                for f in Reader::new(field.value.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => id = Some(decode_id(f.value.bytes()?, actors)?),
                        2 => node.kind = Kind::from_code(f.value.u64()?),
                        3 => node.deleted = f.value.bool()?,
                        4 => node.group = Some(decode_id(f.value.bytes()?, actors)?),
                        5 => {
                            let (mut no, mut stamp, mut enc) = (0, None, Vec::new());
                            for r in Reader::new(f.value.bytes()?) {
                                let r = r?;
                                match r.no {
                                    1 => no = r.value.u32()?,
                                    2 => stamp = Some(decode_stamp(r.value.bytes()?, actors)?),
                                    3 => enc = r.value.bytes()?.to_vec(),
                                    _ => {}
                                }
                            }
                            let stamp = stamp.ok_or_else(|| JournalError::Corrupt("register without a stamp".into()))?;
                            node.fields.insert(no, Reg { stamp, enc });
                        }
                        _ => {}
                    }
                }
                let id = id.ok_or_else(|| JournalError::Corrupt("node without an id".into()))?;
                doc.nodes.insert(id, node);
            }
            3 => {
                let (mut index, mut next) = (0, 0);
                for f in Reader::new(field.value.bytes()?) {
                    let f = f?;
                    match f.no {
                        1 => index = f.value.u64()?,
                        2 => next = f.value.u64()?,
                        _ => {}
                    }
                }
                doc.version.insert(actors.get(index)?, next);
            }
            5 => doc.clock = decode_hlc(field.value.bytes()?)?,
            _ => {}
        }
    }
    Ok(doc)
}

/// Actors and snapshot frames for `doc`, as a compacted file holds them.
/// Equal documents give equal bytes.
pub fn snapshot_bytes(doc: &Doc) -> Vec<u8> {
    let actors = doc_actors(doc);
    let mut out = Vec::new();
    encode_frame(&mut out, frame::ACTORS, &actors_message(&actors.list));
    encode_frame(&mut out, frame::SNAPSHOT, &snapshot_message(doc, &actors));
    out
}

/// The actors an op names in its structure.
fn op_actors(op: &Op) -> Vec<ActorId> {
    let mut actors = vec![op.id.actor];
    match &op.body {
        OpBody::Create { node, .. } | OpBody::Set { node, .. } | OpBody::Delete { node } => actors.push(node.actor),
        OpBody::Move { node, group } => {
            actors.push(node.actor);
            actors.extend(group.map(|g| g.actor));
        }
    }
    actors
}

/// Ops: `{1: author's actor index, 2: op*}`. An op is `{1: counter delta,
/// 2: ms delta (both zigzag, from the previous op in the frame), 3:
/// logical, then one body field: 4 Create {1: node, 2: kind, 3: fields},
/// 5 Set {1: node, 2: field, 3: value encoding}, 6 Move {1: node, 2:
/// group}, 7 Delete {1: node}}`.
fn ops_message(ops: &[Op], actors: &Actors) -> Vec<u8> {
    let author = ops[0].id.actor;
    let mut w = Writer::new();
    let index = actors.index(author);
    if index != 0 {
        w.varint(1, index);
    }
    let (mut counter, mut ms) = (0u64, 0u64);
    for op in ops {
        assert_eq!(op.id.actor, author, "one author per ops frame");
        let mut o = Writer::new();
        let delta = |a: u64, b: u64| varint::zigzag(a.wrapping_sub(b) as i64);
        if delta(op.id.counter, counter) != 0 {
            o.varint(1, delta(op.id.counter, counter));
        }
        if delta(op.hlc.ms, ms) != 0 {
            o.varint(2, delta(op.hlc.ms, ms));
        }
        if op.hlc.logical != 0 {
            o.varint(3, u64::from(op.hlc.logical));
        }
        (counter, ms) = (op.id.counter, op.hlc.ms);
        let mut b = Writer::new();
        let field = match &op.body {
            OpBody::Create { node, kind, fields } => {
                b.message(1, &id_message(*node, actors));
                b.varint(2, kind.code());
                if !fields.is_empty() {
                    b.bytes(3, fields);
                }
                4
            }
            OpBody::Set { node, field, enc } => {
                b.message(1, &id_message(*node, actors));
                b.varint(2, u64::from(*field));
                if !enc.is_empty() {
                    b.bytes(3, enc);
                }
                5
            }
            OpBody::Move { node, group } => {
                b.message(1, &id_message(*node, actors));
                if let Some(group) = group {
                    b.message(2, &id_message(*group, actors));
                }
                6
            }
            OpBody::Delete { node } => {
                b.message(1, &id_message(*node, actors));
                7
            }
        };
        o.message(field, &b);
        w.message(2, &o);
    }
    w.into_bytes()
}

/// The ops in an ops frame, and whether it held an op kind this build
/// doesn't know (those are skipped; the file must then never compact).
fn decode_ops(buf: &[u8], actors: &Actors) -> Result<(Vec<Op>, bool), JournalError> {
    let mut author = 0;
    for field in Reader::new(buf) {
        let field = field?;
        if field.no == 1 {
            author = field.value.u64()?;
        }
    }
    let author = actors.get(author)?;
    let (mut ops, mut unknown) = (Vec::new(), false);
    let (mut counter, mut ms) = (0u64, 0u64);
    for field in Reader::new(buf) {
        let field = field?;
        if field.no != 2 {
            continue;
        }
        let (mut logical, mut body) = (0, None);
        let (mut counter_delta, mut ms_delta) = (0, 0);
        for f in Reader::new(field.value.bytes()?) {
            let f = f?;
            match f.no {
                1 => counter_delta = f.value.u64()?,
                2 => ms_delta = f.value.u64()?,
                3 => logical = f.value.u32()?,
                4..=7 => body = Some((f.no, f.value.bytes()?)),
                _ => unknown = true,
            }
        }
        counter = counter.wrapping_add(varint::unzigzag(counter_delta) as u64);
        ms = ms.wrapping_add(varint::unzigzag(ms_delta) as u64);
        let Some((kind, bytes)) = body else {
            unknown = true;
            continue;
        };
        let (mut node, mut num, mut blob, mut group) = (None, 0, Vec::new(), None);
        for f in Reader::new(bytes) {
            let f = f?;
            match (kind, f.no) {
                (_, 1) => node = Some(decode_id(f.value.bytes()?, actors)?),
                (4 | 5, 2) => num = f.value.u64()?,
                (6, 2) => group = Some(decode_id(f.value.bytes()?, actors)?),
                (4 | 5, 3) => blob = f.value.bytes()?.to_vec(),
                _ => {}
            }
        }
        let node = node.ok_or_else(|| JournalError::Corrupt("op without a node".into()))?;
        let body = match kind {
            4 => OpBody::Create { node, kind: Kind::from_code(num), fields: blob },
            5 => OpBody::Set {
                node,
                field: u32::try_from(num).map_err(|_| JournalError::Corrupt("field number".into()))?,
                enc: blob,
            },
            6 => OpBody::Move { node, group },
            _ => OpBody::Delete { node },
        };
        ops.push(Op { id: NodeId { actor: author, counter }, hlc: Hlc { ms, logical }, body });
    }
    Ok((ops, unknown))
}

// ---- The journal ----------------------------------------------------------

/// One replica's handle on a session file: the document as of the last
/// read, this launch's actor, clock and id counter.
pub struct Journal {
    path: PathBuf,
    actor: ActorId,
    clock: Clock,
    counter: u64,
    generation: u64,
    /// End of the last whole frame read: where the next append goes.
    end: u64,
    actors: Actors,
    doc: Doc,
    local: Option<Vec<u8>>,
    /// Size of the latest snapshot frame, and of the frames after it.
    snapshot_len: u64,
    since_snapshot: u64,
    /// A frame type, op kind or format version this build doesn't know
    /// was read: never compact, since that would drop it.
    keep_unknown: bool,
    read_only: bool,
    /// Length and modified time last seen, for `changed_on_disk`.
    seen: Option<(u64, SystemTime)>,
    last_sync: Instant,
    unsynced: bool,
}

/// Holds the profile lock; commit through it. Dropping it unlocks.
pub struct Txn<'a> {
    journal: &'a mut Journal,
    _lock: File,
    /// Nodes other processes changed since this replica last read.
    pub remote: Vec<NodeId>,
}

impl Journal {
    fn blank(path: &Path, actor: ActorId) -> Journal {
        Journal {
            path: path.to_path_buf(),
            actor,
            clock: Clock::default(),
            counter: 0,
            generation: 0,
            end: 0,
            actors: Actors::default(),
            doc: Doc::new(DocId([0; 16])),
            local: None,
            snapshot_len: 0,
            since_snapshot: 0,
            keep_unknown: false,
            read_only: false,
            seen: None,
            last_sync: Instant::now(),
            unsynced: false,
        }
    }

    /// Opens `path`, first creating an empty document there if there is
    /// none. `NotAJournal` and `Corrupt` leave the file as it was.
    pub fn open(path: &Path, actor: ActorId) -> Result<Journal, JournalError> {
        let mut journal = Journal::blank(path, actor);
        let _lock = journal.lock()?;
        if !path.exists() {
            write_compacted(path, &Doc::new(DocId::random()), None)?;
        }
        journal.load(&std::fs::read(path)?)?;
        journal.note_seen();
        Ok(journal)
    }

    /// Reads a session file's document and personal state, as `open`
    /// would, without a lock or an actor. For `--dump` and tests.
    pub fn read(bytes: &[u8]) -> Result<(Doc, Option<Vec<u8>>), JournalError> {
        let mut journal = Journal::blank(Path::new(""), ActorId(0));
        journal.load(bytes)?;
        Ok((journal.doc, journal.local))
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// The latest personal state (spec "Personal state"), as written.
    pub fn local(&self) -> Option<&[u8]> {
        self.local.as_deref()
    }

    pub fn actor(&self) -> ActorId {
        self.actor
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// A fresh id from this launch's counter, for a new node.
    pub fn next_id(&mut self) -> NodeId {
        let id = NodeId { actor: self.actor, counter: self.counter };
        self.counter += 1;
        id
    }

    fn lock(&self) -> Result<File, JournalError> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut lock_path = self.path.clone().into_os_string();
        lock_path.push(".lock");
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(lock_path)?;
        file.lock()?;
        Ok(file)
    }

    /// Takes the lock and reads what other processes appended since.
    pub fn begin(&mut self) -> Result<Txn<'_>, JournalError> {
        let lock = self.lock()?;
        let remote = self.catch_up()?;
        Ok(Txn { journal: self, _lock: lock, remote })
    }

    /// Cheap check, no lock: has the file changed since this replica last
    /// read or wrote it? If so, `begin` picks the changes up.
    pub fn changed_on_disk(&self) -> bool {
        file_seen(&self.path) != self.seen
    }

    fn note_seen(&mut self) {
        self.seen = file_seen(&self.path);
    }

    /// Replaces this replica's state with `bytes`, a whole file.
    fn load(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let header = read_header(bytes)?;
        self.generation = header.generation;
        self.read_only = header.version > FORMAT_VERSION;
        self.keep_unknown = self.read_only;
        self.actors = Actors::default();
        self.doc = Doc::new(DocId([0; 16]));
        self.local = None;
        self.snapshot_len = 0;
        self.since_snapshot = 0;
        let scan = scan(&bytes[header.len..], header.len);
        for frame in &scan.frames {
            self.take(frame)?;
        }
        self.end = scan.end as u64;
        Ok(())
    }

    /// Applies one frame read from the file.
    fn take(&mut self, frame: &RawFrame) -> Result<Vec<NodeId>, JournalError> {
        let size = frame.size as u64;
        match frame.ty {
            frame::SNAPSHOT => {
                (self.snapshot_len, self.since_snapshot) = (size, 0);
            }
            _ => self.since_snapshot += size,
        }
        match frame.ty {
            frame::ACTORS => {
                for actor in decode_actors(frame.message)? {
                    self.actors.push(actor);
                }
                Ok(Vec::new())
            }
            frame::SNAPSHOT => {
                let before: Vec<NodeId> = self.doc.nodes.keys().copied().collect();
                self.doc = decode_snapshot(frame.message, &self.actors)?;
                self.clock.observe(self.doc.clock);
                Ok(before.into_iter().chain(self.doc.nodes.keys().copied()).collect())
            }
            frame::OPS => {
                let (ops, unknown) = decode_ops(frame.message, &self.actors)?;
                self.keep_unknown |= unknown;
                let mut touched = Vec::new();
                for op in &ops {
                    self.clock.observe(op.hlc);
                    touched.extend(self.doc.apply(op));
                }
                Ok(touched)
            }
            frame::LOCAL => {
                self.local = Some(frame.message.to_vec());
                Ok(Vec::new())
            }
            _ => {
                self.keep_unknown = true; // a newer build's frame: kept, not read
                Ok(Vec::new())
            }
        }
    }

    /// Reads the frames other processes appended, or the whole file again
    /// if another process compacted it. Returns the nodes they touched.
    fn catch_up(&mut self) -> Result<Vec<NodeId>, JournalError> {
        let bytes = std::fs::read(&self.path)?;
        let header = read_header(&bytes)?;
        let touched = if header.generation != self.generation || (bytes.len() as u64) < self.end {
            let before: Vec<NodeId> = self.doc.nodes.keys().copied().collect();
            self.load(&bytes)?;
            before.into_iter().chain(self.doc.nodes.keys().copied()).collect()
        } else {
            let scan = scan(&bytes[self.end as usize..], self.end as usize);
            let mut touched = Vec::new();
            for frame in &scan.frames {
                touched.extend(self.take(frame)?);
            }
            self.end = scan.end as u64;
            touched
        };
        self.note_seen();
        let unique: BTreeSet<NodeId> = touched.into_iter().collect();
        Ok(unique.into_iter().collect())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), JournalError> {
        let mut file = OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(self.end)?; // cuts a torn tail
        file.seek(SeekFrom::Start(self.end))?;
        file.write_all(bytes)?;
        self.end += bytes.len() as u64;
        self.since_snapshot += bytes.len() as u64;
        if self.last_sync.elapsed() >= SYNC_DEBOUNCE {
            file.sync_data()?;
            self.last_sync = Instant::now();
            self.unsynced = false;
        } else {
            self.unsynced = true;
        }
        drop(file);
        self.note_seen();
        Ok(())
    }

    /// Flushes an append whose fsync was debounced, once it is due. Call it
    /// now and then (the app does, every frame it draws).
    pub fn sync_if_due(&mut self) -> Result<(), JournalError> {
        if self.unsynced && self.last_sync.elapsed() >= SYNC_DEBOUNCE {
            sync_data(&self.path)?;
            self.last_sync = Instant::now();
            self.unsynced = false;
        }
        Ok(())
    }

    fn should_compact(&self) -> bool {
        !self.keep_unknown && self.since_snapshot > (2 * self.snapshot_len).max(COMPACT_MIN)
    }

    /// Rewrites the file as one snapshot (spec "Compact"). Holds the lock.
    fn compact(&mut self) -> Result<(), JournalError> {
        self.doc.forget_moves();
        let bytes = write_compacted(&self.path, &self.doc, self.local.as_deref())?;
        let doc = std::mem::replace(&mut self.doc, Doc::new(DocId([0; 16])));
        self.load(&bytes)?;
        debug_assert_eq!(self.doc.nodes, doc.nodes);
        self.note_seen();
        self.unsynced = false;
        Ok(())
    }

    /// Clean exit: catch up, then compact, or just fsync when this build
    /// must not compact this file.
    pub fn close(&mut self) -> Result<(), JournalError> {
        if self.read_only {
            return Ok(());
        }
        let _lock = self.lock()?;
        self.catch_up()?;
        if self.keep_unknown {
            sync_data(&self.path)?;
            self.unsynced = false;
            Ok(())
        } else {
            self.compact()
        }
    }

    /// Writes this document, compacted, to `path` under a new document id
    /// (spec "Named sessions": Save As).
    pub fn save_copy(&self, path: &Path) -> Result<(), JournalError> {
        let mut doc = self.doc.clone();
        doc.id = DocId::random();
        doc.forget_moves();
        write_compacted(path, &doc, self.local.as_deref())?;
        Ok(())
    }
}

impl Txn<'_> {
    pub fn doc(&self) -> &Doc {
        &self.journal.doc
    }

    pub fn next_id(&mut self) -> NodeId {
        self.journal.next_id()
    }

    /// Stamps `bodies` as this replica's next ops, applies them, and
    /// appends them, and `local` when given, to the file.
    pub fn commit(self, bodies: Vec<OpBody>, local: Option<Vec<u8>>) -> Result<(), JournalError> {
        let Txn { journal: j, _lock, .. } = self;
        if j.read_only {
            return Err(JournalError::ReadOnly);
        }
        if bodies.is_empty() && local.is_none() {
            return Ok(());
        }
        let now = now_ms();
        let ops: Vec<Op> = bodies
            .into_iter()
            .map(|body| {
                let id = j.next_id();
                Op { id, hlc: j.clock.tick(now), body }
            })
            .collect();
        let mut out = Vec::new();
        let mut new_actors: Vec<ActorId> = Vec::new();
        for actor in ops.iter().flat_map(op_actors) {
            if !j.actors.index.contains_key(&actor) && !new_actors.contains(&actor) {
                new_actors.push(actor);
            }
        }
        if !new_actors.is_empty() {
            encode_frame(&mut out, frame::ACTORS, &actors_message(&new_actors));
            for actor in new_actors {
                j.actors.push(actor);
            }
        }
        if !ops.is_empty() {
            encode_frame(&mut out, frame::OPS, &ops_message(&ops, &j.actors));
        }
        if let Some(local) = &local {
            encode_frame(&mut out, frame::LOCAL, local);
        }
        j.append(&out)?;
        for op in &ops {
            j.doc.apply(op);
        }
        if local.is_some() {
            j.local = local;
        }
        if j.should_compact() {
            j.compact()?;
        }
        Ok(())
    }
}

/// fsyncs `path`. Opened for writing: on Windows, `FlushFileBuffers` on a
/// read-only handle fails with "Access is denied".
fn sync_data(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_data()
}

fn file_seen(path: &Path) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Writes a compacted file (header, actors, snapshot, local) beside
/// `path`, fsyncs it, and renames it over `path`. Returns its bytes.
fn write_compacted(path: &Path, doc: &Doc, local: Option<&[u8]>) -> Result<Vec<u8>, JournalError> {
    let mut bytes = header_bytes(getrandom::u64().expect("the OS random source failed"));
    bytes.extend(snapshot_bytes(doc));
    if let Some(local) = local {
        encode_frame(&mut bytes, frame::LOCAL, local);
    }
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(bytes)
}

// ---- Inspection (`--dump`, `--check`) --------------------------------------

pub struct Inspect {
    pub version: u64,
    pub generation: u64,
    pub file_len: usize,
    /// `(offset, type, size)` of every whole frame.
    pub frames: Vec<(usize, u64, usize)>,
    /// Where a torn tail starts, and why reading stopped there.
    pub torn: Option<(usize, Stop)>,
}

pub fn inspect(bytes: &[u8]) -> Result<Inspect, JournalError> {
    let header = read_header(bytes)?;
    let scan = scan(&bytes[header.len..], header.len);
    Ok(Inspect {
        version: header.version,
        generation: header.generation,
        file_len: bytes.len(),
        frames: scan.frames.iter().map(|f| (f.offset, f.ty, f.size)).collect(),
        torn: scan.stop.map(|stop| (scan.end, stop)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{encode_field, field, FieldValue};
    use crate::store::testutil::temp_dir;

    fn create_tab(txn: &mut Txn, url: &str) -> (NodeId, OpBody) {
        let node = txn.next_id();
        let fields = encode_field(field::URL, &FieldValue::Str(url.into()));
        (node, OpBody::Create { node, kind: Kind::Tab, fields })
    }

    fn url(doc: &Doc, id: NodeId) -> FieldValue {
        doc.nodes[&id].get(field::URL)
    }

    #[test]
    fn frames_round_trip_and_damage_stops_the_scan() {
        let mut bytes = Vec::new();
        encode_frame(&mut bytes, frame::LOCAL, b"one");
        encode_frame(&mut bytes, frame::OPS, b"");
        let scan1 = scan(&bytes, 0);
        assert_eq!(scan1.frames.len(), 2);
        assert_eq!((scan1.frames[0].ty, scan1.frames[0].message), (frame::LOCAL, &b"one"[..]));
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
            txn.commit(vec![create], Some(vec![i])).unwrap();
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
            let (doc, _) = Journal::read(&full[..cut]).unwrap();
            assert_eq!(doc.live().count(), whole_ops, "cut at {cut}");
        }
    }

    #[test]
    fn reopening_gives_back_the_document_and_local_state() {
        let dir = temp_dir("reopen");
        let path = dir.join("session.mnf");
        let mut journal = Journal::open(&path, ActorId(1)).unwrap();
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://a.example/");
        txn.commit(vec![create], Some(b"view".to_vec())).unwrap();
        let doc_id = journal.doc().id;
        drop(journal);

        let mut reopened = Journal::open(&path, ActorId(2)).unwrap();
        assert_eq!(reopened.doc().id, doc_id);
        assert_eq!(url(reopened.doc(), tab), FieldValue::Str("https://a.example/".into()));
        assert_eq!(reopened.local(), Some(&b"view"[..]));
        // The new replica's ops are stamped after everything in the file.
        let seen = reopened.doc().clock;
        let txn = reopened.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }], None).unwrap();
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
        let mut txn = journal.begin().unwrap();
        let (tab, create) = create_tab(&mut txn, "https://b.example/");
        txn.commit(vec![create], None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        let scanned = scan(&bytes[header.len..], header.len);
        assert_eq!((scanned.end, scanned.stop), (bytes.len(), None));
        // The new frames start where the half frame did: it was cut.
        assert!(scanned.frames.iter().any(|f| f.offset as u64 == clean_len));
        let (doc, _) = Journal::read(&bytes).unwrap();
        assert!(doc.nodes[&tab].is_live());
    }

    #[test]
    fn two_replicas_on_one_file_merge_each_others_ops() {
        let dir = temp_dir("two");
        let path = dir.join("session.mnf");
        let mut a = Journal::open(&path, ActorId(1)).unwrap();
        let mut b = Journal::open(&path, ActorId(2)).unwrap();

        let mut txn = a.begin().unwrap();
        let group = txn.next_id();
        txn.commit(vec![OpBody::Create { node: group, kind: Kind::Group, fields: Vec::new() }], None).unwrap();
        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&group));
        let (tab, create) = create_tab(&mut txn, "https://c.example/");
        txn.commit(vec![create, OpBody::Move { node: tab, group: Some(group) }], None).unwrap();

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
        txn.commit(vec![create], None).unwrap();
        a.close().unwrap(); // compacts while `b` has the file open

        let mut txn = b.begin().unwrap();
        assert!(txn.remote.contains(&one));
        let (two, create) = create_tab(&mut txn, "https://two.example/");
        txn.commit(vec![create], None).unwrap();

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
        txn.commit(vec![create], None).unwrap();
        let long = "x".repeat(1000);
        for i in 0..200 {
            let text = FieldValue::Str(format!("{i}{long}"));
            let txn = journal.begin().unwrap();
            txn.commit(vec![OpBody::Set { node: tab, field: field::URL, enc: encode_field(field::URL, &text) }], None).unwrap();
        }
        // 200 KB of appends against a tiny snapshot: compacted at least once.
        assert!(std::fs::metadata(&path).unwrap().len() < 100 * 1024);
        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
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
        txn.commit(vec![create, OpBody::Set { node: tab, field: 99, enc: future.clone() }], None).unwrap();
        journal.close().unwrap(); // compacts: the unknown field is written back

        let (doc, _) = Journal::read(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc.nodes[&tab].fields[&99].enc, future);

        // A frame type from the future: kept, and the file never compacts.
        let mut frame = Vec::new();
        encode_frame(&mut frame, 50, b"new");
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&frame).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        let txn = journal.begin().unwrap();
        txn.commit(vec![OpBody::Delete { node: tab }], None).unwrap();
        journal.close().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let header = read_header(&bytes).unwrap();
        assert!(scan(&bytes[header.len..], header.len).frames.iter().any(|f| f.ty == 50));
    }

    #[test]
    fn a_newer_format_version_opens_read_only() {
        let dir = temp_dir("version");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4] = 2; // format_version 2
        std::fs::write(&path, &bytes).unwrap();
        let mut journal = Journal::open(&path, ActorId(2)).unwrap();
        assert!(journal.read_only());
        let txn = journal.begin().unwrap();
        assert!(matches!(txn.commit(vec![], Some(vec![1])), Err(JournalError::ReadOnly)));
        journal.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
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
    fn inspect_reports_frames_and_the_torn_tail() {
        let dir = temp_dir("inspect");
        let path = dir.join("session.mnf");
        drop(Journal::open(&path, ActorId(1)).unwrap());
        let mut bytes = std::fs::read(&path).unwrap();
        let clean = bytes.len();
        bytes.extend_from_slice(&[0; 16]);
        let report = inspect(&bytes).unwrap();
        assert_eq!(report.frames.iter().map(|f| f.1).collect::<Vec<_>>(), [frame::ACTORS, frame::SNAPSHOT]);
        assert_eq!(report.torn, Some((clean, Stop::ZeroLength)));
    }
}
```

- [ ] **Step 2: Add `pub mod journal;`** to `store/mod.rs`, above `pub mod model;`.

- [ ] **Step 3: Compare encoded snapshots in the convergence test.** In `store/ops.rs`, test `replicas_converge_whatever_the_order`, replace

```rust
            for _ in 0..20 {
```

with

```rust
            let expected_bytes = crate::store::journal::snapshot_bytes(&expected);
            for _ in 0..20 {
```

and replace

```rust
                assert_eq!(doc.version, expected.version);
```

with

```rust
                assert_eq!(doc.version, expected.version);
                assert_eq!(crate::store::journal::snapshot_bytes(&doc), expected_bytes);
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: `36 passed` (journal adds 11).

- [ ] **Step 5: Clippy** as in Task 1 Step 5: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/store/journal.rs crates/manifold/src/store/mod.rs crates/manifold/src/store/ops.rs
git commit -m "store: journal file with frames, lock, catch-up and compaction" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: JSON snapshots

**Files:**
- Create: `crates/manifold/src/store/json.rs`
- Modify: `crates/manifold/src/store/mod.rs` (add `pub mod json;` between `pub mod journal;` and `pub mod model;`)

**Interfaces:**
- Consumes: `model`, `ops::OpBody` (Task 4).
- Produces: `JsonDoc { nodes: Vec<JsonNode>, local: Option<serde_json::Value> }`, `JsonNode { id: String, kind: String, fields: serde_json::Map }` (both serde); `doc_to_json(&Doc) -> JsonDoc` (live nodes, each under its effective group); `subtree_to_json(&Doc, root: NodeId) -> JsonDoc` (root first, root without `group`); `Imported { id, kind, fields: BTreeMap<u32, FieldValue> }`; `import(&JsonDoc, next_id: impl FnMut() -> NodeId) -> Result<Vec<Imported>, String>`; `creates(&[Imported]) -> Vec<OpBody>`.

JSON field values: `pos` `[x, y]`, `color` `"#rrggbb"`, ids as strings, strings, `true`, engine names (`"servo"`, `"cef"`, `"webkit"`, `"webview2"`). Defaults are left out. On import every node gets a fresh id; ids in the file are labels resolved within the file, so hand-written fixtures can use `"t1"`, `"g"`. An id naming no node in the file is kept only if it parses as a real `NodeId`. Unknown field names are ignored; an unknown kind, engine or malformed value is an error.

- [ ] **Step 1: Create `store/json.rs`:**

```rust
//! JSON snapshots (spec "JSON snapshots"): the document's live nodes
//! through serde, with no history. Session and group export, probes'
//! `EXPORTSESSION`, and `--import-json` all use this one shape:
//!
//! `{"nodes": [{"id": "…", "kind": "tab", "url": "…", "pos": [x, y]}, …],
//!   "local": {…}}`
//!
//! Importing gives every node a fresh id, so a file can be imported twice.
//! Ids in the file are labels: any string, resolved within the file.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::model::{encode_field, field, field_by_name, field_def, Doc, FieldType, FieldValue, Kind, Node, NodeId, ENGINES};
use super::ops::OpBody;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JsonDoc {
    pub nodes: Vec<JsonNode>,
    /// Personal state, when the export includes it (spec "Personal state").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonNode {
    pub id: String,
    pub kind: String,
    /// Fields by name; defaults are left out.
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

fn value_to_json(value: &FieldValue) -> Option<Value> {
    Some(match value {
        FieldValue::Pos(x, y) if (*x, *y) != (0.0, 0.0) => serde_json::json!([x, y]),
        FieldValue::Color(Some(rgb)) => Value::String(format!("#{rgb:06x}")),
        FieldValue::Id(Some(id)) => Value::String(id.to_string()),
        FieldValue::Str(s) if !s.is_empty() => Value::String(s.clone()),
        FieldValue::Bool(true) => Value::Bool(true),
        FieldValue::Engine(code) if *code != 0 => Value::String(ENGINES.get(*code as usize - 1)?.to_string()),
        _ => return None,
    })
}

/// `node` as JSON, its group given separately (the effective one).
fn node_to_json(id: NodeId, node: &Node, group: Option<NodeId>) -> JsonNode {
    let mut fields = Map::new();
    let mut put = |no: u32, value: &FieldValue| {
        if let (Some(def), Some(json)) = (field_def(no), value_to_json(value)) {
            fields.insert(def.name.to_owned(), json);
        }
    };
    for &no in node.fields.keys() {
        if field_def(no).is_some() && no != field::GROUP {
            put(no, &node.get(no));
        }
    }
    put(field::GROUP, &FieldValue::Id(group));
    let kind = node.kind.name().expect("live nodes have known kinds").to_owned();
    JsonNode { id: id.to_string(), kind, fields }
}

/// Every live node, in id order, each under its effective group.
pub fn doc_to_json(doc: &Doc) -> JsonDoc {
    let nodes = doc.live().map(|(id, node)| node_to_json(id, node, doc.effective_group(id))).collect();
    JsonDoc { nodes, local: None }
}

/// `root` and its live subtree, root first (a group export). The root
/// is written without a group, since its group isn't exported.
pub fn subtree_to_json(doc: &Doc, root: NodeId) -> JsonDoc {
    let mut nodes = Vec::new();
    if let Some(node) = doc.nodes.get(&root).filter(|n| n.is_live()) {
        nodes.push(node_to_json(root, node, None));
    }
    for (id, node) in doc.live() {
        if id != root && doc.is_ancestor(root, id) {
            nodes.push(node_to_json(id, node, doc.effective_group(id)));
        }
    }
    JsonDoc { nodes, local: None }
}

/// A node read from JSON, with its fresh id and field values.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub id: NodeId,
    pub kind: Kind,
    pub fields: BTreeMap<u32, FieldValue>,
}

/// Reads `json`'s nodes, giving each a fresh id from `next_id`. Ids
/// naming a node in the file are rewritten to its fresh id; others are
/// kept when they are real node ids and dropped otherwise. Unknown field
/// names are ignored (a newer build's export); a bad value is an error.
pub fn import(json: &JsonDoc, mut next_id: impl FnMut() -> NodeId) -> Result<Vec<Imported>, String> {
    let mut fresh = HashMap::new();
    for node in &json.nodes {
        if fresh.insert(node.id.as_str(), next_id()).is_some() {
            return Err(format!("node id {:?} appears twice", node.id));
        }
    }
    let mut out = Vec::new();
    for node in &json.nodes {
        let kind = Kind::from_name(&node.kind).ok_or_else(|| format!("unknown kind {:?}", node.kind))?;
        let mut fields = BTreeMap::new();
        for (name, value) in &node.fields {
            let Some(def) = field_by_name(name) else { continue };
            let bad = || format!("node {:?}: bad {name} {value}", node.id);
            let parsed = match (def.ty, value) {
                (FieldType::Pos, Value::Array(xy)) if xy.len() == 2 => {
                    let get = |v: &Value| v.as_f64().map(|f| f as f32).filter(|f| f.is_finite());
                    FieldValue::Pos(get(&xy[0]).ok_or_else(bad)?, get(&xy[1]).ok_or_else(bad)?)
                }
                (FieldType::Color, Value::String(s)) => {
                    let hex = s.strip_prefix('#').filter(|h| h.len() == 6).ok_or_else(bad)?;
                    FieldValue::Color(Some(u32::from_str_radix(hex, 16).map_err(|_| bad())?))
                }
                (FieldType::Id, Value::String(s)) => match fresh.get(s.as_str()) {
                    Some(id) => FieldValue::Id(Some(*id)),
                    None => FieldValue::Id(s.parse().ok()),
                },
                (FieldType::Str, Value::String(s)) => FieldValue::Str(s.clone()),
                (FieldType::Bool, Value::Bool(b)) => FieldValue::Bool(*b),
                (FieldType::Engine, Value::String(s)) => {
                    let code = ENGINES.iter().position(|e| e == s).ok_or_else(bad)?;
                    FieldValue::Engine(code as u64 + 1)
                }
                _ => return Err(bad()),
            };
            fields.insert(def.no, parsed);
        }
        out.push(Imported { id: fresh[node.id.as_str()], kind, fields });
    }
    Ok(out)
}

/// The `Create` ops that add `imported` to a document.
pub fn creates(imported: &[Imported]) -> Vec<OpBody> {
    imported
        .iter()
        .map(|node| {
            let fields = node.fields.iter().flat_map(|(no, value)| encode_field(*no, value)).collect();
            OpBody::Create { node: node.id, kind: node.kind, fields }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, DocId, Hlc};
    use crate::store::ops::Op;

    fn ids(actor: u64) -> impl FnMut() -> NodeId {
        let mut counter = 0;
        move || {
            counter += 1;
            NodeId { actor: ActorId(actor), counter }
        }
    }

    fn doc_from(json: &str, actor: u64) -> Doc {
        let parsed: JsonDoc = serde_json::from_str(json).unwrap();
        let mut doc = Doc::new(DocId([0; 16]));
        let mut op_ids = ids(actor + 100);
        for (i, body) in creates(&import(&parsed, ids(actor)).unwrap()).into_iter().enumerate() {
            doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
        }
        doc
    }

    const HAND_WRITTEN: &str = r##"{"nodes": [
        {"id": "g", "kind": "group", "name": "Reading", "color": "#000000"},
        {"id": "t1", "kind": "tab", "group": "g", "url": "https://a.example/", "pos": [10.5, -3], "engine": "cef", "pinned": true},
        {"id": "t2", "kind": "tab", "group": "g", "parent": "t1", "url": "https://b.example/", "future_field": 7},
        {"id": "n", "kind": "note", "text": "hi"}
    ]}"##;

    #[test]
    fn hand_written_short_ids_resolve_within_the_file() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let find = |url: &str| doc.live().find(|(_, n)| n.get(field::URL) == FieldValue::Str(url.into())).unwrap();
        let (t1, tab1) = find("https://a.example/");
        let (_, tab2) = find("https://b.example/");
        let group = tab1.group.unwrap();
        assert_eq!(doc.nodes[&group].get(field::NAME), FieldValue::Str("Reading".into()));
        assert_eq!(tab2.group, Some(group));
        assert_eq!(tab2.get(field::PARENT), FieldValue::Id(Some(t1)));
        assert_eq!(tab1.get(field::POS), FieldValue::Pos(10.5, -3.0));
        assert_eq!(tab1.get(field::ENGINE), FieldValue::Engine(2));
        assert_eq!(tab1.get(field::PINNED), FieldValue::Bool(true));
    }

    #[test]
    fn black_is_a_color_not_the_absence_of_one() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let (_, group) = doc.live().find(|(_, n)| n.kind == Kind::Group).unwrap();
        assert_eq!(group.get(field::COLOR), FieldValue::Color(Some(0)));
        let json = serde_json::to_value(doc_to_json(&doc)).unwrap();
        assert!(json["nodes"].as_array().unwrap().iter().any(|n| n["color"] == "#000000"));
    }

    #[test]
    fn export_then_import_gives_the_same_graph_under_fresh_ids() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let exported = serde_json::to_string(&doc_to_json(&doc)).unwrap();
        let again = doc_from(&exported, 2);
        // Same nodes, relabelled: exporting both with ids hidden matches.
        let shape = |doc: &Doc| {
            let json = doc_to_json(doc);
            let label: HashMap<String, usize> = json.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
            let mut out: Vec<String> = json
                .nodes
                .iter()
                .map(|n| {
                    let mut f = n.fields.clone();
                    for key in ["group", "parent"] {
                        if let Some(Value::String(id)) = f.get(key) {
                            f.insert(key.into(), Value::from(label[id]));
                        }
                    }
                    format!("{} {} {}", label[&n.id], n.kind, Value::Object(f))
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(shape(&doc), shape(&again));
        assert!(again.nodes.keys().all(|id| id.actor == ActorId(2)));
    }

    #[test]
    fn a_group_export_lists_the_group_first_then_its_subtree() {
        let doc = doc_from(HAND_WRITTEN, 1);
        let (group, _) = doc.live().find(|(_, n)| n.kind == Kind::Group).unwrap();
        let export = subtree_to_json(&doc, group);
        assert_eq!(export.nodes.iter().map(|n| n.kind.as_str()).collect::<Vec<_>>(), ["group", "tab", "tab"]);
        assert!(!export.nodes[0].fields.contains_key("group"));
    }

    #[test]
    fn bad_input_is_an_error() {
        let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1)).unwrap_err();
        bad(r#"{"nodes": [{"id": "a", "kind": "spaceship"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab", "engine": "gecko"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab", "color": "red"}]}"#);
        bad(r#"{"nodes": [{"id": "a", "kind": "tab"}, {"id": "a", "kind": "tab"}]}"#);
    }
}
```

- [ ] **Step 2: Add `pub mod json;`** to `store/mod.rs`, between `pub mod journal;` and `pub mod model;`. `store/mod.rs` now reads exactly:

```rust
//! The session store: a replicated operation log in a custom binary
//! format. Pure (no egui, no `App`); `session_sync` adapts it to the live
//! graph. See docs/superpowers/specs/2026-09-25-session-op-log-design.md.

// Nothing outside `store` uses it until the app integration lands.
#![allow(dead_code)]

pub mod crc32c;
pub mod journal;
pub mod json;
pub mod model;
pub mod ops;
pub mod tlv;
pub mod varint;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// Seeded xorshift64*, so randomized tests repeat without a dependency.
    pub struct XorShift(pub u64);

    impl XorShift {
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        pub fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A fresh, empty directory for one test, under the system temp dir.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -q -p manifold -j 12 store::`
Expected: `41 passed`.

- [ ] **Step 4: Clippy** as in Task 1 Step 5: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/json.rs crates/manifold/src/store/mod.rs
git commit -m "store: JSON snapshots and import with fresh ids" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Write the rulings into the spec

**Files:**
- Modify: `docs/superpowers/specs/2026-09-25-session-op-log-design.md`

The spec is the authority Plan B argues from, so it must say what the code does. Make these edits (find each passage by its quoted text):

- [ ] **Step 1: Op ids.** Replace "- An **op id** is also `(actor, counter)` from the same counter." with:

```markdown
- An **op id** is also `(actor, counter)` from the same counter. A
  `Create`'s op id differs from the node id it creates: the node id is
  drawn when the node is made, op ids when ops are stamped.
- Every op's stamp `(hlc, actor)` is unique (`Clock::tick`); merge
  order depends on it.
```

- [ ] **Step 2: `color` row.** In the field table, replace "`None` = untagged" with "`None` = untagged; black is `Some(0)` and is encoded".

- [ ] **Step 3: Delete and Move.** Replace "- **Delete**: tombstone; later `Set`/`Move` on it are ignored." with "- **Delete**: tombstone; its registers are cleared and later `Set`s on it are ignored. `Move`s still apply (below)." Replace "A `Move` of a tombstoned node is ignored; a `Move` of a live node into a tombstoned group applies, and display re-homes it." with:

```markdown
Moves apply to tombstoned nodes like any other, so whether a `Delete`
arrived first never changes the tree (ignoring them broke convergence:
undo and redo of later moves differed between replicas). A node in a
tombstoned group is displayed at its nearest live ancestor.
```

- [ ] **Step 4: Ids in field values.** Replace the bullet starting "- node id: nested `{1: actor index, 2: counter}`." (through "full 64-bit actor id.") with:

```markdown
- node id in a frame's own structure (op targets, `Move` groups,
  snapshot node keys and group edges, stamps): nested `{1: actor index,
  2: counter}`, the index into the file's `Actors` table, renumbered at
  compaction.
- node id inside a field value (`parent`): nested `{1: actor (fixed64),
  2: counter}`, the full actor. A field this build does not know is kept
  as raw bytes and could not be renumbered, so field values never hold
  file-local indexes. JSON (and sync later) also uses the full actor.
```

and in the next bullet replace "absent when untagged;" with "absent when untagged (black is `Some(0)`, present). A field is omitted only when it holds exactly what absence decodes to;".

- [ ] **Step 5: Header.** Replace the file grammar line `file  = magic "MNF\0" | format_version (varint) | frame*` with `file  = magic "MNF\0" | format_version (varint) | generation (u64 LE) | frame*`, and after the code block's closing fence add:

```markdown
- `generation` is random, new at each compaction: a reader holding an
  offset into the old file sees it change and re-reads from the start.
```

- [ ] **Step 6: File id.** In "Two instances on one profile", replace "if its file id differs from the one it read last (another instance compacted)" with "if its header's generation differs from the one it read last, or the file is shorter than its offset (another instance compacted)". In "Compact", replace "write `magic, Actors, Snapshot, Local` to a temp file" with "write a header with a new generation, then `Actors, Snapshot, Local`, to a temp file".

- [ ] **Step 7: fsync.** In "Append", after "`fsync` is debounced to ~1 s and forced on exit;", add " (on Windows the handle must be opened for writing, or `FlushFileBuffers` fails)".

- [ ] **Step 8: Check and commit.** Several quoted passages wrap across lines in the spec; match them across the line breaks. Then run `grep -n "tombstoned node is ignored\|magic, Actors\|its file$\|untagged;$" docs/superpowers/specs/2026-09-25-session-op-log-design.md`; expected: no output (each is a phrase the edits above remove).

```bash
git add docs/superpowers/specs/2026-09-25-session-op-log-design.md
git commit -m "Spec: rulings from planning the store core" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Outcome (executed 2026-09-25)

Landed on `dev` as 832da97..25e9dce: 46 `store::` tests, 81 in the crate, and clippy is clean. The code differs from this plan's blocks in two review-driven commits, and the repo is the source of truth:

- **a582397.**
  - A failed read or write marks the journal `stale`, and the next `begin` re-reads the whole file.
  - A frame that doesn't re-encode to its own bytes (an unknown structural field) blocks compaction.
  - An op counter of `u64::MAX` is rejected.
- **83639a5, 25e9dce.**
  - `effective_group` skips every non-live group, and never panics.
  - `remote` includes descendants (`Doc::with_descendants`).
  - A failed compaction no longer fails the commit.
  - `save_copy` refuses while stale (`JournalError::Stale`) and locks its target.
  - A non-finite `pos` decodes as absent.

### Contracts Plan B must honour
- **Actors and ids.** Give each `Journal::open` a fresh `ActorId::random()`, because the id counter restarts at 0 for every `Journal`.
- **Transactions.**
  - Diff and commit only inside a `Txn`, against `txn.doc()`.
  - After any `Err`, don't retry the same bodies. `begin` again and re-diff.
  - Don't trust `Journal::doc()` after an error until a `begin` succeeds.
- **In-flight spawns.** A tab whose spawn is in flight (restore, engine swap, crash recovery) has no live node. The diff must treat its id as present (`App::spawning`), or it deletes the node and can never recreate it.
- **The lock.** `begin` blocks on the lock and does I/O (a whole-file read, and maybe an fsync or a compaction), so keep a `Txn` short.
- **Read-only.** `read_only()` can become true mid-session, when a newer build compacts the file. Check it after `begin`.
- **Flushing.** Call `sync_if_due()` every frame and `close()` on a clean exit.
- **Personal state.** It is in `session.mnf.local` (format revision, Plan A′).
  - Call `read_local()` once after `open`. Treat `Err` or `None` as a fresh start.
  - Call `write_local()` only when the state changed: at most every 30 s, 2.5 s after a Recently Closed change, and at exit before `close()`.
  - Settled positions count as changed only past 1 px.
  - `save_copy(path, Some(state))` gives the copy its state.
- **Missing means deleted.** A node id in `remote` that is missing from `doc()` was deleted; compaction may have pruned it.
  - Never name a non-live node as the group or `parent` of a new op; use its effective group, or none.
  - After `begin`, apply the `remote` changes to the live graph before diffing, so a pruned node is never re-emitted.
- **Repairs.** After `open` and after each `begin`, call `take_recovery()`. When it returns `Some`, tell the user what was lost and where the original was saved (`saved`).
  - A commit returns `Err` while the damaged original can't be saved, rather than dropping edits silently; show this to the user.
  - A `BadCrc` tail in a file this build can't rewrite (`keep_unknown`/read-only) is reported again after each reload, so the app should not repeat an identical notice.
- **`remote`.** It already includes descendants. Code that applies ops outside the journal (sync, later) must call `with_descendants` itself.
- **Fields.**
  - `Node::get` panics on a field number not in the table, so pass only `field::*` constants.
  - Never emit a non-finite `pos`: `Writer::f32` only `debug_assert`s it.
- **`save_copy`.** Never pass the journal's own path.
- **JSON import.** It keeps ids that aren't in the file when they parse as real node ids, so an imported `parent` can dangle.

### Deferred, with reasons
- **`NodeId::from_str` is lenient.** It accepts uppercase hex and leading zeros. This only matters for hand-written JSON labels.
- **`Set.enc` isn't validated** as its own field's encoding. Replicas still converge. Fix it before signing, in sub-project 2.
- **Duplicate fields in a `Create`** are first-wins for registers, but the spec says last-wins. It's deterministic, and honest writers never produce them.
- **Unknown op kinds.** When skipped, the clock doesn't observe them. The file never compacts in that case, so only last-writer-wins ties are affected.
- **`Clock::tick` can overflow** at `ms + 1` when a crafted file holds `u64::MAX` ms.
- **Performance.**
  - `catch_up` reads the whole file on every save (incremental parsing, but a whole-file read). Measured 2026-09-26 (`save_path_costs`, release, file at its largest before compaction, best of 40, CPU 17–47% busy with another session's job): 300 tabs (139 KB) `begin` 0.1 ms, a one-op save 0.14 ms (1.0 ms with its fsync); 3000 tabs (1.5 MB) 0.75 ms and 0.38 ms (1.6 ms). Other costs at 300 / 3000 tabs: reload after another instance compacts 1.8 / 15 ms, compaction 4.6 / 24 ms, `open` 3 / 59 ms, `changed_on_disk` 16 / 26 µs. All fit on the UI thread; no change needed before Plan B.
  - The fsync debounce is leading-edge, so the first save after a quiet second fsyncs on the caller's thread.
- **Durability.** There's no directory fsync or `MOVEFILE_WRITE_THROUGH` after compaction's rename.
- **Test gaps.**
  - Compaction equivalence.
  - fsync failing after the write.
