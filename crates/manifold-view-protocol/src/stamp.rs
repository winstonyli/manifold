//! The protocol number every exe that speaks the protocol carries on disk, and the reader that finds it without
//! running the exe. The record sits in a linker section of its own (`.mfproto`), and `MessageChannel::send_version`
//! reads the number out of it, so any exe that can send the version keeps the section (a linker drops an
//! unreferenced one on ELF) and the number on disk is the number on the wire.
//!
//! `host_protocol` reads the section table with a few small reads and never maps the file: a mapped exe can make a
//! rebuild's linker fail to truncate it. PE and ELF64 (little-endian) only. A Mach-O host would need its section
//! named `"__DATA,__mfproto"`; there is no macOS target.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Starts the record, so a section of the right name holding something else is not misread.
const TAG: [u8; 12] = *b"manifold-ver";
/// The section's name: 8 bytes, the most a PE section name holds.
const SECTION: [u8; 8] = *b".mfproto";
/// The record on disk: the tag, the version and its check (u32 LE each).
const STAMP_LEN: usize = 20;
/// Bounds on what a corrupt header can make the reader allocate: section headers, and bytes of ELF section names.
const MAX_SECTIONS: u64 = 1024;
const MAX_NAMES: u64 = 64 * 1024;

/// What an exe carries: `TAG`, then `version`, then `!version`, so a torn or stray record reads as none.
#[repr(C)]
pub(crate) struct Stamp {
    #[allow(dead_code, reason = "read from the file by `host_protocol`, never through the struct")]
    tag: [u8; 12],
    pub(crate) version: u32,
    #[allow(dead_code, reason = "read from the file by `host_protocol`, never through the struct")]
    check: u32,
}

const _: () = assert!(std::mem::size_of::<Stamp>() == STAMP_LEN);

/// This exe's record. `MessageChannel::send_version` reads it, which keeps it in the exe.
#[link_section = ".mfproto"]
pub(crate) static STAMP: Stamp = Stamp { tag: TAG, version: crate::PROTOCOL_VERSION, check: !crate::PROTOCOL_VERSION };

/// The protocol number the exe at `path` carries, or `None` when it carries none: no such file, a folder, not a PE
/// or ELF64 image, no `.mfproto` section, or a record whose tag or check disagrees. Never panics; reads a few small
/// pieces of the file.
pub fn host_protocol(path: &Path) -> Option<u32> {
    stamp_in(&mut File::open(path).ok()?)
}

fn stamp_in(image: &mut (impl Read + Seek)) -> Option<u32> {
    let head = read_at(image, 0, 64)?;
    let at = if head.starts_with(b"MZ") {
        pe_section(image, &head)?
    } else if head.starts_with(b"\x7fELF") {
        elf_section(image, &head)?
    } else {
        return None;
    };
    parse_stamp(&read_at(image, at, STAMP_LEN as u64)?)
}

fn read_at(image: &mut (impl Read + Seek), offset: u64, len: u64) -> Option<Vec<u8>> {
    let mut bytes = vec![0; usize::try_from(len).ok()?];
    image.seek(SeekFrom::Start(offset)).ok()?;
    image.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..)?.get(..2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..)?.get(..4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..)?.get(..8)?.try_into().ok()?))
}

/// The file offset of the `.mfproto` section of a PE image whose DOS header is `head`. The section table follows the
/// COFF header and the optional header, whose size the COFF header gives; its magic is never read.
fn pe_section(image: &mut (impl Read + Seek), head: &[u8]) -> Option<u64> {
    let pe = u64::from(u32_at(head, 0x3C)?);
    let coff = read_at(image, pe, 24)?;
    if coff.get(..4)? != b"PE\0\0".as_slice() {
        return None;
    }
    let count = u64::from(u16_at(&coff, 6)?);
    let optional = u64::from(u16_at(&coff, 20)?);
    if count > MAX_SECTIONS {
        return None;
    }
    let table = read_at(image, pe + 24 + optional, count * 40)?;
    let header = table.as_chunks::<40>().0.iter().find(|h| h[..8] == SECTION)?;
    u32_at(header, 20).map(u64::from)
}

