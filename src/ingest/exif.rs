//! A deliberately small TIFF/EXIF reader: camera make, model, and the orientation hint.
//!
//! The scan needs a camera name from a 64 KB header read without decoding anything, and
//! the JPEG/TIFF thumbnail path needs the orientation tag (rawler already hands us the
//! equivalent for RAW). Those three short tags in IFD0 are the entire job — no SubIFD
//! traversal, no preview extraction, no pixel data.
//!
//! Every offset here comes out of a file that may be corrupt or hostile, so nothing is
//! trusted: offsets and lengths are bounds-checked before use, `count * size` is computed
//! in `u64`, string lengths are clamped, and visited IFD offsets are tracked so a
//! self-referential chain terminates instead of looping. There is no indexing that can
//! panic.

use std::collections::HashSet;

/// How much of a file's head is worth reading to find IFD0 and its string values.
/// IFD0 sits at the very start and its values follow immediately; 64 KB is generous.
pub const HEADER_BYTES: usize = 64 * 1024;

const TAG_MAKE: u16 = 0x010F;
const TAG_MODEL: u16 = 0x0110;
const TAG_ORIENTATION: u16 = 0x0112;

/// A model string longer than this is not a model string.
const MAX_STRING_BYTES: u64 = 256;

/// Guards against a corrupt entry count claiming more entries than any real file has.
const MAX_ENTRIES: u16 = 512;

/// How many IFDs we are willing to walk before deciding the file is playing games.
const MAX_IFDS: usize = 16;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Header {
    pub make: Option<String>,
    pub model: Option<String>,
    /// EXIF orientation (1-8) as stored; `None` when absent or out of range.
    pub orientation: Option<u16>,
}

impl Header {
    pub fn is_empty(&self) -> bool {
        self.make.is_none() && self.model.is_none() && self.orientation.is_none()
    }

    /// The camera as a person would write it, if the file says at all.
    pub fn camera(&self) -> Option<String> {
        let model = self.model.as_deref().unwrap_or_default();
        let make = self.make.as_deref().unwrap_or_default();
        if model.is_empty() && make.is_empty() {
            return None;
        }
        Some(super::format_camera(make, model))
    }
}

/// Smallest plausible camera RAW. Real files are megabytes; anything under this is a stub,
/// a truncated transfer, or not a RAW at all.
const MIN_RAW_BYTES: usize = 64 * 1024;

/// Whether these bytes open with a container a RAW decoder should be allowed to touch.
///
/// This gate exists because rawler will sniff unrecognised bytes into *some* format and
/// then trust the dimensions it reads there: handed 4 KB of zeroes it allocated its way
/// past 6 GB before we killed it. An out-of-memory abort is not something `catch_unwind`
/// can save us from, so the cheapest real protection is to never let obvious nonsense
/// reach the decoder. Every RAW format we ingest is one of these containers.
pub fn looks_like_raw(bytes: &[u8]) -> bool {
    if bytes.len() < MIN_RAW_BYTES {
        return false;
    }
    let starts_with = |magic: &[u8]| bytes.starts_with(magic);
    let at = |offset: usize, magic: &[u8]| {
        bytes
            .get(offset..offset.saturating_add(magic.len()))
            .is_some_and(|slice| slice == magic)
    };

    // TIFF, little- and big-endian: NEF, CR2, ARW, DNG, ORF, RW2, PEF, SRW, 3FR, IIQ,
    // KDC, MEF, MOS, ERF, NRW, RWL, SR2, DCR — the overwhelming majority.
    starts_with(b"II\x2a\x00")
        || starts_with(b"MM\x00\x2a")
        // Panasonic RW2 and Olympus ORF use their own TIFF-ish version numbers.
        || starts_with(b"IIU\x00")
        || starts_with(b"IIR\x00")
        || starts_with(b"MMOR")
        // Fujifilm RAF.
        || starts_with(b"FUJIFILM")
        // Sigma X3F.
        || starts_with(b"FOVb")
        // Minolta MRW.
        || starts_with(b"\x00MRM")
        // Canon CR3 is ISO base media, Canon CRW has its own signature.
        || at(4, b"ftyp")
        || at(6, b"HEAPCCDR")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        }
    }
}

/// Read the header tags from the first bytes of a file.
///
/// Returns an empty [`Header`] rather than an error for anything unparseable: a file we
/// cannot read metadata from is not a problem worth failing an ingest over.
pub fn parse(bytes: &[u8]) -> Header {
    // A bare TIFF (every RAW format we handle is one), or JPEG's APP1 Exif segment, whose
    // TIFF block has its own byte order and its own offset origin.
    if let Some(header) = tiff_at(bytes, 0) {
        return header;
    }
    match jpeg_exif_offset(bytes) {
        Some(offset) => tiff_at(bytes, offset).unwrap_or_default(),
        None => Header::default(),
    }
}

