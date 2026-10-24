//! A kernel-semantics probe, launched with the common Alpha ABI in private RAM.
//! It does not implement an Alpha or count as shared-initializer coverage.
#![no_std]
#![no_main]

use a9n_abi::{arch, CapabilityDescriptor, CapabilityError, CapabilityType, MessageInfo};
use nanami_uapi::alpha::AlphaEnvironment;
use core::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicUsize = AtomicUsize::new(0);
static RESUME_SELF: AtomicUsize = AtomicUsize::new(0);
#[repr(align(16))]
struct WorkerStack([u8; 8192]);
static mut WORKER_STACK: WorkerStack = WorkerStack([0; 8192]);

extern "C" fn worker() -> ! {
    COUNTER.store(1, Ordering::Release);
    arch::notification_port::notify(READY.load(Ordering::Acquire)).unwrap();
    loop {
        let resume = RESUME_SELF.swap(0, Ordering::AcqRel);
        if resume != 0 && arch::process_control_block::resume(resume).is_err() {
            RESUME_SELF.store(resume, Ordering::Release);
        }
        COUNTER.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text", ".global _start", "_start:",
    "lea rsp, [__alpha_stack_end]", "lea rdi, [__alpha_environment]", "call {entry}",
    "ud2", entry = sym entry
);
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text", ".global _start", "_start:",
    "adrp x1, __alpha_stack_end", "add x1, x1, :lo12:__alpha_stack_end", "mov sp, x1",
    "adrp x0, __alpha_environment", "add x0, x0, :lo12:__alpha_environment", "bl {entry}",
    "brk #0", entry = sym entry
);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("CAPABILITY-PROBE-FAIL {}", info);
    loop {
        nanami_runtime::yield_now();
    }
}

fn slot(radix: usize, index: usize) -> CapabilityDescriptor {
    (radix << 56) | (index << (56 - radix))
}

fn nested(
    root_radix: usize,
    node_index: usize,
    radix: usize,
    index: usize,
) -> CapabilityDescriptor {
    ((root_radix + radix) << 56)
        | (node_index << (56 - root_radix))
        | (index << (56 - root_radix - radix))
}

fn permission_denied(result: Result<(), CapabilityError>) {
    assert!(
        matches!(result, Err(CapabilityError::PermissionDenied)),
        "unexpected result: {:?}",
        result
    );
}

