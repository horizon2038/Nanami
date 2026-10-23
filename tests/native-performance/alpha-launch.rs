//! Production launcher with checked capability spaces, Generic cursors and page
//! contents. No host addresses are dereferenced by the injected PageWriter.
#![allow(dead_code)]
extern crate self as nun;
pub use a9n_abi::{
    capability_call, CapabilityDescriptor, CapabilityError, CapabilityType, Word, BYTE_BITS,
    TLS_BASE_OFFSET, WORD_BITS,
};
use nanami_uapi::alpha::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};
#[path = "../../nanami/src/nanami_utils/descriptor.rs"]
pub mod descriptor;
pub mod nanami_utils {
    pub use crate::descriptor;
}
#[path = "../../nanami/src/nanami_core/elf_loader.rs"]
pub mod elf_loader;
#[path = "../../nanami/src/nanami_core/kernel_object.rs"]
pub mod kernel_object;
pub mod nanami_core {
    pub use crate::{elf_loader, kernel_object};
}
#[path = "../../nanami/src/nanami_core/alpha_launch.rs"]
mod launch;
use descriptor::{make_child_slot_descriptor as child, make_root_slot_descriptor as slot};

#[derive(Clone)]
enum Object {
    Node(usize, BTreeMap<usize, usize>),
    Generic {
        address: usize,
        cursor: usize,
        end: usize,
        device: bool,
    },
    Frame(usize),
    Table(usize),
    Other(CapabilityType),
}
#[derive(Default)]
struct Kernel {
    objects: Vec<Object>,
    maps: BTreeMap<(usize, usize), usize>,
    tables: BTreeSet<(usize, usize, usize)>,
    ram: BTreeMap<usize, Box<[u8; 4096]>>,
    configured: Vec<(usize, usize, usize, usize, usize)>,
    originals: Vec<usize>,
    operations: usize,
    fail_at: Option<usize>,
    allocations: usize,
}
impl Kernel {
    fn operation(&mut self) -> Result<(), CapabilityError> {
        let index = self.operations;
        self.operations += 1;
        if self.fail_at == Some(index) {
            return Err(CapabilityError::Fatal);
        }
        Ok(())
    }
    fn add(&mut self, object: Object) -> usize {
        self.objects.push(object);
        self.objects.len() - 1
    }
    fn lookup(&self, descriptor: usize) -> (usize, usize) {
        self.lookup_from(0, descriptor)
    }
    fn lookup_from(&self, root: usize, descriptor: usize) -> (usize, usize) {
        let depth = descriptor_depth(descriptor);
        let mut consumed = 8;
        let mut node = root;
        loop {
            let Object::Node(radix, slots) = &self.objects[node] else {
                panic!("not a node");
            };
            consumed += radix;
            assert!(consumed <= depth);
            let index = descriptor >> (64 - consumed) & ((1 << radix) - 1);
            if consumed == depth {
                return (node, index);
            }
            node = *slots.get(&index).expect("missing node slot");
        }
    }
    fn resolve(&self, descriptor: usize) -> usize {
        self.resolve_from(0, descriptor)
    }
    fn resolve_from(&self, root: usize, descriptor: usize) -> usize {
        let (node, index) = self.lookup_from(root, descriptor);
        let Object::Node(_, slots) = &self.objects[node] else {
            unreachable!()
        };
        *slots.get(&index).expect("missing capability")
    }
    fn install(&mut self, node: usize, index: usize, object: usize) {
        let Object::Node(radix, slots) = &mut self.objects[node] else {
            panic!("not a node");
        };
        assert!(index < 1 << *radix);
        assert!(
            slots.insert(index, object).is_none(),
            "occupied capability slot"
        );
    }
    fn bytes(&self, address_space: usize, address: usize, size: usize) -> Vec<u8> {
        (0..size)
            .map(|i| {
                let va = address + i;
                let frame = self.maps[&(address_space, va & !4095)];
                let Object::Frame(physical) = self.objects[frame] else {
                    panic!("not a frame");
                };
                self.ram[&physical][va & 4095]
            })
            .collect()
    }
    fn write(&mut self, address: usize, data: &[u8]) {
        let frame = self.maps[&(2, address & !4095)];
        let Object::Frame(physical) = self.objects[frame] else {
            panic!("not a frame");
        };
        let offset = address & 4095;
        assert!(offset + data.len() <= 4096);
        self.ram.get_mut(&physical).unwrap()[offset..offset + data.len()].copy_from_slice(data);
    }
}
fn descriptor_depth(descriptor: usize) -> usize {
    (descriptor >> 56) + 8
}
thread_local! { static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default()); }

