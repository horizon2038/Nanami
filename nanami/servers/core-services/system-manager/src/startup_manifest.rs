//! Rootfs startup entries retain their argument tail; labels are not argv[0].
#[derive(Clone, Copy)]
pub struct SystemEntry<'a> {
    pub name: &'a str,
    pub priority: usize,
    pub path: &'a str,
    pub arguments: &'a str,
}

pub fn parse_system_list_line(line: &str) -> Option<SystemEntry<'_>> {
    let line = line.split('#').next()?;
    let (name, rest) = token(line)?;
    let (priority, rest) = token(rest)?;
    let (path, arguments) = token(rest)?;
    let priority = priority
        .parse::<usize>()
        .ok()
        .filter(|value| *value <= 31)?;
    if line.contains('\0') {
        return None;
    }
    Some(SystemEntry {
        name,
        priority,
        path,
        arguments,
    })
}

fn token(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let (word, rest) = line.split_at(end);
    Some((word, rest.trim_start()))
}
