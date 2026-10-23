//! Resource input for an Alpha instance, independent of A9N's init-only ABI.
//!
//! Descriptors refer to the receiving Alpha's capability space. Metadata never
//! creates authority: the sender must install the corresponding capabilities
//! before entering Alpha. All data below is owned inline, with no references to
//! the sender's stack or address space. Mapped inputs refer to mappings owned by
//! the receiving Alpha; the launcher copies their contents before resume.
use crate::Word;

pub const ALPHA_ENVIRONMENT_ABI: Word = 2;
pub const ALPHA_RESOURCE_CAPACITY: usize = 128;
pub const ALPHA_IMPORT_CAPACITY: usize = 16;
pub const RESOURCE_DEVICE: Word = 1;

/// One explicitly delegated upstream Session, not access to a parent registry.
/// The endpoint has WRITE|COPY, never READ or MODIFY. Downstream connections
/// share this upstream identity; their close handles belong to the local Alpha.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportedService {
    pub endpoint: Word,
    pub upstream_service: Word,
    pub upstream_session: Word,
    pub interface: Word,
    pub session_limit: Word,
    pub name_len: Word,
    pub name: [u8; 24],
}

impl ImportedService {
    pub fn name(&self) -> Result<&str, EnvironmentError> {
        let bytes = self
            .name
            .get(..self.name_len)
            .ok_or(EnvironmentError::Import)?;
        if bytes.is_empty() || bytes.contains(&0) {
            return Err(EnvironmentError::Import);
        }
        core::str::from_utf8(bytes).map_err(|_| EnvironmentError::Import)
    }

    pub fn validate(&self) -> Result<(), EnvironmentError> {
        self.name()?;
        if self.endpoint == 0
            || self.upstream_service == 0
            || self.upstream_session & crate::service::SESSION_IDENTIFIER_BIT == 0
            || self.interface == 0
            || !(1..=64).contains(&self.session_limit)
        {
            return Err(EnvironmentError::Import);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MappedInput {
    pub address: Word,
    pub bytes: Word,
}

impl MappedInput {
    pub fn validate(self) -> Result<(), EnvironmentError> {
        if self.bytes == 0 {
            if self.address != 0 {
                return Err(EnvironmentError::Input);
            }
        } else if self.address == 0
            || self
                .address
                .checked_add(self.bytes)
                .is_none_or(|end| end > (1usize << 47))
        {
            return Err(EnvironmentError::Input);
        }
        Ok(())
    }

    /// The launcher must have mapped these bytes in this process for its lifetime.
    pub unsafe fn as_slice(self) -> &'static [u8] {
        if self.bytes == 0 {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.bytes) }
    }
}

/// Receiver-local references to a stopped launcher's retained resources.
/// These are not an upstream resource-allocation service. No backing is reclaimed
/// merely because Suspend has returned; revocation/reuse needs separate proof.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetiredLauncher {
    pub process: Word,
    pub capability_space: Word,
    pub address_space: Word,
    pub frames: Word,
    pub page_tables: Word,
    pub ipc_buffer: Word,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryResource {
    pub capability: Word,
    pub address: Word,
    pub size_radix: Word,
    pub flags: Word,
    /// Already converted/reserved prefix, including alignment gaps. It is not
    /// available to either the physical allocator or further conversions.
    pub consumed_bytes: Word,
}

impl MemoryResource {
    pub const EMPTY: Self = Self {
        capability: 0,
        address: 0,
        size_radix: 0,
        flags: 0,
        consumed_bytes: 0,
    };

    pub fn is_device(self) -> bool {
        self.flags & RESOURCE_DEVICE != 0
    }

    pub fn size_bytes(self) -> Option<Word> {
        if self.size_radix >= Word::BITS as Word {
            return None;
        }
        Some(1usize << self.size_radix)
    }