pub mod arch {
    pub mod generic {
        use super::super::*;
        pub fn convert(
            source: Word,
            kind: CapabilityType,
            bits: Word,
            count: Word,
            node: Word,
            index: Word,
        ) -> Result<(), CapabilityError> {
            assert_eq!(count, 1);
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let source = k.resolve(source);
                let destination = k.resolve(node);
                let Object::Generic {
                    cursor,
                    end,
                    device,
                    ..
                } = k.objects[source].clone()
                else {
                    panic!("not Generic");
                };
                assert!(!device, "device authority used for launch allocation");
                let radix = match kind {
                    CapabilityType::Node => kernel_object::node_memory_size_bits(bits),
                    CapabilityType::ProcessControlBlock => 12,
                    CapabilityType::AddressSpace
                    | CapabilityType::PageTable
                    | CapabilityType::Frame => 12,
                    CapabilityType::Generic => bits,
                    _ => panic!("unexpected object"),
                };
                let bytes = 1 << radix;
                let address = (cursor + bytes - 1) & !(bytes - 1);
                assert!(
                    address + bytes <= end,
                    "allocation exceeded actual Generic authority"
                );
                let object = match kind {
                    CapabilityType::Node => Object::Node(bits, BTreeMap::new()),
                    CapabilityType::Generic => Object::Generic {
                        address,
                        cursor: address,
                        end: address + bytes,
                        device,
                    },
                    CapabilityType::PageTable => Object::Table(bits),
                    CapabilityType::Frame => {
                        assert!(k.ram.insert(address, Box::new([0xa5; 4096])).is_none());
                        Object::Frame(address)
                    }
                    _ => Object::Other(kind),
                };
                let id = k.add(object);
                k.install(destination, index, id);
                if let Object::Generic { cursor, .. } = &mut k.objects[source] {
                    *cursor = address + bytes;
                }
                k.allocations += 1;
                Ok(())
            })
        }
    }
    pub mod node {
        use super::super::*;
        pub fn mint(
            destination: Word,
            index: Word,
            source: Word,
            rights: Word,
        ) -> Result<(), CapabilityError> {
            assert_eq!(rights, 0b110);
            copy(destination, index, source)
        }
        pub fn copy(destination: Word, index: Word, source: Word) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let destination = k.resolve(destination);
                let object = k.resolve(source);
                assert!(
                    !matches!(k.objects[object], Object::Generic { .. }),
                    "Generic COPY is forbidden"
                );
                k.install(destination, index, object);
                Ok(())
            })
        }
        pub fn movec(destination: Word, index: Word, source: Word) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let destination = k.resolve(destination);
                let (origin, origin_index) = k.lookup(source);
                let Object::Node(_, slots) = &mut k.objects[origin] else {
                    unreachable!()
                };
                let object = slots.remove(&origin_index).expect("MOVE source is empty");
                k.install(destination, index, object);
                Ok(())
            })
        }
    }
    pub mod address_space {
        use super::super::*;
        fn table_key(address_space: usize, address: usize, depth: usize) -> (usize, usize, usize) {
            (address_space, address >> (12 + 9 * depth), depth)
        }
        pub fn get_unset_depth(
            address_space: Word,
            address: Word,
            _: Word,
        ) -> Result<usize, CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let space = k.resolve(address_space);
                Ok((1..=3)
                    .rev()
                    .find(|&depth| !k.tables.contains(&table_key(space, address, depth)))
                    .unwrap_or(0))
            })
        }
        pub fn map(
            address_space: Word,
            frame: Word,
            address: Word,
            _: capability_call::address_space::Attribute,
        ) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let space = k.resolve(address_space);
                let object = k.resolve(frame);
                if let Object::Table(depth) = k.objects[object] {
                    assert!(k.tables.insert(table_key(space, address, depth)));
                } else {
                    assert!(matches!(k.objects[object], Object::Frame(_)));
                    assert!(
                        k.maps.insert((space, address), object).is_none(),
                        "mapping overwritten"
                    );
                }
                Ok(())
            })
        }
        pub fn unmap(
            address_space: Word,
            frame: Word,
            address: Word,
        ) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                let space = k.resolve(address_space);
                let object = k.resolve(frame);
                assert_eq!(k.maps.remove(&(space, address)), Some(object));
                Ok(())
            })
        }
    }
    pub mod process_control_block {
        use super::super::*;
        pub fn configure(
            pcb: Word,
            _: capability_call::process_control_block::ConfigurationInfo,
            address_space: Word,
            root: Word,
            ipc: Word,
            _: Word,
            _: Word,
            entry: Word,
            stack: Word,
            _: Word,
            priority: Word,
            core: Word,
        ) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut k = kernel.borrow_mut();
                k.operation()?;
                assert_eq!((priority, core), (31, 0));
                let pcb = k.resolve(pcb);
                let space = k.resolve(address_space);
                let root = k.resolve(root);
                assert_ne!(space, 2, "source address space reused");
                assert_ne!(root, 0, "source capability space reused");
                let _ = k.resolve(ipc);
                k.configured.push((pcb, space, root, entry, stack));
                Ok(())
            })
        }
    }
}
struct Writer;
impl launch::PageWriter for Writer {
    fn zero(&mut self, address: usize) {
        KERNEL.with(|kernel| kernel.borrow_mut().write(address, &[0; 4096]));
    }
    fn copy(&mut self, address: usize, bytes: &[u8]) {
        KERNEL.with(|kernel| kernel.borrow_mut().write(address, bytes));
    }
}

