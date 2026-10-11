// Kept independent of the Nanami allocator and syscall ABI so the wire format
// can also be tested on a host with `rustc --test`.
pub(crate) fn decode_name(bytes: &[u8; 32]) -> Option<&str> {
    let len = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    if len == 0 {
        return None;
    }
    core::str::from_utf8(&bytes[..len]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stop_at_the_first_nul() {
        let mut bytes = [0xff; 32];
        bytes[..5].copy_from_slice(b"pc99\0");
        assert_eq!(decode_name(&bytes), Some("pc99"));
    }

    #[test]
    fn names_preserve_all_32_bytes() {
        let bytes = *b"0123456789abcdefghijklmnopqrstuv";
        assert_eq!(
            decode_name(&bytes),
            Some("0123456789abcdefghijklmnopqrstuv")
        );
    }

    #[test]
    fn empty_and_invalid_utf8_names_are_rejected() {
        assert_eq!(decode_name(&[0; 32]), None);
        assert_eq!(decode_name(&[0xff; 32]), None);
    }
}
