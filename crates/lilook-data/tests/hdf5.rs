//! HDF5, against a file libhdf5 itself wrote.
//!
//! Only runs with `--features hdf5`, which is off by default because it needs a
//! C library. `scripts/check.sh` keeps the default build honest about that.

#![cfg(all(feature = "hdf5", not(target_arch = "wasm32")))]

use lilook_data::{hdf5 as reader, DataError};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("lilook-hdf5-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn nested_groups_and_every_width_read_as_columns() {
    let path = scratch("run.h5");
    {
        let f = reader::lib::File::create(&path).unwrap();
        let g = f.create_group("results").unwrap();
        g.new_dataset::<f64>()
            .shape([4])
            .create("t")
            .unwrap()
            .write_raw(&[0.0f64, 0.5, 1.0, 1.5])
            .unwrap();
        // A different width, to prove the type is read rather than assumed.
        g.new_dataset::<i32>()
            .shape([4])
            .create("count")
            .unwrap()
            .write_raw(&[1i32, 2, 3, 4])
            .unwrap();
        // A 2-D dataset becomes one column per column.
        f.new_dataset::<f32>()
            .shape([2, 3])
            .create("grid")
            .unwrap()
            .write_raw(&[1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0])
            .unwrap();
        // A string dataset is not a column of numbers and must be skipped.
        let labels: Vec<reader::lib::types::VarLenUnicode> =
            vec!["a".parse().unwrap(), "b".parse().unwrap()];
        f.new_dataset::<reader::lib::types::VarLenUnicode>()
            .shape([2])
            .create("labels")
            .unwrap()
            .write_raw(&labels)
            .unwrap();
        // Three dimensions have no obvious columns either.
        f.new_dataset::<f64>()
            .shape([2, 2, 2])
            .create("cube")
            .unwrap()
            .write_raw(&[0.0f64; 8])
            .unwrap();
    }

    let d = reader::read_path(&path).unwrap();
    let names = d.names();
    // The full path is the name, so /results/t and /reference/t stay distinct.
    assert!(names.contains(&"/results/t".to_string()), "{names:?}");
    assert_eq!(d.column("/results/t").unwrap().values, [0.0, 0.5, 1.0, 1.5]);
    assert_eq!(
        d.column("/results/count").unwrap().values,
        [1.0, 2.0, 3.0, 4.0]
    );
    // Two rows of three, so three columns of two.
    assert_eq!(d.column("/grid[0]").unwrap().values, [1.0, 4.0]);
    assert_eq!(d.column("/grid[2]").unwrap().values, [3.0, 6.0]);
    assert!(
        !names.iter().any(|n| n.contains("labels")),
        "a string dataset is not a column: {names:?}"
    );

    // And the sidecar the document will actually link.
    let picked = d.select(&["/results/t".into(), "/results/count".into()]);
    assert_eq!(picked.columns.len(), 2);
    assert_eq!(picked.to_cbor()[0], 0xa2);
}

#[test]
fn a_file_that_is_not_hdf5_says_so_rather_than_panicking() {
    let path = scratch("not.h5");
    std::fs::write(&path, b"this is not an HDF5 file").unwrap();
    assert!(matches!(
        reader::read_path(&path),
        Err(DataError::Malformed(_))
    ));
    // And the bytes form is refused with a reason, not silently wrong.
    assert!(matches!(
        reader::read(b"anything"),
        Err(DataError::Unsupported(_))
    ));
}
