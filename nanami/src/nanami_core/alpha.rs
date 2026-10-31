mod boot;
#[cfg(target_arch = "x86_64")]
mod boot_trace;
mod control_requests;
mod domain_manifest;
mod domains;
mod mapping_protection;
mod memory_copy;
mod memory_init;
mod memory_pages;
mod memory_requests;
mod mmio;
mod process_policy;
mod process_requests;
mod service_buffers;
mod service_events;
mod service_requests;
mod shared_memory;
mod support;
mod threads;
mod version_info;

#[cfg(target_arch = "x86_64")]
#[path = "alpha/arch/x86_64.rs"]
mod arch_impl;

#[cfg(target_arch = "aarch64")]
#[path = "alpha/arch/aarch64.rs"]
mod arch_impl;

use self::memory_pages::PageSource;
use self::process_policy::{ComponentSpec, ProcessPolicies, ProcessPolicy};
use self::support::*;

use crate::nanami_core::capability_space::RootCapabilitySpace;
use crate::nanami_core::communication::{
    CommunicationEvent, CommunicationManager, KernelFaultEvent, NotificationEvent, OsRequestEvent,
    OS_REQUEST_DEBUG_PING, OS_REQUEST_DMA_REQUEST, OS_REQUEST_DRIVER_PLATFORM_INFO,
    OS_REQUEST_EXIT, OS_REQUEST_HEAP_ALLOC, OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION,
    OS_REQUEST_IO_PORT_CONTROL, OS_REQUEST_IRQ_CONTROL, OS_REQUEST_MAPPING_RELEASE,
    OS_REQUEST_MMIO_REQUEST, OS_REQUEST_NANAMI_CONTROL, OS_REQUEST_NANAMI_INFO,
    OS_REQUEST_NOTIFICATION_PORT_COPY, OS_REQUEST_NOTIFICATION_PORT_CREATE, OS_REQUEST_PAGE_ALLOC,
    OS_REQUEST_PROCESS_ALIVE, OS_REQUEST_PROCESS_EXEC_MEMORY, OS_REQUEST_PROCESS_KILL,
    OS_REQUEST_PROCESS_MAP_ANONYMOUS, OS_REQUEST_PROCESS_MEMORY_CLONE,
    OS_REQUEST_PROCESS_MEMORY_COPY_WITHIN, OS_REQUEST_PROCESS_MEMORY_READ,
    OS_REQUEST_PROCESS_MEMORY_WRITE, OS_REQUEST_PROCESS_REAP, OS_REQUEST_PROCESS_SPAWN,
    OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER, OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER_SUSPENDED,
    OS_REQUEST_PROCESS_SPAWN_MEMORY, OS_REQUEST_PROCESS_SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED,
    OS_REQUEST_PROCESS_SPAWN_MEMORY_SUSPENDED, OS_REQUEST_PROCESS_STATUS, OS_REQUEST_SELF_PID,
    OS_REQUEST_SERVICE_CONNECT, OS_REQUEST_SERVICE_LIST, OS_REQUEST_SERVICE_REGISTER,
    OS_REQUEST_SHARED_FRAMEBUFFER_CREATE, OS_REQUEST_SHARED_MEMORY_CREATE, OS_RESPONSE_FATAL,
    OS_RESPONSE_ILLEGAL_OPERATION, OS_RESPONSE_INVALID_ARGUMENT, OS_RESPONSE_INVALID_DESCRIPTOR,
    OS_RESPONSE_OK, OS_RESPONSE_PERMISSION_DENIED, OS_RESPONSE_PONG_MAGIC,
};
use crate::nanami_core::cpio;
use crate::nanami_core::elf_loader::parse_elf64;
use crate::nanami_core::memory::MemoryManager;
use crate::nanami_core::process::{ProcessLazyMappingKind, ProcessManager};
use crate::nanami_core::vm_space::VmTracker;
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use crate::nanami_utils::heap::init_global_heap;
use crate::{debug, error, info, warn};
use alloc::vec::Vec;
use core::ptr;
use nanami_uapi::alpha::{
    AlphaEnvironment, FramebufferInformation as InitialFramebufferInformation,
};
use nanami_uapi::os::{
    NANAMI_INFO_ARCHITECTURE_NAME, NANAMI_INFO_KERNEL_VERSION, NANAMI_INFO_MEMORY,
    NANAMI_INFO_OS_VERSION, NANAMI_INFO_PLATFORM_NAME, NANAMI_INFO_PROCESS, NANAMI_INFO_SMP,
};
use nun::{arch, CapabilityDescriptor, CapabilityError, CapabilityType, Word};