    pub fn validate(self) -> Result<(), EnvironmentError> {
        let size = self.size_bytes().ok_or(EnvironmentError::Resource)?;
        if self.capability == 0
            || self.flags & !RESOURCE_DEVICE != 0
            || self.address.checked_add(size).is_none()
            || self.consumed_bytes > size
        {
            return Err(EnvironmentError::Resource);
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelfCapabilities {
    pub capability_space: Word,
    pub root_radix: Word,
    pub address_space: Word,
    pub process_control_block: Word,
    pub page_tables: Word,
    pub frames: Word,
    pub ipc_buffer: Word,
    /// Optional platform authority. A child without devices leaves these zero.
    pub interrupt_region: Word,
    pub io_ports: Word,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelVersion {
    pub major: Word,
    pub minor: Word,
    pub patch: Word,
    pub pre_release: [u8; 32],
    pub build_metadata: [u8; 32],
}

pub fn name(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformInformation {
    pub architecture: [u8; 32],
    pub platform: [u8; 32],
    pub core_count: Word,
    pub rsdp_address: Word,
    pub kernel: KernelVersion,
}

/// Boot display metadata only, not a framebuffer driver or mapping authority.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FramebufferInformation {
    pub display_id: Word,
    pub address: Word,
    pub size_bytes: Word,
    pub width: Word,
    pub height: Word,
    pub stride: Word,
    pub bits_per_pixel: Word,
    pub red_position: Word,
    pub red_size: Word,
    pub green_position: Word,
    pub green_size: Word,
    pub blue_position: Word,
    pub blue_size: Word,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AlphaEnvironment {
    pub abi_version: Word,
    pub size: Word,
    /// Local identifier assigned by the launcher; not a global PID or authority.
    pub domain_id: Word,
    pub capabilities: SelfCapabilities,
    pub platform: PlatformInformation,
    pub framebuffer: FramebufferInformation,
    /// Optional; First Alpha requires no parent connection.
    pub parent_control: Word,
    pub retired_launcher: RetiredLauncher,
    pub boot_archive: MappedInput,
    pub boot_configuration: MappedInput,
    pub resource_count: Word,
    pub bootstrap_resource: Word,
    pub resources: [MemoryResource; ALPHA_RESOURCE_CAPACITY],
    pub import_count: Word,
    pub imported_services: [ImportedService; ALPHA_IMPORT_CAPACITY],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    Abi,
    Capabilities,
    Platform,
    Resource,
    BootstrapResource,
    Input,
    Import,
}

impl Default for AlphaEnvironment {
    fn default() -> Self {
        Self {
            abi_version: ALPHA_ENVIRONMENT_ABI,
            size: core::mem::size_of::<Self>(),
            domain_id: 0,
            capabilities: SelfCapabilities::default(),
            platform: PlatformInformation::default(),
            framebuffer: FramebufferInformation::default(),
            parent_control: 0,
            retired_launcher: RetiredLauncher::default(),
            boot_archive: MappedInput::default(),
            boot_configuration: MappedInput::default(),
            resource_count: 0,
            bootstrap_resource: 0,
            resources: [MemoryResource::EMPTY; ALPHA_RESOURCE_CAPACITY],
            import_count: 0,
            imported_services: [ImportedService::default(); ALPHA_IMPORT_CAPACITY],
        }
    }
}

impl AlphaEnvironment {
    pub fn imports(&self) -> Result<&[ImportedService], EnvironmentError> {
        self.imported_services
            .get(..self.import_count)
            .ok_or(EnvironmentError::Import)
    }

    pub fn memory_resources(&self) -> Result<&[MemoryResource], EnvironmentError> {
        self.resources
            .get(..self.resource_count)
            .ok_or(EnvironmentError::Resource)
    }

    pub fn validate(&self) -> Result<(), EnvironmentError> {
        if self.abi_version != ALPHA_ENVIRONMENT_ABI || self.size != core::mem::size_of::<Self>() {
            return Err(EnvironmentError::Abi);
        }
        let caps = &self.capabilities;
        if caps.capability_space == 0
            || caps.address_space == 0
            || caps.process_control_block == 0
            || caps.root_radix == 0
            || caps.root_radix > Word::BITS as Word - 8
        {
            return Err(EnvironmentError::Capabilities);
        }
        if self.platform.core_count == 0 || self.platform.core_count > 64 {
            return Err(EnvironmentError::Platform);
        }
        self.boot_archive.validate()?;
        self.boot_configuration.validate()?;
        for (index, import) in self.imports()?.iter().enumerate() {
            import.validate()?;
            if self.imported_services[..index]
                .iter()
                .any(|other| other.name().ok() == import.name().ok())
            {
                return Err(EnvironmentError::Import);
            }
        }
        let resources = self.memory_resources()?;
        for (index, &resource) in resources.iter().enumerate() {
            resource.validate()?;
            if resources[..index]
                .iter()
                .any(|other| other.capability == resource.capability)
            {
                return Err(EnvironmentError::Resource);
            }
        }
        let bootstrap = resources
            .get(self.bootstrap_resource)
            .ok_or(EnvironmentError::BootstrapResource)?;
        if bootstrap.is_device() || bootstrap.consumed_bytes == bootstrap.size_bytes().unwrap() {
            return Err(EnvironmentError::BootstrapResource);
        }
        Ok(())
    }
}
