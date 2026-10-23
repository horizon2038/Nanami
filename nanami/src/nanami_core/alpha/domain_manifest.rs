use alloc::vec::Vec;
use nanami_uapi::alpha::ALPHA_IMPORT_CAPACITY;
use nun::CapabilityError;

#[derive(Clone, Copy, Debug)]
pub(super) struct ImportSpec<'a> {
    pub child: &'a str,
    pub name: &'a str,
    pub source: &'a str,
    pub sessions: usize,
}

impl<'a> ImportSpec<'a> {
    pub fn parse(line: &'a str) -> Result<Option<Self>, CapabilityError> {
        let mut tokens = line.split('#').next().unwrap_or("").split_whitespace();
        if tokens.next() != Some("@import") {
            return Ok(None);
        }
        let invalid = || CapabilityError::InvalidArgument;
        let child = tokens
            .next()
            .filter(|name| name.len() <= 32)
            .ok_or_else(invalid)?;
        let mut name = || {
            tokens
                .next()
                .filter(|name| name.len() <= 24 && !name.as_bytes().contains(&0))
                .ok_or_else(invalid)
        };
        let local = name()?;
        let source = name()?;
        let raw = tokens.next().ok_or_else(invalid)?;
        if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let sessions = raw
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=64).contains(limit))
            .ok_or_else(invalid)?;
        if tokens.next().is_some() {
            return Err(invalid());
        }
        Ok(Some(Self {
            child,
            name: local,
            source,
            sessions,
        }))
    }
}

/// Parse before launching anything, so a malformed or ambiguous import policy
/// cannot silently turn into an unbounded/default grant.
pub(super) fn imports(configuration: &str) -> Result<Vec<ImportSpec<'_>>, CapabilityError> {
    let mut imports: Vec<ImportSpec<'_>> = Vec::new();
    for line in configuration.lines() {
        if let Some(spec) = ImportSpec::parse(line)? {
            let children = configuration
                .lines()
                .filter_map(|line| ChildSpec::parse(line).ok().flatten())
                .filter(|child| child.label == spec.child)
                .count();
            if children != 1
                || imports
                    .iter()
                    .any(|other| other.child == spec.child && other.name == spec.name)
                || imports
                    .iter()
                    .filter(|other| other.child == spec.child)
                    .count()
                    == ALPHA_IMPORT_CAPACITY
            {
                return Err(CapabilityError::InvalidArgument);
            }
            imports.push(spec);
        }
    }
    Ok(imports)
}

/// External child-domain policy; none of these names have service semantics.
#[derive(Clone, Copy)]
pub(super) struct ChildSpec<'a> {
    pub(super) label: &'a str,
    pub(super) radix: usize,
    pub(super) image: &'a str,
    pub(super) archive: &'a str,
    pub(super) configuration: &'a str,
    pub(super) after: Option<&'a str>,
}
impl<'a> ChildSpec<'a> {
    pub(super) fn parse(line: &'a str) -> Result<Option<Self>, CapabilityError> {
        let mut tokens = line.split('#').next().unwrap_or("").split_whitespace();
        if tokens.next() != Some("@alpha") {
            return Ok(None);
        }
        let invalid = || CapabilityError::InvalidArgument;
        let label = tokens
            .next()
            .filter(|label| label.len() <= 32)
            .ok_or_else(invalid)?;
        let raw = tokens.next().ok_or_else(invalid)?;
        if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let radix = raw
            .parse::<usize>()
            .ok()
            .filter(|radix| (29..usize::BITS as usize).contains(radix))
            .ok_or_else(invalid)?;
        let mut spec = Self {
            label,
            radix,
            image: tokens.next().ok_or_else(invalid)?,
            archive: tokens.next().ok_or_else(invalid)?,
            configuration: tokens.next().ok_or_else(invalid)?,
            after: None,
        };
        if let Some(option) = tokens.next() {
            spec.after = Some(
                option
                    .strip_prefix("after=")
                    .filter(|name| !name.is_empty() && name.len() <= 24)
                    .ok_or_else(invalid)?,
            );
        }
        if tokens.next().is_some() {
            return Err(invalid());
        }
        Ok(Some(spec))
    }
}