/// Byte offset of the TIFF header inside a JPEG's `APP1`/`Exif\0\0` segment.
fn jpeg_exif_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut cursor = 2usize;
    loop {
        // Segments may be preceded by fill bytes.
        while bytes.get(cursor) == Some(&0xFF) && bytes.get(cursor + 1) == Some(&0xFF) {
            cursor += 1;
        }
        if bytes.get(cursor)? != &0xFF {
            return None;
        }
        let marker = *bytes.get(cursor + 1)?;
        // Start of scan or end of image: no metadata beyond here.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let length = u16::from_be_bytes([*bytes.get(cursor + 2)?, *bytes.get(cursor + 3)?]) as usize;
        if length < 2 {
            return None;
        }
        let payload = cursor.checked_add(4)?;
        if marker == 0xE1 && bytes.get(payload..payload.checked_add(6)?)? == b"Exif\0\0" {
            return payload.checked_add(6);
        }
        cursor = cursor.checked_add(2)?.checked_add(length)?;
    }
}

/// Parse a TIFF header and its IFD0 chain, with `base` as the origin all offsets are
/// relative to.
fn tiff_at(bytes: &[u8], base: usize) -> Option<Header> {
    let header = bytes.get(base..base.checked_add(8)?)?;
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    if endian.u16([header[2], header[3]]) != 42 {
        return None;
    }
    let first_ifd = endian.u32([header[4], header[5], header[6], header[7]]) as usize;

    let mut result = Header::default();
    let mut visited = HashSet::new();
    let mut next = first_ifd;
    for _ in 0..MAX_IFDS {
        if next == 0 || !visited.insert(next) {
            break;
        }
        match read_ifd(bytes, base, next, endian, &mut result) {
            Some(following) => next = following,
            None => break,
        }
        // IFD0 carries everything we want; later IFDs describe the thumbnail. Stop as
        // soon as we have what we came for.
        if result.make.is_some() && result.model.is_some() && result.orientation.is_some() {
            break;
        }
    }
    Some(result)
}

/// Read one IFD into `out`, returning the offset of the next one.
fn read_ifd(
    bytes: &[u8],
    base: usize,
    offset: usize,
    endian: Endian,
    out: &mut Header,
) -> Option<usize> {
    let start = base.checked_add(offset)?;
    let count_bytes = bytes.get(start..start.checked_add(2)?)?;
    let entries = endian.u16([count_bytes[0], count_bytes[1]]);
    if entries == 0 || entries > MAX_ENTRIES {
        return None;
    }
    // The whole entry table plus the next-IFD pointer must actually be present.
    let table_len = 2usize.checked_add(12usize.checked_mul(entries as usize)?)?;
    let table_end = start.checked_add(table_len)?;
    bytes.get(start..table_end)?;

    for index in 0..entries as usize {
        let entry_at = start.checked_add(2)?.checked_add(index.checked_mul(12)?)?;
        let entry = match bytes.get(entry_at..entry_at.checked_add(12)?) {
            Some(entry) => entry,
            None => break,
        };
        let tag = endian.u16([entry[0], entry[1]]);
        if !matches!(tag, TAG_MAKE | TAG_MODEL | TAG_ORIENTATION) {
            continue;
        }
        let kind = endian.u16([entry[2], entry[3]]);
        let count = endian.u32([entry[4], entry[5], entry[6], entry[7]]) as u64;
        let value_field = [entry[8], entry[9], entry[10], entry[11]];

        match tag {
            TAG_ORIENTATION => {
                // SHORT, count 1 — always inline, in the first two bytes of the field.
                if kind == 3 && count == 1 {
                    let value = endian.u16([value_field[0], value_field[1]]);
                    if (1..=8).contains(&value) {
                        out.orientation.get_or_insert(value);
                    }
                }
            }
            _ => {
                // ASCII.
                if kind != 2 {
                    continue;
                }
                let Some(text) = read_string(bytes, base, endian, count, value_field, entry_at)
                else {
                    continue;
                };
                let slot = if tag == TAG_MAKE {
                    &mut out.make
                } else {
                    &mut out.model
                };
                if slot.is_none() {
                    *slot = Some(text);
                }
            }
        }
    }

    let next = bytes.get(table_end.checked_sub(4)?..table_end)?;
    Some(endian.u32([next[0], next[1], next[2], next[3]]) as usize)
}