/// The file offset of the `.mfproto` section of a 64-bit little-endian ELF image whose header is `head`, found by
/// name through the section-name table.
fn elf_section(image: &mut (impl Read + Seek), head: &[u8]) -> Option<u64> {
    if head.get(4) != Some(&2) || head.get(5) != Some(&1) {
        return None;
    }
    let table_at = u64_at(head, 0x28)?;
    if u16_at(head, 0x3A)? != 64 {
        return None;
    }
    let count = u64::from(u16_at(head, 0x3C)?);
    let names_index = u64::from(u16_at(head, 0x3E)?);
    if count > MAX_SECTIONS || names_index >= count {
        return None;
    }
    let table = read_at(image, table_at, count * 64)?;
    let names_header = table.as_chunks::<64>().0.get(usize::try_from(names_index).ok()?)?;
    let names_size = u64_at(names_header, 32)?;
    if names_size > MAX_NAMES {
        return None;
    }
    let names = read_at(image, u64_at(names_header, 24)?, names_size)?;
    let header = table.as_chunks::<64>().0.iter().find(|&h| {
        u32_at(h, 0)
            .and_then(|at| names.get(usize::try_from(at).ok()?..))
            .is_some_and(|name| name.starts_with(&SECTION) && name.get(8) == Some(&0))
    })?;
    u64_at(header, 24)
}

