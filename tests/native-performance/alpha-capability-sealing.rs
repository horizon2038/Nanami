//! Exercise the real sealing handler; model only Kernel calls and the root.
use super::*;
use a9n_abi::CapabilityType;
use std::cell::RefCell;

const SEALED_CAPABILITY_ROOT_SLOT: usize = 68;
#[derive(Clone, Copy)]
struct Root {
    bootstrap_generic: usize,
    root_descriptor: usize,
    root_radix: usize,
}
#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
struct Alpha {
    root: Root,
    processes: ProcessManager,
    sealed_capability_root: Option<usize>,
}
#[derive(Debug, PartialEq, Eq)]
enum Call {
    Suspend(usize),
    Convert,
    Configure(usize, usize),
}
#[derive(Default)]
struct Kernel {
    calls: Vec<Call>,
    fail_suspend: bool,
    fail_convert: bool,
    fail_configure: bool,
}
thread_local! {
    static KERNEL: RefCell<Kernel> = RefCell::new(Kernel::default());
}
fn make_root_slot_descriptor(radix: usize, slot: usize) -> usize {
    assert_eq!(radix, 12);
    (radix << 56) | (slot << 44)
}
mod arch {
    use super::*;
    pub mod generic {
        use super::*;
        pub fn convert(
            generic: usize,
            kind: CapabilityType,
            radix: usize,
            count: usize,
            root: usize,
            slot: usize,
        ) -> Result<(), CapabilityError> {
            assert_eq!(generic, 12);
            assert!(matches!(kind, CapabilityType::Node));
            assert_eq!((radix, count, root, slot), (1, 1, 11, 68));
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                kernel.calls.push(Call::Convert);
                if kernel.fail_convert {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
    pub mod process_control_block {
        use super::*;
        pub fn suspend(pcb: usize) -> Result<(), CapabilityError> {
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                kernel.calls.push(Call::Suspend(pcb));
                if kernel.fail_suspend {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
        pub fn configure(
            pcb: usize,
            config: capability_call::process_control_block::ConfigurationInfo,
            address_space: usize,
            root: usize,
            frame: usize,
            notification: usize,
            resolver: usize,
            pc: usize,
            sp: usize,
            tls: usize,
            priority: usize,
            affinity: usize,
        ) -> Result<(), CapabilityError> {
            assert_eq!(config.data, 2); // Only the root-node configuration flag.
            assert_eq!(
                [
                    address_space,
                    frame,
                    notification,
                    resolver,
                    pc,
                    sp,
                    tls,
                    priority,
                    affinity
                ],
                [0; 9]
            );
            KERNEL.with(|kernel| {
                let mut kernel = kernel.borrow_mut();
                kernel.calls.push(Call::Configure(pcb, root));
                if kernel.fail_configure {
                    return Err(CapabilityError::Fatal);
                }
                Ok(())
            })
        }
    }
}
#[path = "../../nanami/src/nanami_core/alpha/capability_sealing.rs"]
mod production;

fn setup() -> (Alpha, OsRequestEvent) {
    KERNEL.with(|kernel| *kernel.borrow_mut() = Kernel::default());
    let mut processes = manager();
    processes
        .install_process(1, 9, 201, 100, 101, 102, 103, 1, 2, 0, 4096, 8192)
        .unwrap();
    processes
        .install_process(2, 9, 202, 200, 201, 202, 203, 2, 3, 0, 4096, 8192)
        .unwrap();
    (
        Alpha {
            root: Root {
                bootstrap_generic: 12,
                root_descriptor: 11,
                root_radix: 12,
            },
            processes,
            sealed_capability_root: None,
        },
        OsRequestEvent {
            identifier: 9,
            arg0: 1,
            arg1: 0,
            arg2: 0,
            arg3: 0,
        },
    )
}

#[test]
fn sealing_keeps_private_management_objects_and_shares_only_an_empty_execution_root() {
    let (mut a, mut request) = setup();
    let old = a.processes.find_entry_by_pid(1).unwrap();
    a.handle_process_seal_capabilities(request).unwrap();
    let task = a.processes.find_entry_by_pid(1).unwrap();
    assert!(task.capabilities_sealed);
    assert_eq!(
        (task.root_node, task.address_space, task.pcb, task.os_port),
        (old.root_node, old.address_space, old.pcb, old.os_port)
    );
    request.arg0 = 2;
    a.handle_process_seal_capabilities(request).unwrap();
    let root = make_root_slot_descriptor(12, 68);
    KERNEL.with(|kernel| {
        assert_eq!(
            kernel.borrow().calls,
            [
                Call::Suspend(101),
                Call::Convert,
                Call::Configure(101, root),
                Call::Suspend(201),
                Call::Configure(201, root),
            ]
        )
    });
    // An authorized repeat does not stop a task that has since been resumed.
    a.handle_process_seal_capabilities(request).unwrap();
    KERNEL.with(|kernel| assert_eq!(kernel.borrow().calls.len(), 5));
    assert!(!a.processes.alpha_entry().capabilities_sealed);
}

#[test]
fn unauthorized_self_unknown_exited_and_reserved_requests_make_no_kernel_calls() {
    let (mut a, request) = setup();
    for (caller, target) in [(8, 1), (1, 1), (9, 0), (9, 99)] {
        let mut invalid = request;
        invalid.identifier = caller;
        invalid.arg0 = target;
        assert_eq!(
            a.handle_process_seal_capabilities(invalid),
            Err(CapabilityError::PermissionDenied)
        );
    }
    for argument in 1..=3 {
        let mut invalid = request;
        match argument {
            1 => invalid.arg1 = 1,
            2 => invalid.arg2 = 1,
            _ => invalid.arg3 = 1,
        }
        assert_eq!(
            a.handle_process_seal_capabilities(invalid),
            Err(CapabilityError::InvalidArgument)
        );
    }
    a.processes.mark_exited(1, 1, 0).unwrap();
    assert_eq!(
        a.handle_process_seal_capabilities(request),
        Err(CapabilityError::PermissionDenied)
    );
    KERNEL.with(|kernel| assert!(kernel.borrow().calls.is_empty()));
}

#[test]
fn suspension_failure_never_creates_or_installs_a_new_root() {
    let (mut a, request) = setup();
    KERNEL.with(|kernel| kernel.borrow_mut().fail_suspend = true);
    assert_eq!(
        a.handle_process_seal_capabilities(request),
        Err(CapabilityError::Fatal)
    );
    assert!(
        !a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
    assert_eq!(a.sealed_capability_root, None);
    KERNEL.with(|kernel| assert_eq!(kernel.borrow().calls, [Call::Suspend(101)]));
}

#[test]
fn failed_creation_is_retryable_and_failed_configuration_never_publishes_the_seal() {
    let (mut a, request) = setup();
    KERNEL.with(|kernel| kernel.borrow_mut().fail_convert = true);
    assert_eq!(
        a.handle_process_seal_capabilities(request),
        Err(CapabilityError::Fatal)
    );
    assert_eq!(a.sealed_capability_root, None);
    assert!(
        !a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
    KERNEL.with(|kernel| {
        let mut kernel = kernel.borrow_mut();
        kernel.fail_convert = false;
        kernel.fail_configure = true;
    });
    assert_eq!(
        a.handle_process_seal_capabilities(request),
        Err(CapabilityError::Fatal)
    );
    assert!(a.sealed_capability_root.is_some());
    assert!(
        !a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
    KERNEL.with(|kernel| kernel.borrow_mut().fail_configure = false);
    a.handle_process_seal_capabilities(request).unwrap();
    assert!(
        a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
    KERNEL.with(|kernel| {
        assert_eq!(
            kernel
                .borrow()
                .calls
                .iter()
                .filter(|call| **call == Call::Convert)
                .count(),
            2
        )
    });
}

#[test]
fn fresh_or_reused_native_processes_do_not_inherit_a_previous_seal() {
    let (mut a, _) = setup();
    assert!(
        !a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
    assert_eq!(
        a.processes.seal_capabilities(0),
        Err(CapabilityError::PermissionDenied)
    );
    assert_eq!(
        a.processes.seal_capabilities(99),
        Err(CapabilityError::InvalidArgument)
    );
    a.processes.seal_capabilities(1).unwrap();
    a.processes
        .install_process(1, 9, 201, 100, 101, 102, 103, 1, 2, 0, 4096, 8192)
        .unwrap();
    assert!(
        !a.processes
            .find_entry_by_pid(1)
            .unwrap()
            .capabilities_sealed
    );
}
