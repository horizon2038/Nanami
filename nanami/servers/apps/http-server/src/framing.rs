//! Bounded HTTP/1 request framing. Requests with bodies are rejected/closed,
//! rather than treating body bytes as a pipelined request (RFC 9112 section 6).
pub const MAX_HEADER: usize = 8192;
pub const MAX_PENDING: usize = 2 * MAX_HEADER;
pub struct Request {
    pub bytes: usize,
    pub head: bool,
    pub keep_alive: bool,
}
pub fn request(bytes: &[u8]) -> Result<Option<Request>, ()> {
    let Some(end) = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|n| n + 4)
    else {
        if bytes.len() >= MAX_HEADER {
            return Err(());
        }
        return Ok(None);
    };
    if end > MAX_HEADER {
        return Err(());
    }
    let mut lines = bytes[..end - 2].split(|b| *b == b'\n');
    let line = lines.next().ok_or(())?;
    let line = line.strip_suffix(b"\r").ok_or(())?;
    let mut parts = line.split(|b| *b == b' ');
    let method = parts.next().ok_or(())?;
    let path = parts.next().ok_or(())?;
    let version = parts.next().ok_or(())?;
    if !matches!(method, b"GET" | b"HEAD")
        || !path.starts_with(b"/")
        || path.iter().any(|b| *b <= 32 || *b == 127)
        || !matches!(version, b"HTTP/1.0" | b"HTTP/1.1")
        || parts.next().is_some()
    {
        return Err(());
    }
    let mut hosts = 0;
    let mut content_length = false;
    let mut close = false;
    let mut persistent = version == b"HTTP/1.1";
    for line in lines {
        if line.is_empty() {
            break;
        }
        let line = line.strip_suffix(b"\r").ok_or(())?;
        let colon = line.iter().position(|b| *b == b':').ok_or(())?;
        let name = &line[..colon];
        if name.is_empty()
            || !name
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
        {
            return Err(());
        }
        let mut value = &line[colon + 1..];
        while matches!(value.first(), Some(b' ' | b'\t')) {
            value = &value[1..];
        }
        while matches!(value.last(), Some(b' ' | b'\t')) {
            value = &value[..value.len() - 1];
        }
        if value.iter().any(|b| (*b < 32 && *b != b'\t') || *b == 127) {
            return Err(());
        }
        if name.eq_ignore_ascii_case(b"host") {
            hosts += 1;
            if value.is_empty() || hosts > 1 {
                return Err(());
            }
        }
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Err(());
        }
        if name.eq_ignore_ascii_case(b"connection") {
            for token in value.split(|b| *b == b',') {
                let token = token.trim_ascii();
                close |= token.eq_ignore_ascii_case(b"close");
                persistent |= token.eq_ignore_ascii_case(b"keep-alive");
            }
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            if content_length || value.is_empty() || !value.iter().all(|b| *b == b'0') {
                return Err(());
            }
            content_length = true;
        }
    }
    if version == b"HTTP/1.1" && hosts != 1 {
        return Err(());
    }
    Ok(Some(Request {
        bytes: end,
        head: method == b"HEAD",
        keep_alive: persistent && !close,
    }))
}
