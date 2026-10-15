//! A small, host-independent terminfo entry for Shell's VT100-compatible mode.
//!
//! Emit the standard little-endian, 16-bit format directly so image creation
//! does not depend on the host's tic, ncurses version, or personal database.
//! Capability indices are the stable SVr4/term.h order documented by ncurses:
//! https://invisible-island.net/ncurses/man/term.5.html
//! Only advertise sequences implemented by Shell; in particular, do not claim
//! DEC line-drawing, programmable tabs, function keys, or blink support.
use ext2_image::ImageBuilder;
use std::io;

const NAMES: &[u8] = b"vt100|Nanami VT100-compatible terminal\0";
const BOOLEANS: &[u8] = &[0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]; // am=1, xenl=4, msgr=14
const NUMBERS: &[i16] = &[80, 8, 24]; // cols, it, lines; TIOCGWINSZ overrides size.
const STRINGS: &[(usize, &[u8])] = &[
    (2, b"\r"),                   // cr
    (3, b"\x1b[%i%p1%d;%p2%dr"),  // csr
    (5, b"\x1b[H\x1b[J"),         // clear
    (6, b"\x1b[K"),               // el
    (7, b"\x1b[J"),               // ed
    (10, b"\x1b[%i%p1%d;%p2%dH"), // cup
    (11, b"\x1b[B"),              // cud1
    (12, b"\x1b[H"),              // home
    (13, b"\x1b[?25l"),           // civis
    (14, b"\x08"),                // cub1
    (16, b"\x1b[?25h"),           // cnorm
    (17, b"\x1b[C"),              // cuf1
    (19, b"\x1b[A"),              // cuu1
    (27, b"\x1b[1m"),             // bold
    (34, b"\x1b[7m"),             // rev
    (35, b"\x1b[7m"),             // smso
    (36, b"\x1b[4m"),             // smul
    (39, b"\x1b[0m"),             // sgr0
    (43, b"\x1b[27m"),            // rmso
    (44, b"\x1b[24m"),            // rmul
    (55, b"\x7f"),                // kbs: matches Shell's Backspace key
    (61, b"\x1bOB"),              // kcud1
    (79, b"\x1bOD"),              // kcub1
    (83, b"\x1bOC"),              // kcuf1
    (87, b"\x1bOA"),              // kcuu1
    (88, b"\x1b[?1l"),            // rmkx: normal cursor keys
    (89, b"\x1b[?1h"),            // smkx: application cursor keys
    (107, b"\x1b[%p1%dB"),        // cud
    (111, b"\x1b[%p1%dD"),        // cub
    (112, b"\x1b[%p1%dC"),        // cuf
    (114, b"\x1b[%p1%dA"),        // cuu
    (126, b"\x1b8"),              // rc
    (128, b"\x1b7"),              // sc
    (129, b"\n"),                 // ind
    (130, b"\x1bM"),              // ri
    (134, b"\t"),                 // ht
    (269, b"\x1b[1K"),            // el1
];

pub fn install(builder: &mut ImageBuilder, guest: &str) -> io::Result<()> {
    let root = format!("/alter/{guest}/usr/share/terminfo");
    for suffix in [
        "/usr/share",
        "/usr/share/terminfo",
        "/usr/share/terminfo/v",
        "/usr/share/terminfo/76",
    ] {
        builder.add_directory(&format!("/alter/{guest}{suffix}"), 0o755)?;
    }
    builder.add_file(&format!("{root}/v/vt100"), 0o644, vt100())?;
    // ncurses built for case-insensitive filesystems uses hexadecimal names.
    builder.add_hard_link(&format!("{root}/76/vt100"), &format!("{root}/v/vt100"))?;
    Ok(())
}

