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
static WAIT_PORT: AtomicUsize = AtomicUsize::new(0);
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

extern "C" {
    fn continuation_foreign_call();
    fn continuation_native_wait(port: usize) -> usize;
    static continuation_foreign_instruction: u8;
    static continuation_foreign_after: u8;
    static continuation_native_after: u8;
}

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text",
    ".global continuation_foreign_call", "continuation_foreign_call:",
    "mov eax, 0x7000", "mov edi, 0x1122", "mov esi, 0x3344", "xor edx, edx",
    ".global continuation_foreign_instruction", "continuation_foreign_instruction:", "syscall",
    ".global continuation_foreign_after", "continuation_foreign_after:", "ret",
    ".global continuation_native_wait", "continuation_native_wait:",
    "mov rax, -1", "mov esi, 2", "syscall",
    ".global continuation_native_after", "continuation_native_after:",
    "mov rax, rdi", "ret",
);
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text",
    ".global continuation_foreign_call", "continuation_foreign_call:",
    "mov x8, #0x7000", "mov x0, #0x1122", "mov x1, #0x3344", "mov x2, #0",
    ".global continuation_foreign_instruction", "continuation_foreign_instruction:", "svc #0",
    ".global continuation_foreign_after", "continuation_foreign_after:", "ret",
    ".global continuation_native_wait", "continuation_native_wait:",
    "mov x8, #-1", "mov x1, #2", "svc #0",
    ".global continuation_native_after", "continuation_native_after:", "ret",
);

extern "C" fn foreign_worker() -> ! {
    unsafe { continuation_foreign_call(); }
    COUNTER.store(2, Ordering::Release);
    arch::notification_port::notify(READY.load(Ordering::Acquire)).unwrap();
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn native_wait_worker() -> ! {
    COUNTER.store(1, Ordering::Release);
    arch::notification_port::notify(READY.load(Ordering::Acquire)).unwrap();
    let success = unsafe { continuation_native_wait(WAIT_PORT.load(Ordering::Acquire)) };
    let mut result = 2;
    if success != 0 {
        result = 3;
    }
    COUNTER.store(result, Ordering::Release);
    arch::notification_port::notify(READY.load(Ordering::Acquire)).unwrap();
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "x86_64")]
const PC_REGISTER: usize = 15;
#[cfg(target_arch = "aarch64")]
const PC_REGISTER: usize = 32;
#[cfg(target_arch = "x86_64")]
const ARGUMENT_REGISTER: usize = 4;
#[cfg(target_arch = "aarch64")]
const ARGUMENT_REGISTER: usize = 0;

fn read_context(pcb: usize) -> [usize; nanami_runtime::ipc::HARDWARE_CONTEXT_WORDS] {
    let mut context = [0; nanami_runtime::ipc::HARDWARE_CONTEXT_WORDS];
    arch::process_control_block::read_register(pcb, context.len()).unwrap();
    let ipc = arch::ipc_buffer::get_ipc_buffer();
    for (index, value) in context.iter_mut().enumerate() {
        *value = ipc.get_message(3 + index);
    }
    context
}