const ORIGINAL_OS_PORT_SLOT: usize = 64;
const SERVICE_DIRECTORY_SLOT: usize = 65;
const EVENT_DIRECTORY_SLOT: usize = 66;
const NOTIFICATION_COPY_TEMP_SLOT: usize = 67;
const SERVICE_DIRECTORY_RADIX: usize = 8;
const PROCESS_ROOT_RADIX: usize = 8;
const PROCESS_SLOT_PCB: usize = 1;
const PROCESS_SLOT_OS_PORT: usize = 2;
const PROCESS_SLOT_ADDRESS_SPACE: usize = 3;
const PROCESS_SLOT_L3_NODE: usize = 4;
const PROCESS_SLOT_L2_NODE: usize = 5;
const PROCESS_SLOT_L1_NODE: usize = 6;
const PROCESS_SLOT_FRAME_NODE: usize = 7;
const PROCESS_SLOT_SERVICE_PORT: usize = 20;
const PROCESS_SLOT_NOTIFICATION_PORT: usize = 21;
const PROCESS_SLOT_FAULT_RESOLVER: usize = 22;
// A9N ObjectRights READ|WRITE: fixed identity, no COPY or MODIFY.
// READ is temporarily retained for the current target-slot cleanup contract.
const IDENTIFIED_IPC_RIGHTS: Word = 0b11;
const PROCESS_FRAME_DIRECTORY_RADIX: usize = 10;
const PROCESS_FRAME_NODE_RADIX: usize = 9;
const PROCESS_FRAME_CHUNK_PAGES: usize = 1 << PROCESS_FRAME_NODE_RADIX;
const PROCESS_FRAME_TOTAL_PAGES: usize =
    (1 << PROCESS_FRAME_DIRECTORY_RADIX) * PROCESS_FRAME_CHUNK_PAGES;
const PAGE_TABLE_NODE_RADIX: usize = 7;
const PAGE_SIZE: usize = 4096;
const USER_STACK_BASE: usize = 0x0400_0000;
const USER_STACK_PAGES: usize = 64;
const USER_ANONYMOUS_MAP_BASE: usize = 0x1000_0000;
const USER_HEAP_GUARD_PAGES: usize = 8;
const USER_HEAP_LIMIT: usize = 0x7000_0000;
const PROCESS_COPY_TEMP_BASE: usize = 0x6800_0000;
const PROCESS_COPY_TEMP_WINDOW_SIZE: usize = 0x0100_0000;
const PROCESS_ZERO_TEMP_BASE: usize = PROCESS_COPY_TEMP_BASE + PROCESS_COPY_TEMP_WINDOW_SIZE;
// The copy window below is also the largest image that can be staged at once.
// Keep the static backing buffer at that effective limit: making it larger only
// inflates Nanami's BSS (and can make the init ELF exceed A9N's load capacity).
const PROCESS_SPAWN_MEMORY_MAX_BYTES: usize = PROCESS_COPY_TEMP_WINDOW_SIZE;
static mut PROCESS_MEMORY_IMAGE_BUFFER: [u8; PROCESS_SPAWN_MEMORY_MAX_BYTES] =
    [0; PROCESS_SPAWN_MEMORY_MAX_BYTES];
