//! Minimal ISO9660 reader.
//!
//! Just enough to walk a Joliet-or-primary directory tree and stream file
//! contents back. Designed to be fed a `Read + Seek` over 2048-byte logical
//! sectors (see [`crate::cooked::CookedSectorReader`]).
//!
//! References:
//! - ECMA-119 / ISO 9660 (volume descriptors, directory records)
//! - Joliet specification: Microsoft "Joliet" supplement defining SVD escape
//!   sequences for UCS-2 names
//! - https://wiki.osdev.org/ISO_9660 for a digestible summary

use crate::{
    cooked::COOKED_SECTOR_SIZE,
    error::{Error, Result},
};
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
};

/// First volume descriptor lives at LBA 16 — the "system area" is LBAs 0..16.
const VD_START_LBA: u64 = 16;

/// Cap on bytes we'll allocate for a single directory extent. Real-world
/// ISO9660 directories rarely exceed a few hundred KB even on large discs.
/// Bound at 16 MiB so a malformed/malicious image can't request a multi-GB
/// allocation and OOM the process.
const MAX_DIRECTORY_BYTES: u32 = 16 * 1024 * 1024;

/// Stack-depth limit for directory recursion. ISO9660 nominally limits depth
/// to 8 but Joliet relaxes this; 64 is generous in practice and prevents
/// stack overflow from a crafted image.
const MAX_DIRECTORY_DEPTH: usize = 64;

#[derive(Debug)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Debug)]
pub enum EntryKind {
    File { lba: u32, size: u32 },
    Dir(Vec<DirEntry>),
}

/// Parse the disc, choosing Joliet if available and falling back to the
/// primary volume descriptor otherwise. Returns the root directory's
/// children — the root itself has no name.
pub fn read_tree<R: Read + Seek>(reader: &mut R) -> Result<Vec<DirEntry>> {
    let descriptors = read_volume_descriptors(reader)?;

    // Prefer Joliet (SVD with a known escape sequence). Fall back to PVD.
    let (root_lba, root_size, encoding) = if let Some(svd) = descriptors
        .iter()
        .find(|d| matches!(d, VolDesc::Supplementary { joliet: true, .. }))
    {
        match svd {
            VolDesc::Supplementary {
                root_lba,
                root_size,
                ..
            } => (*root_lba, *root_size, NameEncoding::Joliet),
            _ => unreachable!(),
        }
    } else if let Some(pvd) = descriptors.iter().find(|d| matches!(d, VolDesc::Primary { .. })) {
        match pvd {
            VolDesc::Primary {
                root_lba,
                root_size,
            } => (*root_lba, *root_size, NameEncoding::Primary),
            _ => unreachable!(),
        }
    } else {
        return Err(Error::Iso9660(
            "No primary volume descriptor found".to_string(),
        ));
    };

    let mut ancestors = HashSet::new();
    read_directory(reader, root_lba, root_size, encoding, &mut ancestors, 0)
}

#[derive(Debug)]
enum VolDesc {
    Primary {
        root_lba: u32,
        root_size: u32,
    },
    Supplementary {
        joliet: bool,
        root_lba: u32,
        root_size: u32,
    },
    Other,
}

/// Per ECMA-119 §8.4, descriptors are at LBA 16, 17, ... until a Volume
/// Descriptor Set Terminator (type 0xFF) is encountered. Cap iteration at a
/// sane limit so a malformed disc can't loop forever.
fn read_volume_descriptors<R: Read + Seek>(reader: &mut R) -> Result<Vec<VolDesc>> {
    let mut out = Vec::new();
    let mut buf = [0u8; COOKED_SECTOR_SIZE];
    for lba in VD_START_LBA..VD_START_LBA + 32 {
        seek_lba(reader, lba)?;
        reader.read_exact(&mut buf).map_err(Error::Io)?;
        // Standard ID must be "CD001" at offset 1..6.
        if &buf[1..6] != b"CD001" {
            return Err(Error::Iso9660(format!(
                "Bad standard ID at LBA {lba} (not 'CD001')"
            )));
        }
        let vd_type = buf[0];
        match vd_type {
            0x01 => out.push(parse_primary(&buf)?),
            0x02 => out.push(parse_supplementary(&buf)?),
            0xFF => return Ok(out),
            _ => out.push(VolDesc::Other),
        }
    }
    Err(Error::Iso9660(
        "No volume descriptor set terminator within 32 sectors".to_string(),
    ))
}