fn continuation_tests(environment: &AlphaEnvironment, source: usize, unit: usize) {
    let root = environment.capabilities.capability_space;
    let radix = environment.capabilities.root_radix;
    let pcb = slot(radix, 100);
    let endpoint = slot(radix, 102);
    // An Alpha's entry process starts at priority 31. Match the workers so
    // yield can let a same-core worker reach WAIT; yielding never donates
    // CPU time to lower priorities in the fixed-priority scheduler.
    let priority = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
        false, false, false, false, false, false, false, false, true, false,
    );
    arch::process_control_block::configure(
        environment.capabilities.process_control_block, priority,
        0, 0, 0, 0, 0, 0, 0, 0, 29, 0,
    ).unwrap();
    arch::generic::convert(source, CapabilityType::IpcPort, 0, 1, root, 102).unwrap();
    arch::ipc_port::identify(endpoint, 0x77).unwrap();
    arch::generic::convert(source, CapabilityType::NotificationPort, 0, 1, root, 103).unwrap();
    WAIT_PORT.store(slot(radix, 103), Ordering::Release);
    let stack = core::ptr::addr_of!(WORKER_STACK) as usize + 8192;
    for native in [false, true] {
        COUNTER.store(0, Ordering::Release);
        arch::generic::convert(unit, CapabilityType::ProcessControlBlock, 0, 1, root, 100).unwrap();
        let configuration = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
            true, true, false, false, !native, true, true, false, true, true,
        );
        let mut entry = foreign_worker as *const () as usize;
        if native {
            entry = native_wait_worker as *const () as usize;
        }
        arch::process_control_block::configure(
            pcb, configuration, environment.capabilities.address_space,
            root, 0, 0, endpoint, entry, stack, 0, 29, environment.platform.core_count - 1,
        ).unwrap();
        arch::process_control_block::resume(pcb).unwrap();
        if native {
            arch::notification_port::wait(READY.load(Ordering::Acquire)).unwrap();
            let mut cancelled = false;
            for _ in 0..1000 {
                nanami_runtime::yield_now();
                arch::process_control_block::suspend(pcb).unwrap();
                let context = read_context(pcb);
                if context[PC_REGISTER] == core::ptr::addr_of!(continuation_native_after) as usize
                    && context[ARGUMENT_REGISTER] == 0
                {
                    cancelled = true;
                    break;
                }
                arch::process_control_block::resume(pcb).unwrap();
            }
            assert!(cancelled, "native wait never reached cancellation point");
            assert_eq!(COUNTER.load(Ordering::Acquire), 1);
            arch::process_control_block::resume(pcb).unwrap();
        } else {
            let mut message = MessageInfo::normal(true, 0, 0);
            let mut identifier = 0;
            arch::ipc_port::receive(endpoint, &mut message, &mut identifier).unwrap();
            assert_eq!(identifier, 0x77);
            assert_eq!(arch::ipc_buffer::get_ipc_buffer().get_message(4), 5);
            arch::process_control_block::suspend(pcb).unwrap();
            let context = read_context(pcb);
            assert_eq!(context[PC_REGISTER], core::ptr::addr_of!(continuation_foreign_instruction) as usize);
            assert_eq!(context[ARGUMENT_REGISTER], 0x1122);
            assert_eq!(context[ARGUMENT_REGISTER + 1], 0x3344);
            arch::process_control_block::suspend(pcb).unwrap();
            assert_eq!(read_context(pcb)[PC_REGISTER], context[PC_REGISTER]);
            arch::process_control_block::resume(pcb).unwrap();
            // The same foreign syscall must fault a second time, not disappear.
            message = MessageInfo::normal(true, 0, 0);
            arch::ipc_port::receive(endpoint, &mut message, &mut identifier).unwrap();
            assert_eq!(identifier, 0x77);
            let context = read_context(pcb);
            assert_eq!(context[PC_REGISTER], core::ptr::addr_of!(continuation_foreign_after) as usize);
            let ipc = arch::ipc_buffer::get_ipc_buffer();
            for (index, value) in context.iter().enumerate() {
                ipc.configure_message(3 + index, *value);
            }
            ipc.configure_message(3, 0); // completed foreign syscall result (RAX/x0)
            arch::process_control_block::write_register(pcb, context.len()).unwrap();
            arch::process_control_block::resume(pcb).unwrap();
        }
        arch::notification_port::wait(READY.load(Ordering::Acquire)).unwrap();
        assert_eq!(COUNTER.load(Ordering::Acquire), 2, "continuation did not finish correctly");
        arch::node::revoke(root, 97).unwrap();
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
    continuation_tests(environment, source, unit);
    nanami_runtime::println!("CAPABILITY-PROBE-PASS siblings=true weak-delete=true node-drain=true generic-reuse=32 process-reuse=16 cores={}", environment.platform.core_count);
    nanami_runtime::println!("SUSPEND-PROBE-PASS stopped=true register-roundtrip=true immediate-resume=true self-resume=true fault-replay=true args=true idempotent=true explicit-context=true native-cancel=true");
    // The debug ABI emits one character at a time across CPUs. Keep a short
    // completion token after ALL assertions and repeat it so unrelated boot
    // logs cannot turn a successful SMP probe into a parser timeout.
    for _ in 0..8 {
        nanami_runtime::println!("KCONT-PASS");
        nanami_runtime::println!("CAPABILITY-PROBE-DONE");
    }
    // Keep all test backing reserved; no test claims failed revoke freed it.
    arch::process_control_block::suspend(environment.capabilities.process_control_block).unwrap();
    loop {
        nanami_runtime::yield_now();
    }
}