fn source() -> AlphaEnvironment {
    let mut source = AlphaEnvironment::default();
    source.capabilities = SelfCapabilities {
        capability_space: slot(12, 3),
        root_radix: 12,
        process_control_block: slot(12, 1),
        address_space: slot(12, 2),
        frames: slot(12, 5),
        page_tables: slot(12, 4),
        ipc_buffer: slot(12, 6),
        ..SelfCapabilities::default()
    };
    source.platform.core_count = 4;
    source.resource_count = 3;
    KERNEL.with(|kernel| {
        let mut k = Kernel::default();
        k.add(Object::Node(12, BTreeMap::new())); // root id 0
        let pcb = k.add(Object::Other(CapabilityType::ProcessControlBlock));
        let space = k.add(Object::Other(CapabilityType::AddressSpace)); // source AS id 2
        k.install(0, 1, pcb);
        k.install(0, 2, space);
        k.install(0, 3, 0);
        for index in [4, 5, 6] {
            let object = k.add(Object::Other(CapabilityType::Frame));
            k.install(0, index, object);
        }
        let resources = k.add(Object::Node(7, BTreeMap::new()));
        k.install(0, 7, resources);
        for (i, (address, bits, device)) in [
            (0x40000000, 30, false),
            (0x80000000, 26, false),
            (0xfed00000, 12, true),
        ]
        .into_iter()
        .enumerate()
        {
            let object = k.add(Object::Generic {
                address,
                cursor: address,
                end: address + (1 << bits),
                device,
            });
            k.install(resources, i, object);
            k.originals.push(object);
            source.resources[i] = MemoryResource {
                capability: child(slot(12, 7), 7, i),
                address,
                size_radix: bits,
                flags: if device { RESOURCE_DEVICE } else { 0 },
                consumed_bytes: 0,
            };
        }
        *kernel.borrow_mut() = k;
    });
    source
}
fn slots(base: usize) -> launch::LaunchSlots {
    launch::LaunchSlots {
        root: base,
        root_unit: base + 1,
        backing_unit: base + 2,
        control: base + 3,
    }
}
fn word(bytes: &mut [u8], at: usize, value: usize, size: usize) {
    bytes[at..at + size].copy_from_slice(&value.to_le_bytes()[..size]);
}
fn elf() -> Vec<u8> {
    let mut bytes = vec![0; 0x1100];
    bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
    word(&mut bytes, 16, 2, 2);
    word(
        &mut bytes,
        18,
        if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        },
        2,
    );
    for (at, value, size) in [
        (24, 0x1000, 8),
        (32, 64, 8),
        (40, 0x200, 8),
        (54, 56, 2),
        (56, 1, 2),
        (58, 64, 2),
        (60, 3, 2),
        (64, 1, 4),
        (72, 0x1000, 8),
        (80, 0x1000, 8),
        (96, 64, 8),
        (104, 0x20000, 8),
    ] {
        word(&mut bytes, at, value, size);
    }
    bytes[0x1000..0x1040].fill(0x90);
    for (at, value, size) in [
        (0x244, 2, 4),
        (0x258, 0x400, 8),
        (0x260, 96, 8),
        (0x268, 2, 4),
        (0x278, 24, 8),
        (0x284, 3, 4),
        (0x298, 0x600, 8),
        (0x2a0, 256, 8),
    ] {
        word(&mut bytes, at, value, size);
    }
    let mut string = 1;
    for (index, (name, address)) in [
        ("__alpha_environment", 0x4000),
        ("__alpha_environment_end", 0x14000),
        ("__ipc_buffer_start", 0x15000),
        ("__alpha_stack_end", 0x21000),
    ]
    .into_iter()
    .enumerate()
    {
        bytes[0x600 + string..0x600 + string + name.len()].copy_from_slice(name.as_bytes());
        word(&mut bytes, 0x400 + index * 24, string, 4);
        word(&mut bytes, 0x408 + index * 24, address, 8);
        string += name.len() + 1;
    }
    bytes
}
fn prepare(
    source: &mut AlphaEnvironment,
    image: &[u8],
    retire: bool,
) -> Result<launch::PreparedAlpha, CapabilityError> {
    launch::prepare(
        source,
        slots(600),
        launch::Inputs {
            image,
            archive: b"arbitrary module bytes",
            configuration: b"a-user-chosen-role 20 /bin/example\n",
            retire_launcher: retire,
        },
        &mut Writer,
    )
}

