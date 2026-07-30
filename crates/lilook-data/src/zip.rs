//! Just enough zip to open a `.npz`.
//!
//! An npz is a zip archive of `.npy` members, written by `np.savez` (stored) or
//! `np.savez_compressed` (deflate). Reading it needs the central directory, the
//! local headers it points at, and those two compression methods -- which is a
//! couple of hundred lines against a dependency that brings encryption,
//! zstd/bzip2 and a `std::io` surface none of this wants.
//!
//! Deliberately strict about what it does not do: an encrypted or
//! zip64-spanning member is reported, not skipped, so a file that does not read
//! says why rather than appearing to hold nothing.

use crate::{take, DataError};

const CENTRAL: [u8; 4] = [b'P', b'K', 1, 2];
const END: [u8; 4] = [b'P', b'K', 5, 6];
const END64: [u8; 4] = [b'P', b'K', 6, 6];

/// Every member's name and decompressed contents.
pub fn members(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, DataError> {
    let mut out = vec![];
    for entry in directory(bytes)? {
        let data = read_member(bytes, &entry)?;
        out.push((entry.name, data));
    }
    Ok(out)
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    uncompressed: usize,
    offset: usize,
    encrypted: bool,
}

/// Walk the central directory, which is the only reliable index: local headers
/// may carry zeroed sizes with the real ones in a trailing descriptor.
fn directory(bytes: &[u8]) -> Result<Vec<Entry>, DataError> {
    let end = find_last(bytes, &END)
        .ok_or_else(|| DataError::Malformed("no zip end-of-directory record".into()))?;
    let mut at = u32::from_le_bytes(take(bytes, end + 16, 4)?.try_into().unwrap()) as usize;
    let mut count = u16::from_le_bytes(take(bytes, end + 10, 2)?.try_into().unwrap()) as usize;

    // Zip64: the 32-bit fields are saturated and the real ones live in the
    // zip64 record. numpy writes this for arrays over 4 GB.
    if count == 0xffff || at == 0xffff_ffff {
        let z = find_last(bytes, &END64)
            .ok_or_else(|| DataError::Malformed("a zip64 archive with no zip64 record".into()))?;
        count = u64::from_le_bytes(take(bytes, z + 32, 8)?.try_into().unwrap()) as usize;
        at = u64::from_le_bytes(take(bytes, z + 48, 8)?.try_into().unwrap()) as usize;
    }

    let mut entries = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        if take(bytes, at, 4)? != CENTRAL {
            return Err(DataError::Malformed(
                "the zip directory ends earlier than it claimed".into(),
            ));
        }
        let u16_at = |o: usize| -> Result<u16, DataError> {
            Ok(u16::from_le_bytes(
                take(bytes, at + o, 2)?.try_into().unwrap(),
            ))
        };
        let u32_at = |o: usize| -> Result<u32, DataError> {
            Ok(u32::from_le_bytes(
                take(bytes, at + o, 4)?.try_into().unwrap(),
            ))
        };
        let flags = u16_at(8)?;
        let name_len = u16_at(28)? as usize;
        let extra_len = u16_at(30)? as usize;
        let comment_len = u16_at(32)? as usize;
        let name = String::from_utf8_lossy(take(bytes, at + 46, name_len)?).into_owned();
        entries.push(Entry {
            name,
            method: u16_at(10)?,
            compressed: u32_at(20)? as usize,
            uncompressed: u32_at(24)? as usize,
            offset: u32_at(42)? as usize,
            // Bit 0 is the traditional-encryption flag.
            encrypted: flags & 1 != 0,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

fn read_member(bytes: &[u8], e: &Entry) -> Result<Vec<u8>, DataError> {
    if e.encrypted {
        return Err(DataError::Unsupported(format!(
            "{} is encrypted, and lilook does not ask for passwords",
            e.name
        )));
    }
    // The local header repeats the name and may carry a longer extra field than
    // the central one, so its length has to be read here rather than assumed.
    let name_len = u16::from_le_bytes(take(bytes, e.offset + 26, 2)?.try_into().unwrap()) as usize;
    let extra_len = u16::from_le_bytes(take(bytes, e.offset + 28, 2)?.try_into().unwrap()) as usize;
    let start = e.offset + 30 + name_len + extra_len;
    let raw = take(bytes, start, e.compressed)?;
    match e.method {
        0 => Ok(raw.to_vec()),
        8 => {
            use flate2::read::DeflateDecoder;
            use std::io::Read as _;
            let mut out = Vec::with_capacity(e.uncompressed);
            DeflateDecoder::new(raw)
                .read_to_end(&mut out)
                .map_err(|err| DataError::Malformed(format!("{}: {err}", e.name)))?;
            Ok(out)
        }
        other => Err(DataError::Unsupported(format!(
            "{} uses compression method {other}, which lilook does not read",
            e.name
        ))),
    }
}

/// The last occurrence of a signature. Last, because the end-of-directory record
/// is at the end and a member's *contents* can contain the same four bytes.
fn find_last(bytes: &[u8], sig: &[u8; 4]) -> Option<usize> {
    (0..bytes.len().saturating_sub(3))
        .rev()
        .find(|&i| &bytes[i..i + 4] == sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored-only zip, built here so the reader is tested against the layout
    /// rather than against a writer of its own.
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, method
            out.extend_from_slice(&[0; 4]); // time, date
            out.extend_from_slice(&0u32.to_le_bytes()); // crc, unchecked here
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&CENTRAL);
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]); // versions, flags, method
            central.extend_from_slice(&[0; 4]); // time, date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&[0; 8]); // disk, attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let dir_at = out.len() as u32;
        let dir_len = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&END);
        out.extend_from_slice(&[0; 4]); // disks
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&dir_len.to_le_bytes());
        out.extend_from_slice(&dir_at.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out
    }

    #[test]
    fn stored_members_come_back_with_their_names() {
        let z = zip_of(&[("t.npy", b"first"), ("y.npy", b"second")]);
        let m = members(&z).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], ("t.npy".to_string(), b"first".to_vec()));
        assert_eq!(m[1], ("y.npy".to_string(), b"second".to_vec()));
    }

    #[test]
    fn a_deflated_member_is_inflated() {
        use flate2::write::DeflateEncoder;
        use std::io::Write as _;
        let raw = b"the same four bytes PK\x05\x06 appear inside this member".repeat(4);
        let mut e = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(&raw).unwrap();
        let packed = e.finish().unwrap();

        // Same layout as `zip_of`, but method 8 and two different sizes.
        let mut z = zip_of(&[("v.npy", &packed)]);
        // Patch both method fields to deflate.
        let local_method = 8;
        z[8] = 8;
        let dir = find_last(&z, &CENTRAL).unwrap();
        z[dir + 10] = local_method;
        // And tell the directory the uncompressed size.
        z[dir + 24..dir + 28].copy_from_slice(&(raw.len() as u32).to_le_bytes());

        let m = members(&z).unwrap();
        assert_eq!(m[0].1, raw);
    }

    #[test]
    fn a_broken_archive_says_so() {
        assert!(members(b"not a zip").is_err());
        // Truncated after the signature.
        assert!(members(b"PK\x05\x06").is_err());
        // A directory that claims more entries than it has.
        let mut z = zip_of(&[("t.npy", b"x")]);
        let end = find_last(&z, &END).unwrap();
        z[end + 10..end + 12].copy_from_slice(&9u16.to_le_bytes());
        assert!(members(&z).is_err());
    }
}