fn parse_primary(sector: &[u8]) -> Result<VolDesc> {
    // ECMA-119 §8.4: the PVD root directory record sits at offset 156, length
    // 34 bytes (a directory record describing the root).
    let root_record = &sector[156..156 + 34];
    let (root_lba, root_size) = parse_root_record(root_record)?;
    Ok(VolDesc::Primary {
        root_lba,
        root_size,
    })
}

fn parse_supplementary(sector: &[u8]) -> Result<VolDesc> {
    // Same layout as PVD for our purposes: root directory record at offset 156.
    let root_record = &sector[156..156 + 34];
    let (root_lba, root_size) = parse_root_record(root_record)?;
    // Escape Sequences: 32 bytes at offset 88 (§8.5.6). Joliet defines:
    // 25/2F/40 (level 1), 25/2F/43 (level 2), 25/2F/45 (level 3)
    // i.e. "%/" followed by '@', 'C', or 'E'. Per the Joliet spec the
    // sequence sits at the START of the escape field — don't scan with a
    // sliding window, or non-Joliet SVDs whose escape bytes happen to
    // contain `%/E` later would be mis-decoded as UCS-2.
    let escape = &sector[88..88 + 32];
    let joliet = escape[0] == 0x25
        && escape[1] == 0x2F
        && matches!(escape[2], 0x40 | 0x43 | 0x45);
    Ok(VolDesc::Supplementary {
        joliet,
        root_lba,
        root_size,
    })
}

fn parse_root_record(rec: &[u8]) -> Result<(u32, u32)> {
    // Directory record layout (ECMA-119 §9.1):
    //   0: length of directory record (LEN_DR)
    //   1: extended attribute record length
    //   2..10: location of extent (LBA, both-endian u32)
    //   10..18: data length (both-endian u32)
    //   ... (we only need lba + size here)
    if rec.len() < 18 {
        return Err(Error::Iso9660("Root directory record truncated".to_string()));
    }
    let lba = parse_both_endian_u32(&rec[2..10], "root LBA")?;
    let size = parse_both_endian_u32(&rec[10..18], "root size")?;
    Ok((lba, size))
}

/// Parse one of ISO9660's "both-endian" u32 fields (4 bytes LE followed by
/// 4 bytes BE encoding the same value). Errors if the two halves disagree —
/// that's a sign of a malformed image or a parser bug worth surfacing.
fn parse_both_endian_u32(field: &[u8], context: &str) -> Result<u32> {
    if field.len() < 8 {
        return Err(Error::Iso9660(format!(
            "{context}: both-endian field truncated"
        )));
    }
    let le = u32::from_le_bytes([field[0], field[1], field[2], field[3]]);
    let be = u32::from_be_bytes([field[4], field[5], field[6], field[7]]);
    if le != be {
        return Err(Error::Iso9660(format!(
            "{context}: both-endian halves disagree ({le} LE vs {be} BE)"
        )));
    }
    Ok(le)
}

#[derive(Clone, Copy, Debug)]
enum NameEncoding {
    Primary,
    Joliet,
}

/// Read a directory's contents starting at `lba`, spanning `size_bytes` bytes.
/// Recurses into subdirectories, refusing extents larger than
/// `MAX_DIRECTORY_BYTES`, depth past `MAX_DIRECTORY_DEPTH`, or revisits of an
/// LBA that's already on the current recursion stack (cycle defence).
fn read_directory<R: Read + Seek>(
    reader: &mut R,
    lba: u32,
    size_bytes: u32,
    encoding: NameEncoding,
    ancestors: &mut HashSet<u32>,
    depth: usize,
) -> Result<Vec<DirEntry>> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(Error::Iso9660(format!(
            "Directory nesting deeper than {MAX_DIRECTORY_DEPTH} levels"
        )));
    }
    if size_bytes > MAX_DIRECTORY_BYTES {
        return Err(Error::Iso9660(format!(
            "Directory extent at LBA {lba} declares {size_bytes} bytes, \
             exceeds {MAX_DIRECTORY_BYTES}-byte cap"
        )));
    }
    // Track only the current recursion path. A directory whose LBA appears
    // in our ancestor chain is a genuine cycle (A → B → A). The
    // MAX_DIRECTORY_DEPTH cap bounds total work even if a malformed image
    // somehow reaches the same extent via two non-overlapping paths.
    if !ancestors.insert(lba) {
        return Err(Error::Iso9660(format!(
            "Directory cycle detected at LBA {lba}"
        )));
    }
    let result = read_directory_body(reader, lba, size_bytes, encoding, ancestors, depth);
    ancestors.remove(&lba);
    result
}

