//! The whole transcode path, on files built the way the tools that write them do.
//!
//! The exit criterion for the binary formats: a `.npz`, a `.fits` and a Veusz
//! descriptor `.dat` become a CBOR sidecar, and the values that come back are the
//! ones an independent calculation says they should be. Whether typst can *read*
//! the sidecar is checked in `lilook-compile`, which has a compiler.

use lilook_data::{decode, sniff, Dataset, Format};

/// A `.npy` as numpy writes one.
fn npy(descr: &str, shape: &str, data: &[u8]) -> Vec<u8> {
    let dict = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let mut header = dict.into_bytes();
    while (10 + header.len()) % 64 != 0 {
        header.push(b' ');
    }
    header.push(b'\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    out
}

/// A stored zip, as `np.savez` writes one.
fn npz(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in members {
        let offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    let dir_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&dir_len.to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn f64s(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The values every fixture holds, computed here so the assertions do not just
/// restate what the decoder produced.
fn expected() -> (Vec<f64>, Vec<f64>) {
    let t: Vec<f64> = (0..16).map(|i| i as f64 * 0.5).collect();
    let y: Vec<f64> = t.iter().map(|t| t.sin()).collect();
    (t, y)
}

#[test]
fn an_npz_transcodes_to_a_sidecar_with_its_member_names() {
    let (t, y) = expected();
    let file = npz(&[
        ("t.npy", npy("<f8", "(16,)", &f64s(&t))),
        ("flux.npy", npy("<f8", "(16,)", &f64s(&y))),
    ]);
    assert_eq!(sniff(&file, "run.npz"), Some(Format::Npz));

    let d = decode(&file, Format::Npz).unwrap();
    assert_eq!(d.names(), ["t", "flux"], "members keep their keyword names");
    assert_eq!(d.column("t").unwrap().values, t);
    assert_eq!(d.column("flux").unwrap().values, y);

    // And the sidecar is a CBOR map of exactly those two arrays.
    let sidecar = d.to_cbor();
    assert_eq!(sidecar[0], 0xa2, "a two-entry map");
    assert_eq!(sidecar.len(), 1 + (2 + 1 + 16 * 9) + (5 + 1 + 16 * 9));
}

#[test]
fn a_fits_bintable_transcodes_to_a_sidecar() {
    let (t, y) = expected();
    let mut data = Vec::new();
    for (a, b) in t.iter().zip(&y) {
        data.extend_from_slice(&a.to_be_bytes());
        data.extend_from_slice(&(*b as f32).to_be_bytes());
    }
    let mut file = String::new();
    for c in [
        "SIMPLE  =                    T",
        "BITPIX  =                    8",
        "NAXIS   =                    0",
    ] {
        file.push_str(&format!("{c:<80}"));
    }
    file.push_str(&format!("{:<80}", "END"));
    while !file.len().is_multiple_of(2880) {
        file.push(' ');
    }
    let mut header = String::new();
    for c in [
        "XTENSION= 'BINTABLE'",
        "BITPIX  =                    8",
        "NAXIS   =                    2",
        "NAXIS1  =                   12",
        "NAXIS2  =                   16",
        "TFIELDS =                    2",
        "TFORM1  = 'D       '",
        "TTYPE1  = 't       '",
        "TFORM2  = 'E       '",
        "TTYPE2  = 'flux    '",
    ] {
        header.push_str(&format!("{c:<80}"));
    }
    header.push_str(&format!("{:<80}", "END"));
    while !header.len().is_multiple_of(2880) {
        header.push(' ');
    }
    let mut bytes = file.into_bytes();
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&data);
    while !bytes.len().is_multiple_of(2880) {
        bytes.push(0);
    }

    assert_eq!(sniff(&bytes, "run.fits"), Some(Format::Fits));
    let d = decode(&bytes, Format::Fits).unwrap();
    assert_eq!(d.names(), ["t", "flux"]);
    assert_eq!(d.column("t").unwrap().values, t);
    // Single-precision, so compare at single-precision.
    for (got, want) in d.column("flux").unwrap().values.iter().zip(&y) {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }
}

#[test]
fn a_veusz_descriptor_file_transcodes_with_its_error_column_named() {
    let (t, y) = expected();
    let mut text = String::from("# a run\ndescriptor t flux +-\n");
    for (a, b) in t.iter().zip(&y) {
        text.push_str(&format!("{a} {b} {}\n", b.abs() * 0.1));
    }
    let bytes = text.into_bytes();
    assert_eq!(sniff(&bytes, "run.dat"), Some(Format::Ascii));

    let d = decode(&bytes, Format::Ascii).unwrap();
    assert_eq!(d.names(), ["t", "flux", "flux_err"]);
    assert_eq!(d.column("t").unwrap().values, t);
    assert_eq!(d.column("flux").unwrap().values, y);
    let err = &d.column("flux_err").unwrap().values;
    for (got, want) in err.iter().zip(y.iter().map(|v| v.abs() * 0.1)) {
        assert!((got - want).abs() < 1e-12);
    }
}

/// The measurement that justifies a sidecar over `read()`: a slice of a large
/// file is small, where typst's own `read` would have to load and hash all of it
/// on every compile.
#[test]
fn a_sidecar_is_a_slice_rather_than_a_copy() {
    let n = 100_000;
    let t: Vec<f64> = (0..n).map(|i| i as f64).collect();
    let members: Vec<(&str, Vec<u8>)> = vec![
        ("t.npy", npy("<f8", &format!("({n},)"), &f64s(&t))),
        ("flux.npy", npy("<f8", &format!("({n},)"), &f64s(&t))),
        ("unused1.npy", npy("<f8", &format!("({n},)"), &f64s(&t))),
        ("unused2.npy", npy("<f8", &format!("({n},)"), &f64s(&t))),
    ];
    let file = npz(&members);
    let d = decode(&file, Format::Npz).unwrap();
    assert_eq!(d.columns.len(), 4);

    let two = d.select(&["t".into(), "flux".into()]);
    let sidecar = two.to_cbor();
    let ratio = sidecar.len() as f64 / file.len() as f64;
    eprintln!(
        "origin {} bytes, sidecar {} bytes ({:.0}%)",
        file.len(),
        sidecar.len(),
        ratio * 100.0
    );
    // Two of four columns, at 9 bytes per value against 8: a little over half.
    assert!(ratio < 0.6, "{ratio}");
    assert_eq!(Dataset::default().to_cbor(), vec![0xa0]);
}
