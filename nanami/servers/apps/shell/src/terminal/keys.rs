//! Set-1 navigation keys (PS/2 and USB's 0x100 extended-key flag).
pub fn sequence(code: usize, application_cursor: bool) -> &'static [u8] {
    match code & 0xff {
        0x01 => b"\x1b",
        0x0e => b"\x7f",
        0x0f => b"\t",
        0x1c => b"\r",
        0x48 => {
            if application_cursor {
                b"\x1bOA"
            } else {
                b"\x1b[A"
            }
        }
        0x50 => {
            if application_cursor {
                b"\x1bOB"
            } else {
                b"\x1b[B"
            }
        }
        0x4d => {
            if application_cursor {
                b"\x1bOC"
            } else {
                b"\x1b[C"
            }
        }
        0x4b => {
            if application_cursor {
                b"\x1bOD"
            } else {
                b"\x1b[D"
            }
        }
        0x47 => {
            if application_cursor {
                b"\x1bOH"
            } else {
                b"\x1b[H"
            }
        }
        0x4f => {
            if application_cursor {
                b"\x1bOF"
            } else {
                b"\x1b[F"
            }
        }
        0x52 => b"\x1b[2~",
        0x53 => b"\x1b[3~",
        0x49 => b"\x1b[5~",
        0x51 => b"\x1b[6~",
        _ => b"",
    }
}
