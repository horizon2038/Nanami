use nanami_uapi::alpha::*;

fn environment() -> AlphaEnvironment {
    let mut environment = AlphaEnvironment {
        resource_count: 2,
        capabilities: SelfCapabilities {
            capability_space: 0xc00300000000000,
            root_radix: 12,
            address_space: 0xc00200000000000,
            process_control_block: 0xc00100000000000,
            ..SelfCapabilities::default()
        },
        ..AlphaEnvironment::default()
    };
    environment.platform.core_count = 4;
    environment.resources[0] = MemoryResource {
        capability: 0x1307760000000000,
        address: 0x40000000,
        size_radix: 30,
        flags: 0,
        consumed_bytes: 0x80000,
    };
    environment.resources[1] = MemoryResource {
        capability: 0x1307750000000000,
        address: 0xfed00000,
        size_radix: 12,
        flags: RESOURCE_DEVICE,
        consumed_bytes: 0,
    };
    environment
}

#[test]
fn owned_contract_needs_no_parent_and_no_device_authority() {
    let environment = environment();
    assert_eq!(environment.validate(), Ok(()));
    assert_eq!(environment.parent_control, 0);
    assert_eq!(environment.capabilities.io_ports, 0);
    assert_eq!(environment.capabilities.interrupt_region, 0);
    assert_eq!(environment.memory_resources().unwrap().len(), 2);
    assert!(environment.resources[1].is_device());
}

#[test]
fn mapped_inputs_reject_null_overflow_and_noncanonical_ranges() {
    for (address, bytes) in [(1, 0), (0, 1), (usize::MAX, 4096), ((1usize << 47) - 1, 2)] {
        let mut input = environment();
        input.boot_archive = MappedInput { address, bytes };
        assert_eq!(input.validate(), Err(EnvironmentError::Input));
    }
    let mut input = environment();
    input.boot_archive = MappedInput {
        address: 0x20000000,
        bytes: 4097,
    };
    input.boot_configuration = MappedInput {
        address: 0x24000000,
        bytes: 100,
    };
    assert_eq!(input.validate(), Ok(()));
}

#[test]
fn incompatible_and_truncated_headers_are_rejected() {
    let mut input = environment();
    input.abi_version += 1;
    assert_eq!(input.validate(), Err(EnvironmentError::Abi));
    input = environment();
    input.size -= 1;
    assert_eq!(input.validate(), Err(EnvironmentError::Abi));
    for count in [129, usize::MAX] {
        input = environment();
        input.resource_count = count;
        assert_eq!(input.validate(), Err(EnvironmentError::Resource));
    }
}

#[test]
fn missing_self_capabilities_and_invalid_topology_fail_validation() {
    for index in 0..5 {
        let mut input = environment();
        match index {
            0 => input.capabilities.capability_space = 0,
            1 => input.capabilities.address_space = 0,
            2 => input.capabilities.process_control_block = 0,
            3 => input.capabilities.root_radix = 0,
            _ => input.capabilities.root_radix = usize::BITS as usize,
        }
        assert_eq!(input.validate(), Err(EnvironmentError::Capabilities));
    }
    for cores in [0, 65, usize::MAX] {
        let mut input = environment();
        input.platform.core_count = cores;
        assert_eq!(input.validate(), Err(EnvironmentError::Platform));
    }
}

#[test]
fn malformed_ranges_cannot_become_allocator_input() {
    for index in 0..6 {
        let mut input = environment();
        let resource = &mut input.resources[0];
        match index {
            0 => resource.capability = 0,
            1 => resource.size_radix = usize::BITS as usize,
            2 => resource.size_radix = usize::MAX,
            3 => resource.address = usize::MAX,
            4 => resource.consumed_bytes = (1 << 30) + 1,
            _ => resource.flags = 2,
        }
        assert_eq!(input.validate(), Err(EnvironmentError::Resource));
    }
    let mut input = environment();
    input.resources[1].capability = input.resources[0].capability;
    assert_eq!(input.validate(), Err(EnvironmentError::Resource));
}

#[test]
fn bootstrap_source_must_be_usable_ram() {
    let mut input = environment();
    input.bootstrap_resource = 2;
    assert_eq!(input.validate(), Err(EnvironmentError::BootstrapResource));
    input.bootstrap_resource = 1;
    assert_eq!(input.validate(), Err(EnvironmentError::BootstrapResource));
    input.bootstrap_resource = 0;
    input.resources[0].consumed_bytes = 1 << 30;
    assert_eq!(input.validate(), Err(EnvironmentError::BootstrapResource));
}

#[test]
fn contract_copy_retains_owned_metadata_not_origin_references() {
    let mut origin = environment();
    origin.platform.architecture[..7].copy_from_slice(b"example");
    let received = origin;
    origin.platform.architecture.fill(0);
    origin.resources.fill(MemoryResource::EMPTY);
    assert_eq!(name(&received.platform.architecture), "example");
    assert_eq!(received.resources[0].consumed_bytes, 0x80000);
    assert_eq!(received.validate(), Ok(()));
}

fn imported() -> ImportedService {
    ImportedService {
        endpoint: 0x1234,
        upstream_service: 3,
        upstream_session: nanami_uapi::service::SESSION_IDENTIFIER_BIT | 7,
        interface: 99,
        session_limit: 2,
        name_len: 4,
        name: *b"echo\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    }
}

#[test]
fn imports_are_owned_named_scoped_and_bounded() {
    let mut input = environment();
    input.import_count = 1;
    input.imported_services[0] = imported();
    assert_eq!(input.validate(), Ok(()));
    assert_eq!(input.imports().unwrap()[0].name().unwrap(), "echo");
    let copy = input;
    input.imported_services[0].name.fill(0);
    assert_eq!(copy.imports().unwrap()[0].name().unwrap(), "echo");
    assert_eq!(input.validate(), Err(EnvironmentError::Import));
    for index in 0..9 {
        let mut input = copy;
        let import = &mut input.imported_services[0];
        match index {
            0 => import.endpoint = 0,
            1 => import.upstream_service = 0,
            2 => import.upstream_session = 7,
            3 => import.interface = 0,
            4 => import.session_limit = 0,
            5 => import.session_limit = 65,
            6 => import.name_len = 25,
            7 => import.name_len = 0,
            _ => import.name[0] = 0xff,
        }
        assert_eq!(input.validate(), Err(EnvironmentError::Import));
    }
    let mut input = copy;
    input.import_count = 2;
    input.imported_services[1] = imported();
    assert_eq!(input.validate(), Err(EnvironmentError::Import));
    input.import_count = ALPHA_IMPORT_CAPACITY + 1;
    assert_eq!(input.validate(), Err(EnvironmentError::Import));
}

#[test]
fn malformed_word_fields_never_panic_or_wrap_into_valid_resources() {
    let mut random = 17u64;
    for _ in 0..4000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let value = random as usize;
        let mut input = environment();
        match random % 7 {
            0 => input.resource_count = value,
            1 => input.bootstrap_resource = value,
            2 => input.capabilities.root_radix = value,
            3 => input.resources[0].size_radix = value,
            4 => input.resources[0].consumed_bytes = value,
            5 => input.resources[0].address = value,
            _ => input.size = value,
        }
        let _ = input.validate();
    }
}
