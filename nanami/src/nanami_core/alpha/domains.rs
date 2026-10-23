pub(super) use super::domain_manifest::ChildSpec;
use super::*;
use crate::nanami_core::{alpha_launch, memory::DelegationId};
use nanami_uapi::{
    alpha::ImportedService,
    service::{ServiceId, SESSION_IDENTIFIER_BIT},
};

/// Retained parent-side ownership. Failed construction is never silently freed.
pub(super) struct ChildDomain {
    pub memory: DelegationId,
    pub slots: [usize; 4],
    pub control: Option<CapabilityDescriptor>,
    pub running: bool,
}

fn module(archive: &'static [u8], name: &str) -> Result<&'static [u8], CapabilityError> {
    let mut found = None;
    cpio::for_each_newc_entry(archive, |entry| {
        if entry.name == name {
            if found.is_some() {
                return Err(CapabilityError::InvalidArgument);
            }
            found = Some(entry.data);
        }
        Ok(())
    })?;
    found.ok_or(CapabilityError::InvalidArgument)
}

impl Alpha {
    pub(super) fn child_ready(&self, spec: ChildSpec<'_>) -> Result<bool, CapabilityError> {
        let mut ready = spec
            .after
            .is_none_or(|name| self.communication.resolve_service(name).is_some());
        for import in self
            .import_policies
            .iter()
            .filter(|import| import.child == spec.label)
        {
            if let Some(id) = self.communication.services.managed_id(import.source) {
                if self.communication.bindings.service(ServiceId(id))?.profile
                    != nanami_uapi::service::STATELESS_CONTROL_PROFILE
                {
                    return Err(CapabilityError::IllegalOperation);
                }
            }
            if self
                .communication
                .services
                .managed_id(import.source)
                .is_none()
            {
                if self.communication.resolve_service(import.source).is_some() {
                    // This name cannot later become managed: replacement is
                    // forbidden. Report the incompatible profile, not a wait.
                    return Err(CapabilityError::IllegalOperation);
                }
                ready = false;
            }
        }
        Ok(ready)
    }

    pub(super) fn start_ready_children(&mut self) {
        let mut index = 0;
        while index < self.pending_children.len() {
            let ready = match self.child_ready(self.pending_children[index]) {
                Ok(ready) => ready,
                Err(error) => {
                    let spec = self.pending_children.remove(index);
                    error!(
                        "[domain] launch failed label={} import={:?}",
                        spec.label, error
                    );
                    continue;
                }
            };
            if ready {
                let spec = self.pending_children.remove(index);
                if let Err(error) = self.spawn_child_alpha(spec) {
                    error!(
                        "[domain] launch failed label={} err={:?}",
                        spec.label, error
                    );
                }
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn spawn_child_alpha(&mut self, spec: ChildSpec<'_>) -> Result<(), CapabilityError> {
        let image = module(self.initramfs_image, spec.image)?;
        let archive = if spec.archive == "-" {
            self.initramfs_image
        } else {
            module(self.initramfs_image, spec.archive)?
        };
        let configuration = module(self.initramfs_image, spec.configuration)?;
        // Temporary page-table backing belongs to the parent, never the child
        // whose memory can later be revoked. Successful launch leaves no alias.
        self.memory.ensure_page_tables(
            self.launch_capabilities.address_space,
            alpha_launch::TEMP,
            self.processes.alpha_vm_space_mut(),
        )?;
        let mut slots = [0; 4];
        for index in 0..4 {
            match self.processes.alloc_process_slot() {
                Ok((_, slot)) => slots[index] = slot,
                Err(error) => {
                    for &slot in &slots[..index] {
                        self.processes.release_process_slot_reservation(slot);
                    }
                    return Err(error);
                }
            }
        }
        let memory = match self.memory.reserve_alpha_memory(spec.radix) {
            Ok(memory) => memory,
            Err(error) => {
                for &slot in &slots {
                    self.processes.release_process_slot_reservation(slot);
                }
                return Err(error);
            }
        };
        let index = self.children.len();
        self.children.push(ChildDomain {
            memory,
            slots,
            control: None,
            running: false,
        });
        let resources = self.memory.take_delegated_resources(memory)?;
        let mut environment = AlphaEnvironment {
            domain_id: memory.value(),
            capabilities: self.launch_capabilities,
            platform: self.platform_information,
            resource_count: resources.len(),
            bootstrap_resource: 0,
            ..AlphaEnvironment::default()
        };
        // Parent authority is used only to construct the target. No interrupt,
        // I/O or framebuffer authority is inherited by this device-free child.
        environment.capabilities.interrupt_region = 0;
        environment.capabilities.io_ports = 0;
        environment.resources[..resources.len()].copy_from_slice(&resources);
        // A parent charges exactly one upstream Session per explicit import.
        // Child clients share that bounded authority, not the parent's registry
        // or a bare Provider PID. Failed launches retain the charge and backing.
        for policy in self
            .import_policies
            .iter()
            .filter(|policy| policy.child == spec.label)
        {
            let id = ServiceId(
                self.communication
                    .services
                    .managed_id(policy.source)
                    .ok_or(CapabilityError::InvalidArgument)?,
            );
            let imported = self.imported_services[id.0 - 1];
            let endpoint = if let Some(import) = imported {
                import.endpoint
            } else {
                self.service_cap(id.0 - 1)
            };
            let interface = if let Some(import) = imported {
                import.interface
            } else {
                self.communication
                    .bindings
                    .provider(self.communication.bindings.service(id)?.provider)?
                    .interface
            };
            let directory = self.service_directory();
            let session =
                self.communication
                    .bindings
                    .connect_domain(memory.value(), id, 0, |session| {
                        let index = 160 + (session.0 & !SESSION_IDENTIFIER_BIT) - 1;
                        if imported.is_some() {
                            return arch::node::mint(directory, index, endpoint, 0b110);
                        }
                        arch::node::copy(directory, index, endpoint)?;
                        let alias =
                            make_child_slot_descriptor(directory, SERVICE_DIRECTORY_RADIX, index);
                        let result = arch::ipc_port::identify(alias, session.0)
                            .and_then(|_| arch::node::demote(directory, index, 0b110));
                        if result.is_err() && arch::node::remove(directory, index).is_err() {
                            return Err(CapabilityError::Fatal);
                        }
                        result
                    })?;
            let mut import = ImportedService {
                endpoint: self.service_cap(160 + (session.0 & !SESSION_IDENTIFIER_BIT) - 1),
                upstream_service: id.0,
                upstream_session: session.0,
                interface,
                session_limit: policy.sessions,
                name_len: policy.name.len(),
                name: [0; 24],
            };
            if let Some(source) = imported {
                import.upstream_service = source.upstream_service;
                import.upstream_session = source.upstream_session;
            }
            import.name[..policy.name.len()].copy_from_slice(policy.name.as_bytes());
            environment.imported_services[environment.import_count] = import;
            environment.import_count += 1;
        }
        let prepared = alpha_launch::prepare(
            &mut environment,
            alpha_launch::LaunchSlots {
                root: slots[0],
                root_unit: slots[1],
                backing_unit: slots[2],
                control: slots[3],
            },
            alpha_launch::Inputs {
                image,
                archive,
                configuration,
                retire_launcher: false,
            },
            &mut alpha_launch::NativeWriter,
        )?;
        self.children[index].control = Some(prepared.control);
        arch::process_control_block::resume(prepared.control)?;
        self.children[index].running = true;
        crate::force_info!(
            "[domain] parent={} child={} label={} budget={} started",
            self.domain_id,
            memory.value(),
            spec.label,
            1usize << spec.radix
        );
        Ok(())
    }
}