/// Read an ASCII tag value, which lives inline when it fits in the 4-byte value field and
/// at an offset otherwise.
fn read_string(
    bytes: &[u8],
    base: usize,
    endian: Endian,
    count: u64,
    value_field: [u8; 4],
    entry_at: usize,
) -> Option<String> {
    if count == 0 || count > MAX_STRING_BYTES {
        return None;
    }
    let len = count as usize;
    let slice = if count <= 4 {
        // Inline: the bytes are the value field itself.
        bytes.get(entry_at.checked_add(8)?..entry_at.checked_add(8)?.checked_add(len)?)?
    } else {
        let offset = endian.u32(value_field) as usize;
        let start = base.checked_add(offset)?;
        bytes.get(start..start.checked_add(len)?)?
    };
    let text = slice
        .iter()
        .copied()
        .take_while(|byte| *byte != 0)
        .map(char::from)
        .collect::<String>();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal little-endian TIFF with the given IFD0 entries.
    fn tiff(entries: &[(u16, u16, u32, [u8; 4])], trailing: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(value);
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        out.extend_from_slice(trailing);
        out
    }

    #[test]
    fn reads_make_model_and_orientation() {
        // "NIKON Z 7" is 10 bytes with its NUL, so it lives at an offset; orientation is
        // inline.
        let value_offset = (8 + 2 + 3 * 12 + 4) as u32;
        let bytes = tiff(
            &[
                (TAG_MAKE, 2, 6, value_offset.to_le_bytes()),
                (TAG_MODEL, 2, 10, (value_offset + 6).to_le_bytes()),
                (TAG_ORIENTATION, 3, 1, [6, 0, 0, 0]),
            ],
            b"NIKON\0NIKON Z 7\0",
        );
        let header = parse(&bytes);
        assert_eq!(header.make.as_deref(), Some("NIKON"));
        assert_eq!(header.model.as_deref(), Some("NIKON Z 7"));
        assert_eq!(header.orientation, Some(6));
        assert_eq!(header.camera().as_deref(), Some("Nikon Z 7"));
    }

    #[test]
    fn reads_short_strings_stored_inline() {
        // "ILC" + NUL is 4 bytes, so it sits in the value field rather than at an offset.
        let bytes = tiff(&[(TAG_MODEL, 2, 4, *b"ILC\0")], b"");
        assert_eq!(parse(&bytes).model.as_deref(), Some("ILC"));
    }

    #[test]
    fn big_endian_files_parse_too() {
        let mut out = Vec::new();
        out.extend_from_slice(b"MM");
        out.extend_from_slice(&42u16.to_be_bytes());
        out.extend_from_slice(&8u32.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&TAG_ORIENTATION.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&[0, 8, 0, 0]);
        out.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse(&out).orientation, Some(8));
    }

    #[test]
    fn malformed_input_yields_nothing_rather_than_panicking() {
        // Truncated at every length, including mid-header and mid-entry.
        let full = tiff(
            &[(TAG_MODEL, 2, 10, 46u32.to_le_bytes())],
            b"NIKON Z 7\0",
        );
        for len in 0..full.len() {
            let _ = parse(&full[..len]);
        }

        // Absurd counts must not allocate or index out of bounds.
        assert!(parse(&tiff(&[(TAG_MODEL, 2, u32::MAX, [0; 4])], b"")).is_empty());
        assert!(parse(&tiff(&[(TAG_MAKE, 2, 999_999, [8, 0, 0, 0])], b"")).is_empty());

        // An offset pointing past the end of what we read is normal, not fatal.
        assert!(parse(&tiff(&[(TAG_MODEL, 2, 10, u32::MAX.to_le_bytes())], b"")).is_empty());

        // Entry count larger than the file can hold.
        let mut bogus = Vec::new();
        bogus.extend_from_slice(b"II");
        bogus.extend_from_slice(&42u16.to_le_bytes());
        bogus.extend_from_slice(&8u32.to_le_bytes());
        bogus.extend_from_slice(&u16::MAX.to_le_bytes());
        assert!(parse(&bogus).is_empty());

        // Not a TIFF or a JPEG at all.
        assert!(parse(&[0u8; 1024]).is_empty());
        assert!(parse(&[0xFFu8; 1024]).is_empty());
        assert!(parse(b"").is_empty());
    }

    #[test]
    fn a_self_referential_ifd_chain_terminates() {
        // next-IFD points back at IFD0; the visited set must stop the walk.
        let mut bytes = tiff(&[(TAG_ORIENTATION, 3, 1, [1, 0, 0, 0])], b"");
        let next_at = bytes.len() - 4;
        bytes[next_at..next_at + 4].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(parse(&bytes).orientation, Some(1));
    }

    #[test]
    fn out_of_range_orientation_is_ignored() {
        assert_eq!(parse(&tiff(&[(TAG_ORIENTATION, 3, 1, [0, 0, 0, 0])], b"")).orientation, None);
        assert_eq!(parse(&tiff(&[(TAG_ORIENTATION, 3, 1, [9, 0, 0, 0])], b"")).orientation, None);
    }

    #[test]
    fn finds_exif_inside_a_jpeg_app1_segment() {
        let inner = tiff(&[(TAG_MODEL, 2, 4, *b"X-T\0")], b"");
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&((inner.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&inner);
        jpeg.extend_from_slice(&[0xFF, 0xDA]);
        assert_eq!(parse(&jpeg).model.as_deref(), Some("X-T"));
    }

    #[test]
    fn a_jpeg_without_exif_is_simply_empty() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02];
        assert!(parse(&jpeg).is_empty());
        // A JPEG whose segment length runs off the end must not loop forever.
        let truncated = vec![0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xFF];
        assert!(parse(&truncated).is_empty());
    }
}