extern "C" fn entry(environment: *const AlphaEnvironment) -> ! {
    let environment = unsafe { &*environment };
    environment.validate().unwrap();
    // The shared launcher already configured TLS to this probe's own IPC frame.
    assert!(!unsafe { arch::ipc_buffer::unsafe_get_ipc_buffer() }.is_null());
    let root = environment.capabilities.capability_space;
    let radix = environment.capabilities.root_radix;
    let source = environment.resources[environment.bootstrap_resource].capability;
    arch::generic::convert(source, CapabilityType::Node, 4, 1, root, 96).unwrap();
    arch::generic::convert(source, CapabilityType::Generic, 16, 1, root, 97).unwrap();
    let directory = slot(radix, 96);
    let unit = slot(radix, 97);
    let port = |index| nested(radix, 96, 4, index);
    arch::generic::convert(unit, CapabilityType::IpcPort, 0, 1, directory, 0).unwrap();
    arch::ipc_port::identify(port(0), 0x53).unwrap();
    arch::node::copy(directory, 1, port(0)).unwrap();
    arch::node::mint(directory, 2, port(0), 0b11).unwrap();
    arch::node::mint(directory, 3, port(0), 0b10).unwrap();
    permission_denied(arch::node::copy(directory, 4, port(2)));
    permission_denied(arch::ipc_port::identify(port(2), 99));
    let mut message = MessageInfo::normal(false, 0, 0);
    let mut identifier = 0;
    permission_denied(arch::ipc_port::receive(
        port(3),
        &mut message,
        &mut identifier,
    ));

    // COPY/MINT create same-depth aliases. Revoking one endpoint alias must not
    // be mistaken for revoking a private provider delegation subtree.
    arch::node::revoke(directory, 0).unwrap();
    arch::ipc_port::identify(port(1), 0x54).unwrap();
    arch::node::remove(directory, 2).unwrap();
    arch::ipc_port::identify(port(1), 0x55).unwrap();
    permission_denied(arch::node::revoke(directory, 3));
    arch::node::demote(directory, 3, 0).unwrap();
    arch::node::remove(directory, 3).unwrap();
    // All attenuated rights combinations can be deleted without granting READ
    // or MODIFY. Identifier updates remain independently protected by MODIFY.
    for rights in 0..16 {
        arch::node::mint(directory, 3, port(1), rights).unwrap();
        if rights & 8 == 0 {
            permission_denied(arch::ipc_port::identify(port(3), 99));
        }
        arch::node::remove(directory, 3).unwrap();
    }
    arch::node::mint(directory, 3, port(1), 0b10).unwrap();

    arch::node::movec(root, 98, port(3)).unwrap();
    arch::node::copy(directory, 3, port(1)).unwrap();
    permission_denied(arch::ipc_port::identify(slot(radix, 98), 99));
    arch::node::revoke(root, 96).unwrap();
    assert!(arch::node::copy(directory, 4, port(1)).is_err());
    arch::node::revoke(root, 97).unwrap();
    assert!(arch::ipc_port::identify(slot(radix, 98), 99).is_err());
    for _ in 0..32 {
        arch::generic::convert(unit, CapabilityType::IpcPort, 0, 1, directory, 0).unwrap();
        arch::node::mint(root, 98, port(0), 0).unwrap();
        arch::node::revoke(root, 97).unwrap();
        assert!(arch::node::copy(directory, 1, port(0)).is_err());
    }

    arch::generic::convert(source, CapabilityType::NotificationPort, 0, 1, root, 101).unwrap();
    arch::notification_port::identify(slot(radix, 101), 1).unwrap();
    READY.store(slot(radix, 101), Ordering::Release);
    let affinity = environment.platform.core_count - 1;
    for iteration in 0..16 {
        COUNTER.store(0, Ordering::Release);
        arch::generic::convert(unit, CapabilityType::ProcessControlBlock, 0, 1, root, 100).unwrap();
        let stack = core::ptr::addr_of!(WORKER_STACK) as usize + 8192;
        let configuration = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
            true, true, false, false, false, true, true, false, true, true,
        );
        arch::process_control_block::configure(
            slot(radix, 100), configuration, environment.capabilities.address_space,
            root, 0, 0, 0, worker as *const () as usize, stack, 0, 29, affinity,
        ).unwrap();
        arch::process_control_block::resume(slot(radix, 100)).unwrap();
        arch::notification_port::wait(slot(radix, 101)).unwrap();
        assert!(COUNTER.load(Ordering::Acquire) > 0);
        if environment.platform.core_count > 1 {
            for _ in 0..10_000_000 {
                if COUNTER.load(Ordering::Acquire) >= 1024 {
                    break;
                }
                core::hint::spin_loop();
            }
            assert!(COUNTER.load(Ordering::Acquire) >= 1024, "remote worker did not enter user loop");
        }
        arch::process_control_block::suspend(slot(radix, 100)).unwrap();
        let suspended = COUNTER.load(Ordering::Acquire);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        assert_eq!(COUNTER.load(Ordering::Acquire), suspended, "worker survived successful suspend");
        // A successful suspend permits immediate register access and resume,
        // without waiting for a timer or retrying INVALID_ARGUMENT.
        #[cfg(target_arch = "x86_64")]
        let register_count = 23;
        #[cfg(target_arch = "aarch64")]
        let register_count = 35;
        arch::process_control_block::read_register(slot(radix, 100), register_count).unwrap();
        arch::process_control_block::write_register(slot(radix, 100), register_count).unwrap();
        arch::process_control_block::resume(slot(radix, 100)).unwrap();
        if environment.platform.core_count > 1 {
            for _ in 0..10_000_000 {
                if COUNTER.load(Ordering::Acquire) > suspended {
                    break;
                }
                core::hint::spin_loop();
            }
            assert!(COUNTER.load(Ordering::Acquire) > suspended, "worker did not resume");
        }
        if iteration == 15 {
            RESUME_SELF.store(environment.capabilities.process_control_block, Ordering::Release);
            arch::process_control_block::suspend(environment.capabilities.process_control_block).unwrap();
            assert_eq!(RESUME_SELF.load(Ordering::Acquire), 0, "self suspend returned before resumer ran");
        }
        arch::node::revoke(root, 97).unwrap();
        let stopped = COUNTER.load(Ordering::Acquire);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        assert_eq!(COUNTER.load(Ordering::Acquire), stopped, "worker survived successful revoke");
        assert!(arch::process_control_block::resume(slot(radix, 100)).is_err());
    }
    nanami_runtime::println!("CAPABILITY-PROBE-PASS siblings=true weak-delete=true node-drain=true generic-reuse=32 process-reuse=16 cores={}", environment.platform.core_count);
    nanami_runtime::println!("SUSPEND-PROBE-PASS stopped=true register-roundtrip=true immediate-resume=true self-resume=true");
    nanami_runtime::println!("CAPABILITY-PROBE-DONE");
    // Keep all test backing reserved; no test claims failed revoke freed it.
    arch::process_control_block::suspend(environment.capabilities.process_control_block).unwrap();
    loop {
        nanami_runtime::yield_now();
    }
}