const ALPHA_RUNTIME_STACK_NODE_SLOT: usize = 1300;
const ALPHA_RUNTIME_STACK_NODE_RADIX: usize = 12;
const ALPHA_RUNTIME_STACK_BASE: usize = 0x5000_0000;
const ALPHA_RUNTIME_STACK_PAGES: usize = 1024;
const ALPHA_HEAP_BASE: usize = 0x5800_0000;
const ALPHA_HEAP_PAGES: usize = 8192;
const PROCESS_DEVICE_SLOT_MIN: usize = 8;
const PROCESS_DEVICE_SLOT_MAX: usize = (1 << PROCESS_ROOT_RADIX) - 1;
const PROCESS_IRQ_NOTIFICATION_ALIAS_MIN: usize = 224;
const PROCESS_IRQ_NOTIFICATION_ALIAS_MAX: usize = 255;
const PROCESS_IRQ_NOTIFICATION_ALIAS_COUNT: usize =
    PROCESS_IRQ_NOTIFICATION_ALIAS_MAX - PROCESS_IRQ_NOTIFICATION_ALIAS_MIN + 1;
const FRAMEBUFFER_INFORMATION_REGION: usize = 0;
const FRAMEBUFFER_INFORMATION_GEOMETRY: usize = 1;
const FRAMEBUFFER_INFORMATION_FORMAT: usize = 2;
const FRAMEBUFFER_INFORMATION_COLOR_AND_ID: usize = 3;
const PROCESS_ROOT_RESERVED_SLOTS: [usize; 29] = [
    1024,
    1025,
    1026,
    1027, // physical generic node candidates
    1028,
    1029,
    1030,
    1031, // kernel-object pool candidates
    1040,
    1041,
    1042,
    1043, // initial frame metadata pool candidates
    1100,
    1101,
    1102,
    1103, // physical frame node candidates
    1200,
    1201,
    1202,
    1203, // page-table pool node candidates
    1204,
    1205,
    1206,
    1207, // page-table Generic owner node candidates
    1210,
    1211,
    1212,
    1213, // process arena directory candidates
    ALPHA_RUNTIME_STACK_NODE_SLOT,
];

pub struct Alpha {
    initramfs_image: &'static [u8],
    boot_configuration: &'static [u8],
    root: RootCapabilitySpace,
    memory: MemoryManager,
    processes: ProcessManager,
    communication: CommunicationManager,
    initial_framebuffer: InitialFramebufferInformation,
    interrupt_region: CapabilityDescriptor,
    root_io_port: CapabilityDescriptor,
    architecture_name: [u8; 32],
    platform_name: [u8; 32],
    kernel_version: alloc::string::String,
    platform_rsdp_address: usize,
    runtime_stack_top: usize,
    copy_window: memory_copy::CopyWindow,
    initialization_window: memory_init::InitializationWindow,
    launch_capabilities: nanami_uapi::alpha::SelfCapabilities,
    platform_information: nanami_uapi::alpha::PlatformInformation,
    domain_id: usize,
    children: Vec<domains::ChildDomain>,
    pending_children: Vec<domain_manifest::ChildSpec<'static>>,
    process_policies: ProcessPolicies<'static>,
    service_endpoints: [bool; 32],
    service_endpoint_poisoned: [bool; 32],
    service_wake_slots: [Option<Word>; 32],
    imported_services: [Option<nanami_uapi::alpha::ImportedService>; 32],
    import_policies: Vec<domain_manifest::ImportSpec<'static>>,
    managed_buffers: crate::nanami_core::communication::buffers::Buffers,
    managed_events: crate::nanami_core::communication::events::Events,
    event_directory_ready: bool,
}

enum FaultDisposition {
    Continue(KernelFaultEvent),
    ReceiveOnly,
}

enum PendingReply {
    FaultContinue(KernelFaultEvent),
    Status(usize, usize, usize),
}