/// The version in a record whose tag and check agree.
fn parse_stamp(record: &[u8]) -> Option<u32> {
    let version = u32_at(record, 12)?;
    (record.get(..12)? == TAG.as_slice() && u32_at(record, 16)? == !version).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(bytes: &[u8]) -> Option<u32> {
        stamp_in(&mut Cursor::new(bytes))
    }

    fn stamp_bytes(version: u32, check: u32) -> Vec<u8> {
        [TAG.as_slice(), &version.to_le_bytes(), &check.to_le_bytes()].concat()
    }

    /// A minimal PE image: a DOS header pointing at a COFF header with no optional header, then one 40-byte section
    /// header per section, then the sections' bytes in order.
    fn pe(sections: &[(&[u8; 8], &[u8])]) -> Vec<u8> {
        let mut image = vec![0u8; 0x40];
        image[..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image.extend_from_slice(b"PE\0\0");
        image.extend_from_slice(&0x8664u16.to_le_bytes()); // Machine: x64
        image.extend_from_slice(&(sections.len() as u16).to_le_bytes());
        image.extend_from_slice(&[0; 12]); // TimeDateStamp, PointerToSymbolTable, NumberOfSymbols
        image.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
        image.extend_from_slice(&0u16.to_le_bytes()); // Characteristics
        let mut data_at = image.len() + 40 * sections.len();
        for (name, data) in sections {
            let mut header = [0u8; 40];
            header[..8].copy_from_slice(*name);
            header[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes()); // SizeOfRawData
            header[20..24].copy_from_slice(&(data_at as u32).to_le_bytes()); // PointerToRawData
            image.extend_from_slice(&header);
            data_at += data.len();
        }
        for (_, data) in sections {
            image.extend_from_slice(data);
        }
        image
    }

    /// A minimal ELF64 little-endian image: the 64-byte header, the sections' bytes, the section-name table, then the
    /// section header table (a null header, one per section, then `.shstrtab`'s).
    fn elf(sections: &[(&str, &[u8])]) -> Vec<u8> {
        fn section_header(name: u32, offset: usize, size: usize) -> [u8; 64] {
            let mut header = [0u8; 64];
            header[..4].copy_from_slice(&name.to_le_bytes());
            header[24..32].copy_from_slice(&(offset as u64).to_le_bytes());
            header[32..40].copy_from_slice(&(size as u64).to_le_bytes());
            header
        }
        let mut image = vec![0u8; 64];
        image[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let mut headers = vec![[0u8; 64]];
        let mut names = vec![0u8];
        for (name, data) in sections {
            headers.push(section_header(names.len() as u32, image.len(), data.len()));
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            image.extend_from_slice(data);
        }
        let names_at = names.len() as u32;
        names.extend_from_slice(b".shstrtab\0");
        headers.push(section_header(names_at, image.len(), names.len()));
        image.extend_from_slice(&names);
        let table_at = image.len() as u64;
        image[0x28..0x30].copy_from_slice(&table_at.to_le_bytes()); // e_shoff
        image[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
        image[0x3C..0x3E].copy_from_slice(&(headers.len() as u16).to_le_bytes()); // e_shnum
        image[0x3E..0x40].copy_from_slice(&(headers.len() as u16 - 1).to_le_bytes()); // e_shstrndx
        for header in &headers {
            image.extend_from_slice(header);
        }
        image
    }

    fn images(record: &[u8]) -> [Vec<u8>; 2] {
        [
            pe(&[(b".text\0\0\0", &b"code"[..]), (&SECTION, record)]),
            elf(&[(".text", &b"code"[..]), (".mfproto", record)]),
        ]
    }

    /// The test exe links `send_version`, so it carries the section as every host does, with this build's number, and
    /// the number `send_version` puts on the wire is the one on disk. On ELF this also proves the section is kept by
    /// `send_version`'s read: a linker drops an unreferenced one there (CI's Linux job, or this crate's tests run in WSL).
    #[test]
    fn this_test_exe_carries_its_protocol_number() {
        let mut channel = crate::MessageChannel::new(Cursor::new(Vec::<u8>::new()));
        channel.send_version().unwrap();
        let wire = channel.into_inner().into_inner();
        assert_eq!(wire, crate::PROTOCOL_VERSION.to_le_bytes(), "the wire carries this build's number");
        assert_eq!(host_protocol(&std::env::current_exe().unwrap()), Some(crate::PROTOCOL_VERSION), "so does the exe");
    }

    #[test]
    fn a_stamped_pe_or_elf_image_reads_its_number() {
        for image in images(&stamp_bytes(7, !7)) {
            assert_eq!(read(&image), Some(7));
        }
    }

    #[test]
    fn a_bad_record_or_no_section_reads_none() {
        for image in images(&stamp_bytes(7, 7)) {
            assert_eq!(read(&image), None, "the check field disagrees");
        }
        let mut wrong_tag = stamp_bytes(7, !7);
        wrong_tag[0] ^= 1;
        for image in images(&wrong_tag) {
            assert_eq!(read(&image), None, "the tag disagrees");
        }
        let record = stamp_bytes(7, !7);
        assert_eq!(read(&pe(&[(b".data\0\0\0", record.as_slice())])), None, "no .mfproto section");
        assert_eq!(read(&elf(&[(".mfprotox", record.as_slice())])), None, "a longer name is another section");
        assert_eq!(read(&elf(&[(".mfprot", record.as_slice())])), None, "a shorter name is another section");
    }

    /// Every cut of a stamped image, every single byte set to 0xFF, and random bytes after each format's magic read as
    /// no number or the right one, never a panic, a huge allocation or another number.
    #[test]
    fn truncated_or_garbage_files_read_none() {
        for image in images(&stamp_bytes(7, !7)) {
            for len in 0..image.len() {
                assert_eq!(read(&image[..len]), None, "cut at {len}");
            }
            for at in 0..image.len() {
                let mut bad = image.clone();
                bad[at] = 0xFF;
                assert!(matches!(read(&bad), None | Some(7)), "0xFF at {at}");
            }
        }
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        for head in [&b"MZ"[..], &b"\x7fELF\x02\x01"[..]] {
            for _ in 0..200 {
                let mut bytes = head.to_vec();
                bytes.extend((0..512).map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                }));
                assert_eq!(read(&bytes), None);
            }
        }
    }

    #[test]
    fn a_missing_file_or_a_folder_reads_none() {
        assert_eq!(host_protocol(&std::env::temp_dir()), None, "a folder");
        let gone = std::env::temp_dir().join(format!("no-such-host-{}.exe", std::process::id()));
        assert_eq!(host_protocol(&gone), None, "no such file");
    }
}
