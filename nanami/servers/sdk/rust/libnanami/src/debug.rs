use core::fmt;

pub fn write_char(c: char) {
    a9n_abi::arch::debug_call::write_char(c);
}

pub fn write_string(s: &str) {
    for b in s.as_bytes().iter().copied() {
        let c = if b.is_ascii() { b as char } else { '?' };
        write_char(c);
    }
}

fn write_server_tagged_line_prefix(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('[') else {
        return false;
    };
    let Some(end) = rest.find(']') else {
        return false;
    };

    let name = &rest[..end];
    let max = 12usize;
    let mut ascii_name = [0u8; 12];
    let mut used = 0usize;
    for b in name.as_bytes().iter().copied() {
        if !b.is_ascii() {
            return false;
        }
        if used >= max {
            break;
        }
        ascii_name[used] = b;
        used += 1;
    }
    if used == 0 || used > max {
        return false;
    }

    write_char('[');
    let pad = max.saturating_sub(used);
    for _ in 0..pad {
        write_char(' ');
    }
    let mut i = 0usize;
    while i < used {
        write_char(ascii_name[i] as char);
        i += 1;
    }
    write_char(']');
    write_string(&rest[end + 1..]);
    true
}

struct DebugWriter;

impl fmt::Write for DebugWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if !write_server_tagged_line_prefix(s) {
            write_string(s);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let mut writer = DebugWriter;
    let _ = fmt::write(&mut writer, args);
}

pub fn println(args: fmt::Arguments<'_>) {
    print(args);
    write_char('\r');
    write_char('\n');
}

pub fn print_char(c: char) {
    write_char(c);
}