fn read_directory_body<R: Read + Seek>(
    reader: &mut R,
    lba: u32,
    size_bytes: u32,
    encoding: NameEncoding,
    ancestors: &mut HashSet<u32>,
    depth: usize,
) -> Result<Vec<DirEntry>> {
    let mut raw = vec![0u8; size_bytes as usize];
    seek_lba(reader, lba as u64)?;
    reader.read_exact(&mut raw).map_err(Error::Io)?;

    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos < raw.len() {
        // ECMA-119 §6.8.1.1: directory records do not cross logical sector
        // boundaries. A zero-length record indicates the rest of the current
        // 2048-byte block is padding — skip to the next block.
        let len_dr = raw[pos] as usize;
        if len_dr == 0 {
            let next_sector = (pos / COOKED_SECTOR_SIZE + 1) * COOKED_SECTOR_SIZE;
            if next_sector >= raw.len() {
                break;
            }
            pos = next_sector;
            continue;
        }
        if pos + len_dr > raw.len() {
            return Err(Error::Iso9660(
                "Directory record extends past directory data".to_string(),
            ));
        }
        // ECMA-119 §6.8.1.1 forbids records from crossing a 2048-byte
        // logical sector boundary. If a malformed image's len_dr would
        // make us slice into the next sector, refuse rather than parse
        // unrelated bytes as part of the record.
        if (pos % COOKED_SECTOR_SIZE) + len_dr > COOKED_SECTOR_SIZE {
            return Err(Error::Iso9660(
                "Directory record crosses sector boundary".to_string(),
            ));
        }
        let rec = &raw[pos..pos + len_dr];
        pos += len_dr;

        // 0: LEN_DR, 1: ext attr len, 2..10: LBA, 10..18: size,
        // 18..25: recording date/time (7 bytes), 25: file flags,
        // 26: file unit size, 27: interleave gap, 28..32: vol seq number,
        // 32: LEN_FI (file identifier length), 33..33+LEN_FI: file identifier
        if rec.len() < 33 {
            continue;
        }
        let entry_lba = parse_both_endian_u32(&rec[2..10], "directory entry LBA")?;
        let entry_size = parse_both_endian_u32(&rec[10..18], "directory entry size")?;
        let flags = rec[25];
        let len_fi = rec[32] as usize;
        if 33 + len_fi > rec.len() {
            continue;
        }
        let raw_name = &rec[33..33 + len_fi];

        // Skip the special '.' (0x00) and '..' (0x01) self/parent entries.
        if len_fi == 1 && (raw_name[0] == 0x00 || raw_name[0] == 0x01) {
            continue;
        }

        // Joliet identifiers are UCS-2BE — two bytes per code unit. An odd
        // LEN_FI would silently drop the trailing byte during decoding,
        // potentially altering names. Refuse rather than guess.
        if matches!(encoding, NameEncoding::Joliet) && len_fi % 2 != 0 {
            return Err(Error::Iso9660(format!(
                "Joliet file identifier length {len_fi} is not a multiple of 2"
            )));
        }

        let is_dir = flags & 0x02 != 0;
        let name = decode_name(raw_name, encoding, is_dir);

        let kind = if is_dir {
            let children = read_directory(
                reader,
                entry_lba,
                entry_size,
                encoding,
                ancestors,
                depth + 1,
            )?;
            EntryKind::Dir(children)
        } else {
            EntryKind::File {
                lba: entry_lba,
                size: entry_size,
            }
        };

        entries.push(DirEntry { name, kind });
    }
    Ok(entries)
}