impl Alpha {
    /// The launcher's mapped inputs must be readable and owned for this Alpha's lifetime.
    pub unsafe fn bootstrap(environment: &AlphaEnvironment) -> Result<Self, CapabilityError> {
        environment
            .validate()
            .map_err(|_| CapabilityError::InvalidArgument)?;
        info!("alpha bootstrap start");

        info!("root capability space bootstrap");
        let mut root = RootCapabilitySpace::from_environment(environment);
        info!(
            "root={:#018x} radix={:>2} bootstrap_generic={:#018x}",
            root.root_descriptor, root.root_radix, root.bootstrap_generic
        );

        info!("memory manager bootstrap");
        let mut memory = MemoryManager::bootstrap(environment)?;
        root.bootstrap_generic = memory.kernel_object_generic();
        info!("memory manager ready");

        info!("create alpha os port");
        let os_port = create_alpha_os_port(
            root.root_descriptor,
            root.root_radix,
            root.bootstrap_generic,
        )?;
        info!("os port={:#018x}", os_port);
        arch::generic::convert(
            root.bootstrap_generic,
            CapabilityType::Node,
            SERVICE_DIRECTORY_RADIX,
            1,
            root.root_descriptor,
            SERVICE_DIRECTORY_SLOT,
        )?;

        info!("process manager / communication manager init");
        let alpha_address_space = environment.capabilities.address_space;
        debug!(
            "alpha address space descriptor={:#018x}",
            alpha_address_space
        );
        let mut processes = ProcessManager::new_alpha(
            root.root_descriptor,
            alpha_address_space,
            environment.capabilities.process_control_block,
            os_port,
            1usize << root.root_radix,
            &PROCESS_ROOT_RESERVED_SLOTS,
            environment.platform.core_count,
        )?;
        let mut communication = CommunicationManager::new(os_port);
        info!("managers ready");

        info!("alpha heap bootstrap");
        let heap_physical_base = {
            let vm_space = processes.alpha_vm_space_mut();
            Self::prepare_alpha_heap(
                environment,
                &mut memory,
                root.bootstrap_generic,
                alpha_address_space,
                vm_space,
            )?
        };
        unsafe {
            init_global_heap(ALPHA_HEAP_BASE, ALPHA_HEAP_PAGES * PAGE_SIZE);
        }
        info!(
            "alpha heap ready va=[{:#018x}..{:#018x}) pa={:#018x}",
            ALPHA_HEAP_BASE,
            ALPHA_HEAP_BASE + ALPHA_HEAP_PAGES * PAGE_SIZE,
            heap_physical_base
        );

        info!("physical allocator bootstrap");
        memory.initialize_physical_allocator()?;
        memory.allocate_physical_at(heap_physical_base, ALPHA_HEAP_PAGES * PAGE_SIZE, false)?;
        info!("physical allocator ready");

        info!("capture initial framebuffer information");
        let initial_framebuffer = environment.framebuffer;
        if initial_framebuffer.address != 0 {
            info!(
                "framebuffer phys={:#018x} size={:#x} {}x{} stride={} bpp={}",
                initial_framebuffer.address,
                initial_framebuffer.size_bytes,
                initial_framebuffer.width,
                initial_framebuffer.height,
                initial_framebuffer.stride,
                initial_framebuffer.bits_per_pixel
            );
        }

        info!("alpha bootstrap complete");
        let interrupt_region = environment.capabilities.interrupt_region;
        let root_io_port = environment.capabilities.io_ports;
        let mut imported_services = [None; 32];
        for &import in environment
            .imports()
            .map_err(|_| CapabilityError::InvalidArgument)?
        {
            let name = import
                .name()
                .map_err(|_| CapabilityError::InvalidArgument)?;
            communication.services.can_publish(name)?;
            let id = communication
                .bindings
                .import_service(import.session_limit)?;
            communication
                .services
                .publish(name, id.0, import.endpoint)?;
            imported_services[id.0 - 1] = Some(import);
        }
        let configuration =
            core::str::from_utf8(unsafe { environment.boot_configuration.as_slice() })
                .map_err(|_| CapabilityError::InvalidArgument)?;

        Ok(Self {
            initramfs_image: unsafe { environment.boot_archive.as_slice() },
            boot_configuration: unsafe { environment.boot_configuration.as_slice() },
            root,
            memory,
            processes,
            communication,
            initial_framebuffer,
            interrupt_region,
            root_io_port,
            architecture_name: environment.platform.architecture,
            platform_name: environment.platform.platform,
            kernel_version: version_info::kernel_version(&environment.platform.kernel),
            platform_rsdp_address: environment.platform.rsdp_address,
            runtime_stack_top: 0,
            copy_window: memory_copy::CopyWindow::new(),
            initialization_window: memory_init::InitializationWindow::new(),
            launch_capabilities: environment.capabilities,
            platform_information: environment.platform,
            domain_id: environment.domain_id,
            children: Vec::new(),
            pending_children: Vec::new(),
            process_policies: ProcessPolicies::parse(
                core::str::from_utf8(unsafe { environment.boot_configuration.as_slice() })
                    .map_err(|_| CapabilityError::InvalidArgument)?,
                environment.platform.core_count,
            )?,
            service_endpoints: [false; 32],
            service_endpoint_poisoned: [false; 32],
            service_wake_slots: [None; 32],
            imported_services,
            import_policies: domain_manifest::imports(configuration)?,
            managed_buffers: crate::nanami_core::communication::buffers::Buffers::new(),
            managed_events: crate::nanami_core::communication::events::Events::new(),
            event_directory_ready: false,
        })
    }

