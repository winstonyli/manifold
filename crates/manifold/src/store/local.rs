//! Personal state (spec "Personal state", "Personal-state file"):
//! `<session>.local`, beside the session file, replaced whole under the
//! session's lock. It holds the session's document id, so a file left
//! from another document is ignored.

use std::io;
use std::path::{Path, PathBuf};

use super::journal::{encode_frame, scan, with_suffix, write_synced, JournalError};
use super::model::DocId;
use super::tlv::{Reader, Writer};
use super::varint;

pub const MAGIC: [u8; 4] = *b"MNL\0";
pub const FORMAT_VERSION: u64 = 1;

/// The one frame type: `{1: document id, 2: state}`.
const STATE: u64 = 1;

pub fn path_for(session: &Path) -> PathBuf {
    with_suffix(session, ".local")
}

/// The state saved for document `doc`: `None` when there is none, when it
/// is another document's, or when a newer format version wrote it.
pub fn read(session: &Path, doc: DocId) -> Result<Option<Vec<u8>>, JournalError> {
    let bytes = match std::fs::read(path_for(session)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let damaged = || JournalError::Corrupt("personal state".into());
    if bytes.get(..4) != Some(&MAGIC[..]) {
        return Err(damaged());
    }
    let (version, n) = varint::read(&bytes[4..]).map_err(|_| damaged())?;
    if version > FORMAT_VERSION {
        return Ok(None);
    }
    let at = 4 + n;
    let scan = scan(&bytes[at..], at);
    if scan.stop.is_some() {
        return Err(damaged());
    }
    let mut state = None;
    for frame in scan.frames.iter().filter(|f| f.ty == STATE) {
        let (mut id, mut value) = (Vec::new(), Vec::new());
        for field in Reader::new(frame.message) {
            let field = field?;
            match field.no {
                1 => id = field.value.bytes()?.to_vec(),
                2 => value = field.value.bytes()?.to_vec(),
                _ => {}
            }
        }
        state = Some((id, value));
    }
    Ok(state.filter(|(id, _)| id[..] == doc.0[..]).map(|(_, value)| value))
}

/// Replaces the saved state. A newer format version's file is replaced
/// too: personal state is recomputable, and newer builds read older
/// versions. Call under the session's lock (`Journal::write_local` does).
pub fn write(session: &Path, doc: DocId, state: &[u8]) -> Result<(), JournalError> {
    let mut bytes = MAGIC.to_vec();
    varint::write(&mut bytes, FORMAT_VERSION);
    let mut m = Writer::new();
    m.bytes(1, &doc.0);
    if !state.is_empty() {
        m.bytes(2, state);
    }
    encode_frame(&mut bytes, STATE, &m.into_bytes());
    replace(&path_for(session), &bytes).map_err(JournalError::from)
}

/// Writes `bytes` to a temp file beside `path`, fsyncs it, and renames it
/// over `path`. A failure leaves `path` as it was. The temp file is named
/// per process: instances write the app's JSON files (`json_file::save`)
/// without a lock, and a shared name would let one rename the other's
/// half-written file.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, &format!(".{}.tmp", std::process::id()));
    let written = write_synced(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
