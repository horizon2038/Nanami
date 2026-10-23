use anyhow::{ensure, Result};

/// Write the portable SVR4 newc format used by Alpha, including deterministic
/// inode numbers and zero timestamps. Every name/data field is aligned to 4.
pub fn archive(entries: &[(String, u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for (i, (name, mode, data)) in entries.iter().enumerate() {
        entry(&mut bytes, i as u32 + 1, name, *mode, data)?;
    }
    entry(&mut bytes, 0, "TRAILER!!!", 0, &[])?;
    bytes.resize(bytes.len().div_ceil(512) * 512, 0);
    Ok(bytes)
}
fn entry(out: &mut Vec<u8>, inode: u32, name: &str, mode: u32, data: &[u8]) -> Result<()> {
    ensure!(
        !name.as_bytes().contains(&0)
            && data.len() <= u32::MAX as usize
            && name.len() < u32::MAX as usize,
        "newc entry is invalid or too large"
    );
    out.extend_from_slice(b"070701");
    for value in [
        inode,
        mode,
        0,
        0,
        1,
        0,
        data.len() as u32,
        0,
        0,
        0,
        0,
        name.len() as u32 + 1,
        0,
    ] {
        out.extend_from_slice(format!("{value:08x}").as_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.resize(out.len().div_ceil(4) * 4, 0);
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(4) * 4, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    mod guest_parser {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nanami/src/nanami_core/cpio.rs"
        ));
    }
    // The production parser only needs this error variant from the Nun ABI.
    // See the crate-root alias in the test-only declaration in main.rs.
    #[test]
    fn production_alpha_parser_reads_rust_archive() {
        let records = vec![
            ("./nanami/boot-list".into(), 0o100644, b"startup".to_vec()),
            ("./bin/test".into(), 0o100755, vec![1, 2, 3, 4, 5]),
        ];
        let bytes = archive(&records).unwrap();
        let mut actual = Vec::new();
        guest_parser::for_each_newc_entry(&bytes, |e| {
            actual.push((e.name.to_owned(), e.data.to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            actual,
            records
                .iter()
                .map(|(n, _, b)| (n.clone(), b.clone()))
                .collect::<Vec<_>>()
        );
        assert_eq!(bytes.len() % 512, 0);
        assert_eq!(bytes, archive(&records).unwrap());
    }

    #[test]
    fn explicit_module_paths_and_ambiguous_legacy_names() {
        let records = vec![
            ("./bin/tool".into(), 0o100755, vec![1]),
            ("./modules/tool".into(), 0o100755, vec![2]),
        ];
        let bytes = archive(&records).unwrap();
        assert_eq!(
            guest_parser::find_image(&bytes, "./modules/tool")
                .unwrap()
                .data,
            [2]
        );
        assert_eq!(
            guest_parser::find_image(&bytes, "/bin/tool.elf")
                .unwrap()
                .data,
            [1]
        );
        assert!(guest_parser::find_image(&bytes, "tool").is_err());
        assert!(guest_parser::find_image(&bytes, "other/tool").is_err());
    }
}