/// Decode a directory record's file identifier into a String.
/// Strips the `;<version>` suffix on files and the trailing `.` ISO9660 quirk.
fn decode_name(raw: &[u8], encoding: NameEncoding, is_dir: bool) -> String {
    let mut s = match encoding {
        NameEncoding::Joliet => {
            // UCS-2BE, pairs of bytes per code unit.
            let units = raw
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]));
            char::decode_utf16(units)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect::<String>()
        }
        NameEncoding::Primary => {
            // Strict ISO9660 d-characters are ASCII; tolerate the broader
            // ISO-8859-1 range some discs use.
            raw.iter().map(|&b| b as char).collect::<String>()
        }
    };

    if !is_dir {
        if let Some(semi) = s.rfind(';') {
            s.truncate(semi);
        }
        // ISO9660 requires a '.' between name and extension even when there's
        // no extension; strip a trailing '.' that's just a separator.
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

fn seek_lba<R: Seek>(reader: &mut R, lba: u64) -> Result<()> {
    reader
        .seek(SeekFrom::Start(lba * COOKED_SECTOR_SIZE as u64))
        .map_err(Error::Io)?;
    Ok(())
}

/// Stream the contents of a file at `lba` for `size` bytes into `writer`.
pub fn copy_file<R: Read + Seek, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
    lba: u32,
    size: u32,
) -> Result<()> {
    seek_lba(reader, lba as u64)?;
    let mut remaining = size as u64;
    let mut buf = [0u8; COOKED_SECTOR_SIZE];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        reader.read_exact(&mut buf[..want]).map_err(Error::Io)?;
        writer.write_all(&buf[..want]).map_err(Error::Io)?;
        remaining -= want as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_directory_rejects_oversized_extent() {
        // Need any reader — call won't get far enough to actually read.
        let mut cursor = std::io::Cursor::new(vec![0u8; 4096]);
        let mut visited = HashSet::new();
        let err = read_directory(
            &mut cursor,
            18,
            MAX_DIRECTORY_BYTES + 1,
            NameEncoding::Primary,
            &mut visited,
            0,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Iso9660(_)));
    }

    #[test]
    fn parse_both_endian_u32_matches() {
        let mut buf = [0u8; 8];
        buf[..4].copy_from_slice(&12345u32.to_le_bytes());
        buf[4..].copy_from_slice(&12345u32.to_be_bytes());
        assert_eq!(parse_both_endian_u32(&buf, "test").unwrap(), 12345);
    }

    #[test]
    fn parse_both_endian_u32_rejects_disagreement() {
        let mut buf = [0u8; 8];
        buf[..4].copy_from_slice(&12345u32.to_le_bytes());
        buf[4..].copy_from_slice(&99999u32.to_be_bytes());
        assert!(parse_both_endian_u32(&buf, "test").is_err());
    }

    #[test]
    fn read_directory_rejects_records_crossing_sector_boundary() {
        // Walk pos near the end of the first 2048-byte sector via 8
        // chained records of length 255 (LEN_FI=0, plain files), then
        // place a bogus 40-byte record at pos=2040 whose length would
        // extend into the next sector. Use a 2-sector extent so the
        // overflow vs. boundary checks are distinguishable: the record
        // would fit in `raw` but crosses the 2048-byte boundary.
        let mut buf = vec![0u8; 2 * COOKED_SECTOR_SIZE];
        let mut p = 0usize;
        for _ in 0..8 {
            buf[p] = 255; // LEN_DR; LEN_FI defaults to 0, flags = 0
            p += 255;
        }
        // pos is now 2040; place the boundary-crossing record here.
        buf[p] = 40; // 2040 + 40 = 2080 > 2048

        // Image: 18 zero sectors then our crafted directory at LBA 18.
        let mut img = vec![0u8; 20 * COOKED_SECTOR_SIZE];
        img[18 * COOKED_SECTOR_SIZE..18 * COOKED_SECTOR_SIZE + buf.len()].copy_from_slice(&buf);
        let mut cursor = std::io::Cursor::new(img);

        let mut visited = HashSet::new();
        let err = read_directory(
            &mut cursor,
            18,
            (2 * COOKED_SECTOR_SIZE) as u32,
            NameEncoding::Primary,
            &mut visited,
            0,
        )
        .unwrap_err();
        match err {
            Error::Iso9660(msg) => assert!(msg.contains("sector boundary"), "got: {msg}"),
            _ => panic!("expected Iso9660 error, got {err:?}"),
        }
    }

    #[test]
    fn read_directory_rejects_ancestor_cycle() {
        let mut cursor = std::io::Cursor::new(vec![0u8; 4096]);
        let mut ancestors = HashSet::new();
        ancestors.insert(18); // pretend LBA 18 is on the current path
        let err = read_directory(
            &mut cursor,
            18,
            2048,
            NameEncoding::Primary,
            &mut ancestors,
            1,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Iso9660(_)));
    }

    #[test]
    fn read_directory_ancestors_set_is_popped_on_return() {
        // After read_directory returns, its own LBA should NOT remain in
        // the ancestors set — otherwise a sibling directory that happens
        // to share an extent would be misidentified as a cycle.
        let mut cursor = std::io::Cursor::new(vec![0u8; 4 * COOKED_SECTOR_SIZE]);
        let mut ancestors = HashSet::new();
        // Empty dir extent (all zeroes) so the body returns immediately
        // with no entries. We just want to verify the set is cleaned up.
        let _ = read_directory(
            &mut cursor,
            1,
            COOKED_SECTOR_SIZE as u32,
            NameEncoding::Primary,
            &mut ancestors,
            0,
        )
        .unwrap();
        assert!(ancestors.is_empty(), "ancestors set should be empty after return");
    }

    #[test]
    fn decode_primary_strips_version_and_dot() {
        let raw = b"README.TXT;1";
        assert_eq!(decode_name(raw, NameEncoding::Primary, false), "README.TXT");

        let raw = b"FOO.;1";
        assert_eq!(decode_name(raw, NameEncoding::Primary, false), "FOO");

        let raw = b"BAR";
        assert_eq!(decode_name(raw, NameEncoding::Primary, true), "BAR");
    }

    #[test]
    fn decode_joliet_ucs2be() {
        // "AB;1" in UCS-2BE.
        let raw: Vec<u8> = "AB;1"
            .encode_utf16()
            .flat_map(|u| u.to_be_bytes())
            .collect();
        assert_eq!(decode_name(&raw, NameEncoding::Joliet, false), "AB");
    }

    /// Build a synthetic, minimal ISO9660 image with one top-level file. The
    /// layout is:
    ///   LBA 16: PVD pointing at root dir at LBA 18, size 2048
    ///   LBA 17: VDST (volume descriptor set terminator)
    ///   LBA 18: root directory data: ".", "..", and one file "README.TXT;1"
    ///           located at LBA 19, with `payload.len()` bytes
    ///   LBA 19: payload, padded to a sector
    fn make_synthetic_iso(payload: &[u8]) -> Vec<u8> {
        let mut image = vec![0u8; 20 * COOKED_SECTOR_SIZE];

        // ---- PVD at LBA 16 ----
        {
            let s = &mut image[16 * COOKED_SECTOR_SIZE..17 * COOKED_SECTOR_SIZE];
            s[0] = 0x01;
            s[1..6].copy_from_slice(b"CD001");
            // Root directory record at offset 156 (LEN_DR = 34).
            // 0:LEN_DR=34, 1:ext attr=0, 2..6:LBA LE=18, 6..10:LBA BE=18,
            // 10..14:size LE=2048, 14..18:size BE=2048
            s[156] = 34;
            s[158..162].copy_from_slice(&18u32.to_le_bytes());
            s[162..166].copy_from_slice(&18u32.to_be_bytes());
            s[166..170].copy_from_slice(&2048u32.to_le_bytes());
            s[170..174].copy_from_slice(&2048u32.to_be_bytes());
            // s[181] = file flags (directory bit). 156 + 25 = 181.
            s[181] = 0x02;
            // LEN_FI=1 at 156+32=188, file identifier=0x00 at 189 (".", root).
            s[188] = 1;
            s[189] = 0x00;
        }

        // ---- VDST at LBA 17 ----
        {
            let s = &mut image[17 * COOKED_SECTOR_SIZE..18 * COOKED_SECTOR_SIZE];
            s[0] = 0xFF;
            s[1..6].copy_from_slice(b"CD001");
        }

        // ---- Root directory at LBA 18 ----
        {
            let s = &mut image[18 * COOKED_SECTOR_SIZE..19 * COOKED_SECTOR_SIZE];
            let mut p = 0usize;

            // "." entry: LEN_DR=34, file id length=1, id=0x00
            s[p] = 34;
            s[p + 2..p + 6].copy_from_slice(&18u32.to_le_bytes());
            s[p + 6..p + 10].copy_from_slice(&18u32.to_be_bytes());
            s[p + 10..p + 14].copy_from_slice(&2048u32.to_le_bytes());
            s[p + 14..p + 18].copy_from_slice(&2048u32.to_be_bytes());
            s[p + 25] = 0x02;
            s[p + 32] = 1;
            s[p + 33] = 0x00;
            p += 34;

            // ".." entry: LEN_DR=34, file id length=1, id=0x01
            s[p] = 34;
            s[p + 2..p + 6].copy_from_slice(&18u32.to_le_bytes());
            s[p + 6..p + 10].copy_from_slice(&18u32.to_be_bytes());
            s[p + 10..p + 14].copy_from_slice(&2048u32.to_le_bytes());
            s[p + 14..p + 18].copy_from_slice(&2048u32.to_be_bytes());
            s[p + 25] = 0x02;
            s[p + 32] = 1;
            s[p + 33] = 0x01;
            p += 34;

            // "README.TXT;1" entry. File identifier is 12 chars.
            let id = b"README.TXT;1";
            let len_fi = id.len();
            // len_dr = 33 + len_fi, rounded up to even.
            let len_dr = 33 + len_fi + if (33 + len_fi) % 2 != 0 { 1 } else { 0 };
            s[p] = len_dr as u8;
            s[p + 2..p + 6].copy_from_slice(&19u32.to_le_bytes());
            s[p + 6..p + 10].copy_from_slice(&19u32.to_be_bytes());
            s[p + 10..p + 14].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            s[p + 14..p + 18].copy_from_slice(&(payload.len() as u32).to_be_bytes());
            // flags = 0 (regular file)
            s[p + 32] = len_fi as u8;
            s[p + 33..p + 33 + len_fi].copy_from_slice(id);
            // The rest of the sector is zero (padding marker for end of records).
        }

        // ---- File payload at LBA 19 ----
        {
            let s = &mut image[19 * COOKED_SECTOR_SIZE..20 * COOKED_SECTOR_SIZE];
            s[..payload.len()].copy_from_slice(payload);
        }

        image
    }

    #[test]
    fn end_to_end_read_tree_and_copy_file() {
        let payload = b"Hello from the ISO9660 reader.";
        let image = make_synthetic_iso(payload);
        let mut cursor = std::io::Cursor::new(image);

        let tree = read_tree(&mut cursor).unwrap();
        assert_eq!(tree.len(), 1, "expected exactly one top-level entry");
        assert_eq!(tree[0].name, "README.TXT");

        let (lba, size) = match &tree[0].kind {
            EntryKind::File { lba, size } => (*lba, *size),
            _ => panic!("expected a file"),
        };
        assert_eq!(size as usize, payload.len());

        let mut out = Vec::new();
        copy_file(&mut cursor, &mut out, lba, size).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn joliet_escape_sequence_detected() {
        let mut sector = vec![0u8; COOKED_SECTOR_SIZE];
        sector[0] = 0x02; // SVD type
        sector[1..6].copy_from_slice(b"CD001");
        // Joliet level 1 escape: 25 2F 40
        sector[88] = 0x25;
        sector[89] = 0x2F;
        sector[90] = 0x40;
        // Minimal root record at offset 156: LEN_DR=34, LBA=20, size=2048.
        sector[156] = 34;
        sector[158..162].copy_from_slice(&20u32.to_le_bytes());
        sector[162..166].copy_from_slice(&20u32.to_be_bytes());
        sector[166..170].copy_from_slice(&2048u32.to_le_bytes());
        sector[170..174].copy_from_slice(&2048u32.to_be_bytes());
        let vd = parse_supplementary(&sector).unwrap();
        match vd {
            VolDesc::Supplementary {
                joliet,
                root_lba,
                root_size,
            } => {
                assert!(joliet);
                assert_eq!(root_lba, 20);
                assert_eq!(root_size, 2048);
            }
            _ => panic!("expected Supplementary"),
        }
    }
}