    pub fn start(&mut self) {
        if crate::nanami_core::process::SAME_CORE_DIAGNOSTIC {
            crate::force_info!(
                "[smp.diag] all-processes-core=0 kernel-cores={} kernel-smp-unchanged=true",
                self.processes.online_core_count()
            );
        }
        self.spawn_components_from_initramfs();
        crate::force_info!(
            "[alpha] domain={} children={} online",
            self.domain_id,
            self.children.len()
        );
        crate::force_info!(
            "alpha online: smp_cores={} alpha_core=0",
            self.processes.online_core_count()
        );
    }

    fn spawn_components_from_initramfs(&mut self) {
        info!("[proc] scan initramfs");
        let mut spawned = 0usize;
        let mut failed = 0usize;

        {
            let boot_list = self.boot_configuration;
            let mut line_number = 0usize;
            for line in core::str::from_utf8(boot_list).unwrap_or("").lines() {
                line_number += 1;
                if line
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .is_some_and(|directive| matches!(directive, "@process" | "@import"))
                {
                    continue;
                }
                match domains::ChildSpec::parse(line) {
                    Ok(Some(spec)) => {
                        let ready = match self.child_ready(spec) {
                            Ok(ready) => ready,
                            Err(error) => {
                                failed += 1;
                                error!(
                                    "[domain] launch failed label={} import={:?}",
                                    spec.label, error
                                );
                                continue;
                            }
                        };
                        if !ready {
                            self.pending_children.push(spec);
                            continue;
                        }
                        match self.spawn_child_alpha(spec) {
                            Ok(()) => spawned += 1,
                            Err(error) => {
                                failed += 1;
                                error!(
                                    "[domain] launch failed line={} err={:?}",
                                    line_number, error
                                );
                            }
                        }
                        continue;
                    }
                    Err(error) => {
                        failed += 1;
                        error!(
                            "[domain] invalid manifest line={} err={:?}",
                            line_number, error
                        );
                        continue;
                    }
                    Ok(None) => {}
                }
                let entry = match ComponentSpec::parse(line) {
                    Ok(Some(entry)) => entry,
                    Ok(None) => continue,
                    Err(error) => {
                        failed += 1;
                        error!(
                            "[proc.err] boot-list invalid line={} err={:?}",
                            line_number, error
                        );
                        continue;
                    }
                };
                match self.spawn_initramfs_image(
                    entry.image_path,
                    0,
                    None,
                    true,
                    Some(entry.priority),
                ) {
                    Ok(pid) => {
                        info!("[proc] configured component={} pid={}", entry.name, pid);
                        spawned += 1;
                    }
                    Err(e) => {
                        failed += 1;
                        error!(
                            "[proc.err] boot-list spawn failed line={} image={} err={:?}",
                            line_number, entry.image_path, e
                        );
                    }
                }
            }
            info!(
                "[proc] boot-list spawn summary ok={:>3} failed={:>3}",
                spawned, failed
            );
        }
    }