pub fn vt100() -> Vec<u8> {
    let count = STRINGS.last().unwrap().0 + 1;
    let mut offsets = vec![-1i16; count];
    let mut table = Vec::new();
    for &(index, sequence) in STRINGS {
        offsets[index] = i16::try_from(table.len()).expect("terminfo string offset");
        table.extend_from_slice(sequence);
        table.push(0);
    }
    let mut bytes = Vec::new();
    for value in [
        0x011a,
        NAMES.len(),
        BOOLEANS.len(),
        NUMBERS.len(),
        count,
        table.len(),
    ] {
        bytes.extend_from_slice(
            &i16::try_from(value)
                .expect("terminfo header field")
                .to_le_bytes(),
        );
    }
    bytes.extend_from_slice(NAMES);
    bytes.extend_from_slice(BOOLEANS);
    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    for value in NUMBERS.iter().chain(offsets.iter()) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&table);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_has_portable_layout_and_only_declared_capabilities() {
        let bytes = vt100();
        let short = |offset| i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        assert_eq!(short(0), 0x011a);
        assert_eq!(&bytes[12..12 + NAMES.len()], NAMES);
        assert_eq!(short(4) as usize, BOOLEANS.len());
        assert_eq!(short(6) as usize, NUMBERS.len());
        let numbers_start = (12 + NAMES.len() + BOOLEANS.len() + 1) & !1;
        for (index, &number) in NUMBERS.iter().enumerate() {
            assert_eq!(short(numbers_start + 2 * index), number);
        }
        let strings_start = numbers_start + NUMBERS.len() * 2;
        let count = short(8) as usize;
        let table_start = strings_start + count * 2;
        assert_eq!(table_start + short(10) as usize, bytes.len());
        for index in 0..count {
            let offset = short(strings_start + index * 2);
            if let Some((_, sequence)) = STRINGS.iter().find(|&&(i, _)| i == index) {
                let start = table_start + offset as usize;
                assert_eq!(&bytes[start..start + sequence.len()], *sequence);
                assert_eq!(bytes[start + sequence.len()], 0);
            } else {
                assert_eq!(offset, -1);
            }
        }
    }

    #[test]
    fn both_guest_roots_contain_character_and_hex_directory_entries() {
        let mut builder = ImageBuilder::new(2 * 1024 * 1024).unwrap();
        builder.add_directory("/alter", 0o755).unwrap();
        for guest in ["linux", "freebsd"] {
            builder
                .add_directory(&format!("/alter/{guest}"), 0o755)
                .unwrap();
            builder
                .add_directory(&format!("/alter/{guest}/usr"), 0o755)
                .unwrap();
            install(&mut builder, guest).unwrap();
            for prefix in ["v", "76"] {
                assert!(
                    builder.contains(&format!("/alter/{guest}/usr/share/terminfo/{prefix}/vt100"))
                );
            }
        }
        let image = builder.build().unwrap();
        let entry = vt100();
        assert_eq!(
            image.windows(entry.len()).filter(|w| *w == entry).count(),
            2
        );
    }

    #[test]
    fn host_ncurses_reads_entry_when_infocmp_is_available() {
        let directory = tempfile::tempdir().unwrap();
        for prefix in ["v", "76"] {
            let entry = directory.path().join(prefix);
            std::fs::create_dir(&entry).unwrap();
            std::fs::write(entry.join("vt100"), vt100()).unwrap();
        }
        let result = std::process::Command::new("infocmp")
            .arg("-A")
            .arg(directory.path())
            .args(["-1", "vt100"])
            .output();
        let result = match result {
            Ok(result) => result,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return; // The build itself never needs a host ncurses tool.
            }
            Err(error) => panic!("infocmp: {error}"),
        };
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        for expected in [
            "cols#80",
            "lines#24",
            "am,",
            "xenl,",
            "msgr,",
            "kbs=",
            "smkx=\\E[?1h",
            "cup=\\E[%i%p1%d;%p2%dH",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        for unsupported in ["acsc=", "smacs=", "blink=", "kf1=", "colors#"] {
            assert!(
                !text.contains(unsupported),
                "unexpected {unsupported}: {text}"
            );
        }
    }
}
