//! Trusted boot-configuration policy. Image and component names have no built-in meaning.
use alloc::vec::Vec;
use nun::{CapabilityError, Word};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ProcessPolicy {
    pub priority: Option<Word>,
    pub affinity: Option<Word>,
    pub platform_information: bool,
    pub service_admin: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ComponentSpec<'a> {
    pub name: &'a str,
    pub priority: Word,
    pub image_path: &'a str,
}

impl<'a> ComponentSpec<'a> {
    pub fn parse(line: &'a str) -> Result<Option<Self>, CapabilityError> {
        let mut tokens = line.split('#').next().unwrap_or("").split_whitespace();
        let Some(name) = tokens.next() else {
            return Ok(None);
        };
        if name.starts_with('@') {
            return Err(CapabilityError::InvalidArgument);
        }
        let raw_priority = tokens.next().ok_or(CapabilityError::InvalidArgument)?;
        if !raw_priority.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CapabilityError::InvalidArgument);
        }
        let priority = raw_priority
            .parse::<Word>()
            .map_err(|_| CapabilityError::InvalidArgument)?;
        let image_path = tokens.next().ok_or(CapabilityError::InvalidArgument)?;
        if priority > 31 || tokens.next().is_some() {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(Some(Self {
            name,
            priority,
            image_path,
        }))
    }
}

impl ProcessPolicy {
    pub fn priority(self, explicit: Option<Word>) -> Result<Word, CapabilityError> {
        let priority = explicit.or(self.priority).unwrap_or(4);
        if priority > 31 {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(priority)
    }
}

pub(super) struct ProcessPolicies<'a> {
    entries: Vec<(&'a str, ProcessPolicy)>,
}

fn path_key(path: &str) -> &str {
    path.strip_prefix("./")
        .unwrap_or(path)
        .trim_start_matches('/')
}

impl<'a> ProcessPolicies<'a> {
    pub fn parse(configuration: &'a str, cores: Word) -> Result<Self, CapabilityError> {
        if !(1..=64).contains(&cores) {
            return Err(CapabilityError::InvalidArgument);
        }
        let mut entries: Vec<(&'a str, ProcessPolicy)> = Vec::new();
        for line in configuration.lines() {
            let mut tokens = line.split('#').next().unwrap_or("").split_whitespace();
            if tokens.next() != Some("@process") {
                continue;
            }
            let image = tokens.next().ok_or(CapabilityError::InvalidArgument)?;
            if path_key(image).is_empty()
                || entries
                    .iter()
                    .any(|(name, _)| path_key(name) == path_key(image))
            {
                return Err(CapabilityError::InvalidArgument);
            }
            let mut policy = ProcessPolicy::default();
            let mut options = 0;
            let mut platform_seen = false;
            let mut services_seen = false;
            for option in tokens {
                let (key, value) = option
                    .split_once('=')
                    .ok_or(CapabilityError::InvalidArgument)?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(CapabilityError::InvalidArgument);
                }
                let value = value
                    .parse::<Word>()
                    .map_err(|_| CapabilityError::InvalidArgument)?;
                match key {
                    "priority" if policy.priority.is_none() && value <= 31 => {
                        policy.priority = Some(value);
                    }
                    "affinity" if policy.affinity.is_none() && value < cores => {
                        policy.affinity = Some(value);
                    }
                    "platform" if !platform_seen && value <= 1 => {
                        platform_seen = true;
                        policy.platform_information = value != 0;
                    }
                    "services" if !services_seen && value <= 1 => {
                        services_seen = true;
                        policy.service_admin = value != 0;
                    }
                    _ => return Err(CapabilityError::InvalidArgument),
                }
                options += 1;
            }
            if options == 0 {
                return Err(CapabilityError::InvalidArgument);
            }
            entries.push((image, policy));
        }
        Ok(Self { entries })
    }

    /// Only configured initramfs images receive these defaults or platform access.
    /// Caller-supplied ELF buffers do not select policy with an arbitrary label.
    pub fn for_image(&self, image: &str) -> ProcessPolicy {
        self.entries
            .iter()
            .find(|(name, _)| path_key(name) == path_key(image))
            .map(|(_, policy)| *policy)
            .unwrap_or_default()
    }
}