    fn run_event_loop(&mut self) -> ! {
        // Server loop: first blocking receive, then reply_receive only when a reply is pending.
        let mut event = match self.communication.receive_event() {
            Ok(event) => event,
            Err(e) => {
                error!("[ipc.err] initial receive failed: {:?}", e);
                panic!("initial receive failed");
            }
        };

        loop {
            let pending_reply = match event {
                CommunicationEvent::KernelFault(fault) => {
                    match self.handle_kernel_fault_event(fault) {
                        FaultDisposition::Continue(fault) => {
                            Some(PendingReply::FaultContinue(fault))
                        }
                        FaultDisposition::ReceiveOnly => None,
                    }
                }
                CommunicationEvent::Notification(notification) => {
                    self.handle_notification_event(notification);
                    None
                }
                CommunicationEvent::OsRequest(request) => {
                    info!(
                        "[ipc] os request received: id={:>3} code={:#018x}",
                        request.identifier, request.code
                    );
                    let (status, detail0, detail1) = self.handle_os_request(request);
                    if matches!(
                        request.code,
                        OS_REQUEST_SERVICE_REGISTER
                            | nanami_uapi::service::OS_REQUEST_SERVICE_PUBLISH
                    ) && status == OS_RESPONSE_OK
                    {
                        self.start_ready_children();
                    }
                    let stopped_session_caller = request.code
                        == nanami_uapi::service::OS_REQUEST_SESSION_CONNECT
                        && status == OS_RESPONSE_FATAL
                        && self
                            .processes
                            .find_entry_by_pid(request.identifier)
                            .is_some_and(|entry| entry.exited);
                    if (request.code == OS_REQUEST_EXIT && status == OS_RESPONSE_OK)
                        || stopped_session_caller
                    {
                        None
                    } else {
                        Some(PendingReply::Status(status, detail0, detail1))
                    }
                }
            };

            event = if let Some(reply) = pending_reply {
                let result = match reply {
                    PendingReply::FaultContinue(fault) => {
                        self.communication.reply_receive_fault_continue(fault)
                    }
                    PendingReply::Status(status, detail0, detail1) => self
                        .communication
                        .reply_receive_status(status, detail0, detail1),
                };
                match result {
                    Ok(event) => event,
                    Err(e) => {
                        error!("[ipc.err] reply_receive failed: {:?}", e);
                        panic!("reply_receive failed");
                    }
                }
            } else {
                match self.communication.receive_event() {
                    Ok(event) => event,
                    Err(e) => {
                        error!("[ipc.err] receive failed: {:?}", e);
                        panic!("receive failed");
                    }
                }
            };
        }
    }

    pub fn switch_to_runtime_stack_and_run(&'static mut self) -> ! {
        if self.runtime_stack_top == 0 {
            match self.prepare_runtime_stack() {
                Ok(top) => {
                    self.runtime_stack_top = top;
                    info!("[stack] runtime stack prepared top={:#018x}", top);
                }
                Err(e) => {
                    error!("[stack.err] runtime stack prepare failed: {:?}", e);
                    self.run_event_loop();
                }
            }
        }
        unsafe { jump_to_relocated_stack(self as *mut Alpha, self.runtime_stack_top) }
    }

    fn handle_kernel_fault_event(&mut self, fault: KernelFaultEvent) -> FaultDisposition {
        if self.try_handle_demand_page_fault(fault).is_ok() {
            return FaultDisposition::Continue(fault);
        }

        error!(
            "[fault] id={:>3} reason={} pc={:#018x} addr={:#018x} arch_code={:#018x}",
            fault.identifier,
            fault.reason,
            fault.program_counter,
            fault.fault_address,
            fault.architecture_fault_code
        );

        let pid = fault.identifier;
        if pid == 0 {
            error!("[fault] unknown sender id={:>3}, ignored", pid);
            return FaultDisposition::ReceiveOnly;
        }
        let Some(entry) = self.processes.find_entry_by_pid(pid) else {
            error!("[fault] unknown sender id={:>3}, no entry", pid);
            return FaultDisposition::ReceiveOnly;
        };
        if entry.pcb == 0 {
            error!("[fault] pid={:>3} has no active pcb", pid);
            return FaultDisposition::ReceiveOnly;
        }

        // show all registers
        let _ = arch::process_control_block::read_register(entry.pcb, 22);

        // CLEAN: move to hal
        const REG_NAMES: [&str; 22] = [
            "RAX", "RBX", "RCX", "RDX", "RDI", "RSI", "RBP", "R8 ", "R9 ", "R10", "R11", "R12",
            "R13", "R14", "R15", "RIP", "CS ", "RFLAGS", "RSP", "SS ", "GS_BASE", "FS_BASE",
        ];

        // DEBUG
        let ipc_buffer = arch::ipc_buffer::get_ipc_buffer();
        for reg in 0..22 {
            info!(
                "{} = {:#018x}",
                REG_NAMES[reg],
                ipc_buffer.get_message(reg + 3)
            );
        }

        let _ = arch::process_control_block::suspend(entry.pcb);
        error!("[fault] pid={:>3} suspended (pcb={:#018x})", pid, entry.pcb);
        FaultDisposition::ReceiveOnly
    }

