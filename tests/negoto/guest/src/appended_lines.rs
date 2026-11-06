/// Return only newly completed log lines, retaining an unfinished suffix for
/// the next snapshot. A changed prefix denotes replacement or truncation.
pub fn complete_appended_lines<'a>(previous: &[u8], current: &'a [u8]) -> &'a [u8] {
    let start = if current.starts_with(previous) {
        previous
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |pos| pos + 1)
    } else {
        0
    };
    let end = current
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |pos| pos + 1);
    &current[start..end]
}
