//! Alpha's own capability-space metadata. No A9N init-slot assumptions.
use nanami_uapi::alpha::AlphaEnvironment;
use nun::CapabilityDescriptor;

#[derive(Clone, Copy)]
pub struct RootCapabilitySpace {
    pub root_descriptor: CapabilityDescriptor,
    pub root_radix: usize,
    pub bootstrap_generic: CapabilityDescriptor,
}

impl RootCapabilitySpace {
    /// The caller validates the complete environment before constructing managers.
    pub fn from_environment(environment: &AlphaEnvironment) -> Self {
        Self {
            root_descriptor: environment.capabilities.capability_space,
            root_radix: environment.capabilities.root_radix,
            bootstrap_generic: environment.resources[environment.bootstrap_resource].capability,
        }
    }
}