    fn handle_notification_event(&mut self, notification: NotificationEvent) {
        let NotificationEvent {
            identifier: _identifier,
            value: _value,
        } = notification;
        debug!(
            "[ipc] notification received: id={:>3} value={:#018x}",
            _identifier, _value
        );
    }

    fn handle_os_request(&mut self, request: OsRequestEvent) -> (usize, usize, usize) {
        debug!(
            "[ipc] req id={:>3} code={:#018x} arg0={:#018x} arg1={:#018x} arg2={:#018x} arg3={:#018x}",
            request.identifier,
            request.code,
            request.arg0,
            request.arg1,
            request.arg2,
            request.arg3
        );

        let response = match request.code {
            OS_REQUEST_DEBUG_PING => {
                info!(
                    "[ipc] ping from pid={:>3} token={:#018x}",
                    request.identifier, request.arg0
                );
                (OS_RESPONSE_OK, request.arg0, OS_RESPONSE_PONG_MAGIC)
            }
            OS_REQUEST_PAGE_ALLOC => {
                response_from_detail_result(self.handle_page_alloc_request(request))
            }
            OS_REQUEST_HEAP_ALLOC => {
                response_from_details_result(self.handle_heap_alloc_request(request))
            }
            OS_REQUEST_SELF_PID => (OS_RESPONSE_OK, request.identifier, 0),
            OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION => response_from_details_result(
                self.handle_initial_framebuffer_information_request(request),
            ),
            OS_REQUEST_EXIT => response_from_status_result(self.handle_exit_request(request)),
            OS_REQUEST_DMA_REQUEST => {
                response_from_details_result(self.handle_dma_request(request))
            }
            OS_REQUEST_MMIO_REQUEST => {
                response_from_details_result(self.handle_mmio_request(request))
            }
            OS_REQUEST_SHARED_MEMORY_CREATE => {
                response_from_details_result(self.handle_shared_memory_request(request))
            }
            nanami_uapi::os::OS_REQUEST_MAPPING_SHARE => {
                response_from_details_result(self.handle_mapping_share_request(request))
            }
            OS_REQUEST_MAPPING_RELEASE => {
                response_from_status_result(self.handle_mapping_release_request(request))
            }
            nanami_uapi::os::OS_REQUEST_PROCESS_MEMORY_COMPARE_EXCHANGE => {
                response_from_detail_result(self.handle_process_compare_exchange(request))
            }
            OS_REQUEST_SHARED_FRAMEBUFFER_CREATE => {
                response_from_details_result(self.handle_shared_framebuffer_request(request))
            }
            OS_REQUEST_SERVICE_CONNECT => {
                response_from_detail_result(self.handle_service_connect_request(request))
            }
            nanami_uapi::service::OS_REQUEST_PROVIDER_REGISTER => {
                response_from_detail_result(self.handle_provider_register(request))
            }
            nanami_uapi::service::OS_REQUEST_SERVICE_PUBLISH => {
                response_from_detail_result(self.handle_service_publish(request))
            }
            nanami_uapi::service::OS_REQUEST_SESSION_CONNECT => {
                response_from_details_result(self.handle_session_connect(request))
            }
            nanami_uapi::service::OS_REQUEST_BINDING_CONTROL => {
                response_from_details_result(self.handle_binding_control(request))
            }
            OS_REQUEST_SERVICE_REGISTER => {
                response_from_detail_result(self.handle_service_register_request(request))
            }
            OS_REQUEST_SERVICE_LIST => match self.handle_service_list_request(request) {
                Some((detail0, detail1)) => (OS_RESPONSE_OK, detail0, detail1),
                None => (OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
            },
            OS_REQUEST_PROCESS_SPAWN => {
                response_from_detail_result(self.handle_process_spawn_request(request))
            }
            nanami_uapi::os::OS_REQUEST_PROCESS_THREAD_CREATE => {
                response_from_detail_result(self.handle_thread_create(request))
            }
            nanami_uapi::os::OS_REQUEST_PROCESS_FORK => {
                response_from_detail_result(self.handle_raw_task_create(request, false))
            }
            OS_REQUEST_PROCESS_SPAWN_MEMORY => {
                response_from_detail_result(self.handle_process_spawn_memory_request(request))
            }
            OS_REQUEST_PROCESS_SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED => response_from_detail_result(
                self.handle_process_spawn_memory_fault_handler_suspended_request(request),
            ),
            OS_REQUEST_PROCESS_SPAWN_MEMORY_SUSPENDED => response_from_detail_result(
                self.handle_process_spawn_memory_suspended_request(request),
            ),
            OS_REQUEST_PROCESS_EXEC_MEMORY => {
                response_from_detail_result(self.handle_process_exec_memory_request(request))
            }
            OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER => response_from_detail_result(
                self.handle_process_spawn_fault_handler_request(request),
            ),
            OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER_SUSPENDED => response_from_detail_result(
                self.handle_process_spawn_fault_handler_suspended_request(request),
            ),
            OS_REQUEST_PROCESS_STATUS => {
                response_from_details_result(self.handle_process_status_request(request))
            }
            OS_REQUEST_PROCESS_ALIVE => response_from_detail_result(
                self.handle_process_alive_request(request)
                    .map(|alive| alive as usize),
            ),
            OS_REQUEST_PROCESS_REAP => {
                response_from_status_result(self.handle_process_reap_request(request))
            }
            OS_REQUEST_PROCESS_KILL => {
                response_from_status_result(self.handle_process_kill_request(request))
            }
            OS_REQUEST_PROCESS_MEMORY_READ => {
                response_from_status_result(self.handle_process_memory_copy_request(request, false))
            }
            OS_REQUEST_PROCESS_MEMORY_WRITE => {
                response_from_status_result(self.handle_process_memory_copy_request(request, true))
            }
            OS_REQUEST_PROCESS_MEMORY_CLONE => {
                response_from_status_result(self.handle_process_memory_clone_request(request))
            }
            OS_REQUEST_PROCESS_MEMORY_COPY_WITHIN => {
                response_from_status_result(self.handle_process_memory_copy_within_request(request))
            }
            OS_REQUEST_PROCESS_MAP_ANONYMOUS => {
                response_from_details_result(self.handle_process_map_anonymous_request(request))
            }
            OS_REQUEST_NANAMI_CONTROL => {
                response_from_status_result(self.handle_nanami_control_request(request))
            }
            OS_REQUEST_NANAMI_INFO => {
                response_from_details_result(self.handle_nanami_info_request(request))
            }
            OS_REQUEST_DRIVER_PLATFORM_INFO => {
                response_from_details_result(self.handle_driver_platform_info_request(request))
            }
            OS_REQUEST_IRQ_CONTROL => {
                response_from_status_result(self.handle_irq_control_request(request))
            }
            OS_REQUEST_IO_PORT_CONTROL => {
                response_from_status_result(self.handle_io_port_control_request(request))
            }
            OS_REQUEST_NOTIFICATION_PORT_CREATE => {
                response_from_status_result(self.handle_notification_port_create_request(request))
            }
            OS_REQUEST_NOTIFICATION_PORT_COPY => {
                response_from_status_result(self.handle_notification_port_copy_request(request))
            }
            nanami_uapi::os::OS_REQUEST_PROCESS_MAPPING_PROTECT => {
                response_from_status_result(self.handle_mapping_protect_request(request))
            }
            _ => {
                warn!(
                    "[ipc.warn] unknown request code={:#018x} id={:>3}",
                    request.code, request.identifier
                );
                response_from_status_result(Err(CapabilityError::InvalidArgument))
            }
        };

        debug!(
            "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
            request.identifier, response.0, response.1, response.2
        );
        response
    }
}