#[test]
fn independent_spaces_receive_copied_inputs_and_final_reservations() {
    let mut source = source();
    let image = elf();
    let prepared = prepare(&mut source, &image, true).unwrap();
    assert_eq!(prepared.environment.validate(), Ok(()));
    assert_ne!(prepared.environment.retired_launcher.process, 0);
    KERNEL.with(|kernel| {
        let k = kernel.borrow();
        let (_, space, root, entry, _) = k.configured[0];
        assert_eq!(entry, 0x1000);
        assert_eq!(k.bytes(space, 0x1000, 64), vec![0x90; 64]);
        assert_eq!(k.bytes(space, 0x3000, 4096), vec![0; 4096]);
        let input = prepared.environment.boot_archive;
        assert_eq!(
            k.bytes(space, input.address, input.bytes),
            b"arbitrary module bytes"
        );
        let input = prepared.environment.boot_configuration;
        assert_eq!(
            k.bytes(space, input.address, input.bytes),
            b"a-user-chosen-role 20 /bin/example\n"
        );
        let actual = k.bytes(space, 0x4000, core::mem::size_of::<AlphaEnvironment>());
        let header =
            unsafe { core::ptr::read_unaligned(actual.as_ptr().cast::<AlphaEnvironment>()) };
        assert_eq!(header.validate(), Ok(()));
        assert_eq!(
            header.resources[1].consumed_bytes,
            source.resources[1].consumed_bytes
        );
        assert_eq!(header.retired_launcher.process, slot(12, 16));
        assert_eq!(
            k.resolve_from(root, header.capabilities.capability_space),
            root
        );
        assert_eq!(
            k.resolve_from(root, header.capabilities.address_space),
            space
        );
        assert_eq!(
            k.resolve_from(root, header.retired_launcher.capability_space),
            0
        );
        for (i, &object) in k.originals.iter().enumerate() {
            let Object::Generic {
                address, cursor, ..
            } = k.objects[object]
            else {
                unreachable!()
            };
            assert_eq!(cursor - address, source.resources[i].consumed_bytes);
            assert_eq!(k.resolve_from(root, header.resources[i].capability), object);
        }
        assert!(
            k.maps.keys().all(|&(as_id, _)| as_id != 2),
            "temporary mappings retained"
        );
        assert_eq!(k.resolve(prepared.control), k.configured[0].0);
    });
}

#[test]
fn parent_is_not_retired_for_an_ordinary_child_launch() {
    let mut source = source();
    let prepared = prepare(&mut source, &elf(), false).unwrap();
    assert_eq!(
        prepared.environment.retired_launcher,
        RetiredLauncher::default()
    );
    assert_eq!(prepared.environment.parent_control, 0);
}

#[test]
fn imports_are_minted_into_receiver_local_slots_and_copied_into_owned_boot_data() {
    let mut source = source();
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        let port = kernel.add(Object::Other(CapabilityType::IpcPort));
        kernel.install(0, 70, port);
    });
    source.import_count = 1;
    let import = &mut source.imported_services[0];
    import.endpoint = slot(12, 70);
    import.upstream_service = 12;
    import.upstream_session = nanami_uapi::service::SESSION_IDENTIFIER_BIT | 7;
    import.interface = 99;
    import.session_limit = 3;
    import.name_len = 4;
    import.name[..4].copy_from_slice(b"echo");
    let prepared = prepare(&mut source, &elf(), false).unwrap();
    let received = prepared.environment.imports().unwrap()[0];
    assert_eq!(received.endpoint, child(slot(12, 10), 4, 0));
    assert_ne!(received.endpoint, source.imported_services[0].endpoint);
    assert_eq!(received.upstream_service, 12);
    assert_eq!(
        received.upstream_session,
        nanami_uapi::service::SESSION_IDENTIFIER_BIT | 7
    );
    KERNEL.with(|kernel| {
        let kernel = kernel.borrow();
        let root = kernel.resolve(slot(12, 600));
        assert_eq!(
            kernel.resolve_from(root, received.endpoint),
            kernel.resolve(slot(12, 70))
        );
    });
}

#[test]
fn one_provided_ram_capability_can_back_the_launch_without_other_parent_resources() {
    let mut source = source();
    source.resource_count = 1;
    source.resources[1..].fill(MemoryResource::EMPTY);
    let prepared = prepare(&mut source, &elf(), false).unwrap();
    assert_eq!(prepared.environment.resource_count, 1);
    KERNEL.with(|kernel| {
        let k = kernel.borrow();
        for &object in &k.originals[1..] {
            let Object::Generic {
                address, cursor, ..
            } = k.objects[object]
            else {
                unreachable!()
            };
            assert_eq!(address, cursor, "unprovided parent resource consumed");
        }
    });
}

#[test]
fn malformed_images_and_overlapping_slots_allocate_nothing() {
    for index in 0..3 {
        let mut source = source();
        let mut image = elf();
        match index {
            0 => image[0] = 0,
            1 => word(&mut image, 24, 0x90000000, 8),
            _ => word(&mut image, 0x408, 0x1001, 8),
        }
        assert!(prepare(&mut source, &image, true).is_err());
        KERNEL.with(|kernel| assert_eq!(kernel.borrow().allocations, 0));
    }
    let mut source = source();
    let invalid = launch::LaunchSlots {
        root: 600,
        root_unit: 600,
        backing_unit: 601,
        control: 602,
    };
    assert!(launch::prepare(
        &mut source,
        invalid,
        launch::Inputs {
            image: &elf(),
            archive: &[],
            configuration: &[],
            retire_launcher: false
        },
        &mut Writer
    )
    .is_err());
    KERNEL.with(|kernel| assert_eq!(kernel.borrow().allocations, 0));
}

#[test]
fn overflowing_elf_tables_and_ranges_fail_before_any_capability_allocation() {
    for offset in [32, 40, 72, 80, 96, 104, 0x258, 0x260, 0x298, 0x2a0] {
        let mut source = source();
        let mut image = elf();
        word(&mut image, offset, usize::MAX, 8);
        assert!(
            prepare(&mut source, &image, true).is_err(),
            "offset {offset:x}"
        );
        KERNEL.with(|kernel| assert_eq!(kernel.borrow().allocations, 0));
    }
}

#[test]
fn injected_failures_do_not_roll_back_successful_reservations_or_start_a_child() {
    let mut baseline = source();
    prepare(&mut baseline, &elf(), true).unwrap();
    let operations = KERNEL.with(|kernel| kernel.borrow().operations);
    for fail in 0..operations {
        let mut source = source();
        KERNEL.with(|kernel| kernel.borrow_mut().fail_at = Some(fail));
        assert!(prepare(&mut source, &elf(), true).is_err());
        KERNEL.with(|kernel| {
            let k = kernel.borrow();
            assert!(k.configured.len() <= 1);
            for (i, &object) in k.originals.iter().enumerate() {
                let Object::Generic {
                    address, cursor, ..
                } = k.objects[object]
                else {
                    unreachable!()
                };
                assert_eq!(cursor - address, source.resources[i].consumed_bytes);
            }
        });
    }
}
