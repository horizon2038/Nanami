use crate::nanami_core::capability_space::RootCapabilitySpace;
use crate::nanami_core::communication::{
    CommunicationEvent, CommunicationManager, KernelFaultEvent, NotificationEvent, OsRequestEvent,
    OS_REQUEST_DEBUG_PING, OS_REQUEST_DMA_REQUEST, OS_REQUEST_EXIT, OS_REQUEST_HEAP_ALLOC,
    OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION, OS_REQUEST_IO_PORT_CONTROL, OS_REQUEST_IRQ_CONTROL,
    OS_REQUEST_MAPPING_RELEASE, OS_REQUEST_MMIO_REQUEST, OS_REQUEST_NANAMI_CONTROL,
    OS_REQUEST_NANAMI_INFO, OS_REQUEST_NOTIFICATION_PORT_COPY, OS_REQUEST_NOTIFICATION_PORT_CREATE,
    OS_REQUEST_PAGE_ALLOC, OS_REQUEST_PROCESS_ALIVE, OS_REQUEST_PROCESS_EXEC_MEMORY,
    OS_REQUEST_PROCESS_KILL, OS_REQUEST_PROCESS_MAP_ANONYMOUS, OS_REQUEST_PROCESS_MEMORY_CLONE,
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
use core::arch::asm;
use core::ptr;
use nun::{
    arch, convert_capability_result, CapabilityDescriptor, CapabilityError, CapabilityType,
    FramebufferInfo, InitInfo, InitSlotOffset, KernelCallType, Sword, Word,
};

const ORIGINAL_OS_PORT_SLOT: usize = 64;
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
const PROCESS_PRIORITY_LOW: Word = 4;
const PROCESS_PRIORITY_BACKGROUND_CLIENT: Word = 8;
const PROCESS_PRIORITY_CLIENT: Word = 16;
const PROCESS_PRIORITY_INTERACTIVE_CLIENT: Word = 18;
const PROCESS_PRIORITY_BACKGROUND_SERVER: Word = 21;
const PROCESS_PRIORITY_GUI_SERVER: Word = 24;
const PROCESS_PRIORITY_INPUT_SERVER: Word = 28;
const PROCESS_PRIORITY_TIMER_SERVER: Word = 30;
const USER_HEAP_GUARD_PAGES: usize = 8;
const USER_HEAP_LIMIT: usize = 0x7000_0000;
const TEMP_MAP_BASE: usize = 0x7000_0000;
const TEMP_MAP_STRIDE: usize = 0x0020_0000;
const PROCESS_COPY_TEMP_BASE: usize = 0x6800_0000;
const PROCESS_COPY_TEMP_WINDOW_SIZE: usize = 0x0100_0000;
const PROCESS_ZERO_TEMP_BASE: usize = PROCESS_COPY_TEMP_BASE + PROCESS_COPY_TEMP_WINDOW_SIZE;
const PROCESS_SPAWN_MEMORY_MAX_BYTES: usize = 64 * 1024 * 1024;
const NANAMI_INFO_MEMORY: usize = 1;
const NANAMI_INFO_PROCESS: usize = 2;
static mut PROCESS_COPY_BOUNCE_BUFFER: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
static mut PROCESS_MEMORY_IMAGE_BUFFER: [u8; PROCESS_SPAWN_MEMORY_MAX_BYTES] =
    [0; PROCESS_SPAWN_MEMORY_MAX_BYTES];
const ALPHA_RUNTIME_STACK_NODE_SLOT: usize = 1300;
const ALPHA_RUNTIME_STACK_NODE_RADIX: usize = 12;
const ALPHA_RUNTIME_STACK_BASE: usize = 0x5000_0000;
const ALPHA_RUNTIME_STACK_PAGES: usize = 1024;
const ALPHA_HEAP_BASE: usize = 0x5800_0000;
const ALPHA_HEAP_PAGES: usize = 8192;
const GENERIC_NODE_RADIX: usize = 7;
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
const PROCESS_ROOT_RESERVED_SLOTS: [usize; 17] = [
    1024,
    1025,
    1026,
    1027, // physical generic node candidates
    1100,
    1101,
    1102,
    1103, // physical frame node candidates
    1200,
    1201,
    1202,
    1203, // page-table pool node candidates
    1210,
    1211,
    1212,
    1213, // process arena directory candidates
    ALPHA_RUNTIME_STACK_NODE_SLOT,
];

const INITRAMFS_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/servers/initramfs.cpio"
));

pub struct Alpha {
    root: RootCapabilitySpace,
    memory: MemoryManager,
    processes: ProcessManager,
    communication: CommunicationManager,
    initial_framebuffer: InitialFramebufferInformation,
    interrupt_region: CapabilityDescriptor,
    root_io_port: CapabilityDescriptor,
    runtime_stack_top: usize,
}

#[derive(Clone, Copy)]
struct InitialFramebufferInformation {
    display_id: usize,
    address: usize,
    size_bytes: usize,
    width: usize,
    height: usize,
    stride: usize,
    bits_per_pixel: usize,
    red_position: usize,
    red_size: usize,
    green_position: usize,
    green_size: usize,
    blue_position: usize,
    blue_size: usize,
}

#[derive(Clone, Copy)]
struct BootListEntry<'a> {
    _name: &'a str,
    priority: Word,
    image_path: &'a str,
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
    pub fn bootstrap(init_info: &InitInfo) -> Result<Self, CapabilityError> {
        info!("alpha bootstrap start");

        info!("root capability space bootstrap");
        let mut root = RootCapabilitySpace::bootstrap(init_info)?;
        info!(
            "root={:#018x} radix={:>2} bootstrap_generic={:#018x}",
            root.root_descriptor, root.root_radix, root.bootstrap_generic
        );

        info!("memory manager bootstrap");
        let mut memory = MemoryManager::bootstrap(
            init_info,
            root.root_descriptor,
            root.root_radix,
            root.bootstrap_generic_index,
            root.root_generic_index,
            root.root_generic_consumed_bytes,
        )?;
        root.bootstrap_generic = memory.kernel_object_generic();
        info!("memory manager ready");

        info!("create alpha os port");
        let os_port = create_alpha_os_port(
            root.root_descriptor,
            root.root_radix,
            root.bootstrap_generic,
        )?;
        info!("os port={:#018x}", os_port);

        info!("process manager / communication manager init");
        let alpha_address_space = make_root_slot_descriptor(
            root.root_radix,
            InitSlotOffset::ProcessAddressSpace as usize,
        );
        debug!(
            "alpha address space descriptor={:#018x}",
            alpha_address_space
        );
        let mut processes = ProcessManager::new_alpha(
            root.root_descriptor,
            alpha_address_space,
            os_port,
            1usize << root.root_radix,
            &PROCESS_ROOT_RESERVED_SLOTS,
        );
        let communication = CommunicationManager::new(os_port);
        info!("managers ready");

        info!("alpha heap bootstrap");
        let heap_physical_base = {
            let vm_space = processes.alpha_vm_space_mut();
            Self::prepare_alpha_heap(
                init_info,
                &mut memory,
                root.root_radix,
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
        memory.initialize_physical_allocator(init_info)?;
        memory.allocate_physical_at(heap_physical_base, ALPHA_HEAP_PAGES * PAGE_SIZE, false)?;
        info!("physical allocator ready");

        info!("capture initial framebuffer information");
        let initial_framebuffer = extract_initial_framebuffer_information(init_info)
            .ok_or(CapabilityError::InvalidArgument)?;
        info!(
            "framebuffer phys={:#018x} size={:#x} {}x{} stride={} bpp={}",
            initial_framebuffer.address,
            initial_framebuffer.size_bytes,
            initial_framebuffer.width,
            initial_framebuffer.height,
            initial_framebuffer.stride,
            initial_framebuffer.bits_per_pixel
        );

        info!("alpha bootstrap complete");
        let interrupt_region =
            make_root_slot_descriptor(root.root_radix, InitSlotOffset::InterruptRegion as usize);
        let root_io_port =
            make_root_slot_descriptor(root.root_radix, InitSlotOffset::IoPort as usize);

        Ok(Self {
            root,
            memory,
            processes,
            communication,
            initial_framebuffer,
            interrupt_region,
            root_io_port,
            runtime_stack_top: 0,
        })
    }

    pub fn start(&mut self) {
        self.spawn_components_from_initramfs();
        info!("alpha online");
    }

    fn ensure_process_frame_chunks(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        if page_count == 0 {
            return Ok(());
        }
        let end_slot = start_slot
            .checked_add(page_count)
            .ok_or(CapabilityError::InvalidArgument)?;
        if end_slot > PROCESS_FRAME_TOTAL_PAGES {
            return Err(CapabilityError::InvalidArgument);
        }

        let frame_directory = process_frame_directory_descriptor(process_root);
        let process_generic = self.process_arena_for_root(process_root)?;
        let mut chunk = start_slot / PROCESS_FRAME_CHUNK_PAGES;
        let last_chunk = (end_slot - 1) / PROCESS_FRAME_CHUNK_PAGES;
        while chunk <= last_chunk {
            if !self.processes.has_frame_chunk(pid, chunk) {
                arch::generic::convert(
                    process_generic,
                    CapabilityType::Node,
                    PROCESS_FRAME_NODE_RADIX as Word,
                    1,
                    frame_directory,
                    chunk as Word,
                )?;
                self.processes.register_frame_chunk(pid, chunk)?;
            }
            chunk += 1;
        }
        Ok(())
    }

    fn process_arena_for_root(
        &self,
        process_root: CapabilityDescriptor,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        let payload_bits = nun::WORD_BITS - nun::BYTE_BITS;
        let slot_shift = payload_bits
            .checked_sub(self.root.root_radix)
            .ok_or(CapabilityError::InvalidArgument)?;
        let slot_mask = (1usize << self.root.root_radix) - 1;
        let root_slot = (process_root >> slot_shift) & slot_mask;
        if make_root_slot_descriptor(self.root.root_radix, root_slot) != process_root {
            return Err(CapabilityError::InvalidArgument);
        }
        self.memory.process_arena_descriptor(root_slot)
    }

    fn allocate_process_frames(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<Vec<(usize, usize)>, CapabilityError> {
        self.ensure_process_frame_chunks(pid, process_root, start_slot, page_count)?;
        let mut allocated = Vec::new();
        let mut done = 0usize;
        while done < page_count {
            let global_slot = start_slot + done;
            let chunk = global_slot / PROCESS_FRAME_CHUNK_PAGES;
            let chunk_offset = global_slot % PROCESS_FRAME_CHUNK_PAGES;
            let chunk_remaining = PROCESS_FRAME_CHUNK_PAGES - chunk_offset;
            let batch = chunk_remaining.min(page_count - done);
            let allocated_pages = self.memory.allocate_process_frames(
                process_frame_chunk_descriptor(process_root, chunk),
                PROCESS_FRAME_NODE_RADIX,
                chunk_offset,
                batch,
            )?;
            for (offset, page) in allocated_pages.iter().enumerate() {
                let slot = global_slot + offset;
                allocated.push((slot, *page));
            }
            done += batch;
        }
        Ok(allocated)
    }

    fn spawn_components_from_initramfs(&mut self) {
        info!("[proc] scan initramfs");
        let mut spawned = 0usize;
        let mut failed = 0usize;

        if let Some(boot_list) = initramfs_entry_data("./nanami/boot-list") {
            let mut line_number = 0usize;
            for line in core::str::from_utf8(boot_list).unwrap_or("").lines() {
                line_number += 1;
                let Some(entry) = parse_boot_list_line(line) else {
                    continue;
                };
                match self.spawn_initramfs_image(
                    entry.image_path,
                    0,
                    None,
                    true,
                    Some(entry.priority),
                ) {
                    Ok(_) => {
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
            return;
        }

        warn!("[proc] missing /nanami/boot-list; fallback to legacy initramfs scan");
        let result = cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
            if !initramfs_image_is_auto_spawn_candidate(entry.name) {
                return Ok(());
            }
            if initramfs_image_is_explicit_only(entry.name) {
                info!("[proc] skip explicit-only image: {}", entry.name);
                return Ok(());
            }
            match self.spawn_process_from_elf(entry.name, entry.data, 0, None, true, None) {
                Ok(_) => {
                    spawned += 1;

                    // busy wait
                    /*
                    for _ in 0..10000 {
                        // architecture-independent
                        spin_loop();
                    }
                    */
                    Ok(())
                }
                Err(e) => {
                    failed += 1;
                    error!("[proc.err] spawn failed image={} err={:?}", entry.name, e);
                    Ok(())
                }
            }
        });

        if let Err(e) = result {
            error!("[proc.err] initramfs parse failed: {:?}", e);
        }
        info!(
            "[proc] initramfs spawn summary ok={:>3} failed={:>3}",
            spawned, failed
        );
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
                    if request.code == OS_REQUEST_EXIT && status == OS_RESPONSE_OK {
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

    fn spawn_process_from_elf(
        &mut self,
        image_name: &str,
        image_bytes: &'static [u8],
        reaper_pid: usize,
        resolver_port: Option<CapabilityDescriptor>,
        auto_resume: bool,
        priority_override: Option<Word>,
    ) -> Result<usize, CapabilityError> {
        info!("[proc] parse elf: {}", image_name);
        let elf = parse_elf64(image_bytes)?;
        info!(
            "[proc] elf entry={:#018x} segments={:>2}",
            elf.entry_point, elf.segment_count
        );

        let (pid, process_root_slot) = self.processes.alloc_process_slot()?;
        macro_rules! spawn_try {
            ($expr:expr) => {
                match $expr {
                    Ok(value) => value,
                    Err(error) => {
                        self.cleanup_failed_spawn(pid, process_root_slot);
                        return Err(error);
                    }
                }
            };
        }
        let child_root = make_root_slot_descriptor(self.root.root_radix, process_root_slot);
        let child_pcb =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_PCB);
        let child_os_port =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_OS_PORT);
        let child_address_space =
            make_child_slot_descriptor(child_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_ADDRESS_SPACE);
        let child_notification = make_child_slot_descriptor(
            child_root,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_NOTIFICATION_PORT,
        );
        let process_generic = spawn_try!(self.memory.ensure_process_arena(process_root_slot));

        info!(
            "[proc] create child root slot={:>3} desc={:#018x}",
            process_root_slot, child_root
        );
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PROCESS_ROOT_RADIX as Word,
            1,
            self.root.root_descriptor,
            process_root_slot as Word,
        ));

        info!("[proc] populate child root");
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::ProcessControlBlock,
            14,
            1,
            child_root,
            PROCESS_SLOT_PCB as Word,
        ));
        spawn_try!(arch::node::copy(
            child_root,
            PROCESS_SLOT_OS_PORT as Word,
            self.processes.alpha_entry().os_port,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::AddressSpace,
            0,
            1,
            child_root,
            PROCESS_SLOT_ADDRESS_SPACE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L3_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L2_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L1_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::Node,
            PROCESS_FRAME_DIRECTORY_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_FRAME_NODE as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::IpcPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_SERVICE_PORT as Word,
        ));
        spawn_try!(arch::generic::convert(
            process_generic,
            CapabilityType::NotificationPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_NOTIFICATION_PORT as Word,
        ));
        let _ = arch::notification_port::identify(child_notification, 0);
        let _ = arch::ipc_port::identify(child_os_port, pid as Word);

        let mut image_base = usize::MAX;
        let mut image_end = 0usize;
        let mut i = 0usize;
        while i < elf.segment_count {
            let seg = elf.segments[i];
            if seg.memory_size != 0 {
                image_base = image_base.min(align_down(seg.virtual_address, PAGE_SIZE));
                image_end =
                    image_end.max(align_up(seg.virtual_address + seg.memory_size, PAGE_SIZE));
            }
            i += 1;
        }
        if image_base == usize::MAX || image_end <= image_base {
            return Err(CapabilityError::InvalidArgument);
        }
        let image_pages = (image_end - image_base) / PAGE_SIZE;
        let total_frames = image_pages + USER_STACK_PAGES;
        let stack_top = USER_STACK_BASE + USER_STACK_PAGES * PAGE_SIZE;
        // Native runtime reads argc, argv[0], and envp[0] from the initial stack.
        // Keep these zero words inside the mapped stack even when argc == 0.
        let initial_stack_pointer = stack_top - 32;
        let heap_base = align_up(image_end.max(USER_ANONYMOUS_MAP_BASE), PAGE_SIZE);
        let raw_fault_process = elf.ipc_buffer_start.is_none() && resolver_port.is_some();
        let (has_ipc_buffer, ipc_buffer_va, ipc_buffer_frame_slot, ipc_buffer_tls_base) =
            match elf.ipc_buffer_start {
                Some(va) => {
                    if va < image_base || va >= image_end || (va & (PAGE_SIZE - 1)) != 0 {
                        error!(
                        "[proc.err] invalid __ipc_buffer_start={:#018x} image=[{:#018x}..{:#018x})",
                        va, image_base, image_end
                    );
                        return Err(CapabilityError::InvalidArgument);
                    }
                    let frame_slot = (va - image_base) / PAGE_SIZE;
                    spawn_try!(self.ensure_process_frame_chunks(pid, child_root, frame_slot, 1));
                    (
                        true,
                        va,
                        frame_slot,
                        va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS,
                    )
                }
                None if raw_fault_process => {
                    info!("[proc] raw fault-handler ELF without Nanami IPC buffer");
                    (false, 0, 0, 0)
                }
                None => {
                    error!("[proc.err] missing required symbol __ipc_buffer_start");
                    return Err(CapabilityError::InvalidArgument);
                }
            };

        info!(
            "[proc] lazy map plan image=[{:#018x}..{:#018x}) image_pages={:>3} stack_pages={:>3} ipc={:#018x} temp={:#018x}",
            image_base,
            image_end,
            image_pages,
            USER_STACK_PAGES,
            ipc_buffer_va,
            TEMP_MAP_BASE + pid * TEMP_MAP_STRIDE
        );

        spawn_try!(self.processes.ensure_vm_space_for_pid(pid));
        spawn_try!(self.processes.register_lazy_mapping(
            pid,
            image_base,
            image_pages,
            0,
            ProcessLazyMappingKind::Image {
                image: image_bytes,
                elf,
            },
        ));
        spawn_try!(self.processes.register_lazy_mapping(
            pid,
            USER_STACK_BASE,
            USER_STACK_PAGES,
            image_pages,
            ProcessLazyMappingKind::Zero,
        ));
        info!("[proc] lazy vm tracker ready pid={:>3}", pid);

        let mut image_page = 0usize;
        while image_page < image_pages {
            let image_va = image_base + image_page * PAGE_SIZE;
            spawn_try!(self.materialize_lazy_page(pid, child_root, child_address_space, image_va));
            image_page += 1;
        }
        self.processes.drop_image_lazy_mappings_for_pid(pid);
        let ipc_buffer_frame = if has_ipc_buffer {
            process_frame_descriptor(child_root, ipc_buffer_frame_slot)
        } else {
            0
        };
        let mut stack_page = 0usize;
        while stack_page < USER_STACK_PAGES {
            let stack_va = USER_STACK_BASE + stack_page * PAGE_SIZE;
            spawn_try!(self.materialize_lazy_page(pid, child_root, child_address_space, stack_va));
            stack_page += 1;
        }

        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true,           // address_space
            true,           // root_node
            has_ipc_buffer, // frame_ipc_buffer
            true,           // notification_port
            true,           // ipc_port_resolver
            true,           // instruction_pointer
            true,           // stack_pointer
            has_ipc_buffer, // thread_local_base
            true,           // priority
            false,          // affinity
        );

        info!(
            "[proc] configure pcb={:#018x} root={:#018x} as={:#018x} ip={:#018x} sp={:#018x}",
            child_pcb, child_root, child_address_space, elf.entry_point, initial_stack_pointer
        );
        let priority = priority_override.unwrap_or_else(|| process_priority_for_image(image_name));
        let resolver_port = if let Some(external_resolver) = resolver_port {
            spawn_try!(arch::node::copy(
                child_root,
                PROCESS_SLOT_FAULT_RESOLVER as Word,
                external_resolver,
            ));
            let child_resolver = make_child_slot_descriptor(
                child_root,
                PROCESS_ROOT_RADIX,
                PROCESS_SLOT_FAULT_RESOLVER,
            );
            spawn_try!(arch::ipc_port::identify(child_resolver, pid as Word));
            child_resolver
        } else {
            child_os_port
        };
        spawn_try!(arch::process_control_block::configure(
            child_pcb,
            config,
            child_address_space,
            child_root,
            ipc_buffer_frame,
            child_notification,
            resolver_port,
            elf.entry_point,
            initial_stack_pointer,
            ipc_buffer_tls_base,
            priority,
            0,
        ));

        spawn_try!(self.processes.install_process(
            pid,
            reaper_pid,
            process_root_slot,
            child_root,
            child_pcb,
            child_address_space,
            child_os_port,
            pid as Word,
            total_frames,
            heap_base,
            USER_HEAP_LIMIT,
        ));
        if auto_resume {
            spawn_try!(arch::process_control_block::resume(child_pcb));
        }
        info!(
            "[proc] child {} image={} pid={:>3} priority={:>2} root={:#018x} entry={:#018x}",
            if auto_resume { "resumed" } else { "prepared" },
            image_name,
            pid,
            priority,
            child_root,
            elf.entry_point
        );
        Ok(pid)
    }

    fn spawn_initramfs_image(
        &mut self,
        image_name: &str,
        reaper_pid: usize,
        resolver_port: Option<CapabilityDescriptor>,
        auto_resume: bool,
        priority_override: Option<Word>,
    ) -> Result<usize, CapabilityError> {
        let mut spawned_pid = None;
        cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
            if spawned_pid.is_some() || !initramfs_image_is_auto_spawn_candidate(entry.name) {
                return Ok(());
            }
            if initramfs_image_name_matches(entry.name, image_name) {
                spawned_pid = Some(self.spawn_process_from_elf(
                    entry.name,
                    entry.data,
                    reaper_pid,
                    resolver_port,
                    auto_resume,
                    priority_override,
                )?);
            }
            Ok(())
        })?;
        spawned_pid.ok_or(CapabilityError::InvalidArgument)
    }

    fn cleanup_failed_spawn(&mut self, pid: usize, root_slot: usize) {
        let _ = arch::node::revoke(self.root.root_descriptor, root_slot as Word);
        let _ = arch::node::remove(self.root.root_descriptor, root_slot as Word);

        let physical_allocations = self.processes.releasable_physical_allocations_for_pid(pid);
        for allocation in physical_allocations.iter() {
            let _ = self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            );
        }
        self.free_deferred_process_allocations(pid, None);

        if self.processes.find_entry_by_pid(pid).is_some() {
            let _ = self.processes.mark_exited(pid, 0, 1);
            let _ = self.processes.reap_process(pid, true);
        } else {
            self.processes.discard_process_artifacts(pid, root_slot);
        }
        let _ = self.memory.reset_process_arena(root_slot);
    }

    fn free_deferred_process_allocations(
        &mut self,
        pid: usize,
        process_root: Option<CapabilityDescriptor>,
    ) {
        let allocations = self
            .processes
            .releasable_deferred_physical_allocations_for_pid(pid);
        for allocation in allocations.iter() {
            if let Some(root_node) = process_root {
                let mut i = 0usize;
                while i < allocation.page_count {
                    let slot = allocation.start_slot + i;
                    let _ = arch::node::remove(
                        process_frame_chunk_descriptor(root_node, slot / PROCESS_FRAME_CHUNK_PAGES),
                        (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    );
                    i += 1;
                }
            }
            let _ = self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            );
        }
        self.processes
            .drop_deferred_physical_allocations_for_pid(pid);
    }

    fn try_handle_demand_page_fault(
        &mut self,
        fault: KernelFaultEvent,
    ) -> Result<(), CapabilityError> {
        if fault.identifier == 0 || fault.architecture_fault_code & 1 != 0 {
            return Err(CapabilityError::IllegalOperation);
        }

        let page_va = align_down(fault.fault_address, PAGE_SIZE);
        let entry = self
            .processes
            .find_entry_by_pid(fault.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        let _ = self.materialize_lazy_page(
            fault.identifier,
            entry.root_node,
            entry.address_space,
            page_va,
        )?;
        debug!(
            "[fault] demand page pid={:>3} va={:#018x}",
            fault.identifier, page_va
        );
        Ok(())
    }

    fn materialize_lazy_page(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        page_va: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        if let Some(frame) = self
            .processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(page_va))
        {
            return Ok(frame);
        }
        let mapping = self
            .processes
            .find_lazy_mapping(pid, page_va)
            .ok_or(CapabilityError::InvalidArgument)?;
        let page_offset = (page_va - mapping.base_va) / PAGE_SIZE;
        let frame_slot = mapping.start_slot + page_offset;
        let allocated = self.allocate_process_frames(pid, process_root, frame_slot, 1)?;
        let base_page = allocated
            .first()
            .map(|(_, page)| *page)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.processes
            .register_physical_allocation(pid, page_va, frame_slot, base_page, 1)?;

        let frame = process_frame_descriptor(process_root, frame_slot);
        {
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory.map_frame(address_space, frame, page_va, vm)?;
        }

        let temp_va = TEMP_MAP_BASE + pid * TEMP_MAP_STRIDE + frame_slot * PAGE_SIZE;
        self.map_alpha_temporary_frame(frame, temp_va)?;

        fill_lazy_page(mapping.kind, page_va, temp_va)?;
        if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
            error!(
                "[proc.err] unmap lazy temp pid={:>3} va={:#018x} temp={:#018x} frame={:#018x} err={:?}",
                pid, page_va, temp_va, frame, e
            );
            return Err(e);
        }
        Ok(frame)
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
        debug!(
            "[ipc] notification received: id={:>3} value={:#018x}",
            notification.identifier, notification.value
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
        if request.code == OS_REQUEST_DEBUG_PING {
            info!(
                "[ipc] ping from pid={:>3} token={:#018x}",
                request.identifier, request.arg0
            );
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, OS_RESPONSE_OK, request.arg0, OS_RESPONSE_PONG_MAGIC
            );
            return (OS_RESPONSE_OK, request.arg0, OS_RESPONSE_PONG_MAGIC);
        }
        if request.code == OS_REQUEST_PAGE_ALLOC {
            let result = self.handle_page_alloc_request(request);
            let (status, detail0) = match result {
                Ok(base) => (OS_RESPONSE_OK, base),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_HEAP_ALLOC {
            let result = self.handle_heap_alloc_request(request);
            let (status, detail0, detail1) = match result {
                Ok((base, size)) => (OS_RESPONSE_OK, base, size),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_SELF_PID {
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, OS_RESPONSE_OK, request.identifier, 0usize
            );
            return (OS_RESPONSE_OK, request.identifier, 0);
        }
        if request.code == OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION {
            let result = self.handle_initial_framebuffer_information_request(request);
            let (status, detail0, detail1) = match result {
                Ok((d0, d1)) => (OS_RESPONSE_OK, d0, d1),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_EXIT {
            let result = self.handle_exit_request(request);
            let (status, detail0) = map_request_result_to_status(result);
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_DMA_REQUEST {
            let result = self.handle_dma_request(request);
            let (status, detail0, detail1) = match result {
                Ok((paddr, vaddr)) => (OS_RESPONSE_OK, paddr, vaddr),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_MMIO_REQUEST {
            let result = self.handle_mmio_request(request);
            let (status, detail0, detail1) = match result {
                Ok((paddr, vaddr)) => (OS_RESPONSE_OK, paddr, vaddr),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_SHARED_MEMORY_CREATE {
            let result = self.handle_shared_memory_request(request);
            let (status, detail0, detail1) = match result {
                Ok((local_vaddr, peer_vaddr)) => (OS_RESPONSE_OK, local_vaddr, peer_vaddr),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_MAPPING_RELEASE {
            let result = self.handle_mapping_release_request(request);
            let (status, detail0) = map_request_result_to_status(result);
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_SHARED_FRAMEBUFFER_CREATE {
            let result = self.handle_shared_framebuffer_request(request);
            let (status, detail0, detail1) = match result {
                Ok((local_vaddr, peer_vaddr)) => (OS_RESPONSE_OK, local_vaddr, peer_vaddr),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_SERVICE_CONNECT {
            let result = self.handle_service_connect_request(request);
            let (status, detail0) = match result {
                Ok(service_pid) => (OS_RESPONSE_OK, service_pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_SERVICE_REGISTER {
            let result = self.handle_service_register_request(request);
            let (status, detail0) = match result {
                Ok(registered_pid) => (OS_RESPONSE_OK, registered_pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_SERVICE_LIST {
            let (status, detail0, detail1) = match self.handle_service_list_request(request) {
                Some((owner_pid, service_kind)) => (OS_RESPONSE_OK, owner_pid, service_kind),
                None => (OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN {
            let result = self.handle_process_spawn_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN_MEMORY {
            let result = self.handle_process_spawn_memory_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN_MEMORY_FAULT_HANDLER_SUSPENDED {
            let result = self.handle_process_spawn_memory_fault_handler_suspended_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN_MEMORY_SUSPENDED {
            let result = self.handle_process_spawn_memory_suspended_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_EXEC_MEMORY {
            let result = self.handle_process_exec_memory_request(request);
            let (status, detail0) = match result {
                Ok(entry_point) => (OS_RESPONSE_OK, entry_point),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER {
            let result = self.handle_process_spawn_fault_handler_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_SPAWN_FAULT_HANDLER_SUSPENDED {
            let result = self.handle_process_spawn_fault_handler_suspended_request(request);
            let (status, detail0) = match result {
                Ok(pid) => (OS_RESPONSE_OK, pid),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_STATUS {
            let result = self.handle_process_status_request(request);
            let (status, detail0, detail1) = match result {
                Ok((exited, exit_code)) => (OS_RESPONSE_OK, exited, exit_code),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_PROCESS_ALIVE {
            let result = self.handle_process_alive_request(request);
            let (status, detail0) = match result {
                Ok(alive) => (OS_RESPONSE_OK, alive as usize),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_REAP {
            let result = self.handle_process_reap_request(request);
            let (status, detail0) = match result {
                Ok(()) => (OS_RESPONSE_OK, 0),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_KILL {
            let result = self.handle_process_kill_request(request);
            let (status, detail0) = match result {
                Ok(()) => (OS_RESPONSE_OK, 0),
                Err(e) => map_request_result_to_status(Err(e)),
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_MEMORY_READ {
            let result = self.handle_process_memory_copy_request(request, false);
            let (status, detail0) = map_request_result_to_status(result);
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_MEMORY_WRITE {
            let result = self.handle_process_memory_copy_request(request, true);
            let (status, detail0) = map_request_result_to_status(result);
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_MEMORY_CLONE {
            let result = self.handle_process_memory_clone_request(request);
            let (status, detail0) = map_request_result_to_status(result);
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_MEMORY_COPY_WITHIN {
            let result = self.handle_process_memory_copy_within_request(request);
            let (status, detail0) = map_request_result_to_status(result);
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_PROCESS_MAP_ANONYMOUS {
            let result = self.handle_process_map_anonymous_request(request);
            let (status, detail0, detail1) = match result {
                Ok((base, mapped)) => (OS_RESPONSE_OK, base, mapped),
                Err(e) => {
                    let (s, d0) = map_request_result_to_status(Err(e));
                    (s, d0, 0)
                }
            };
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, detail1
            );
            return (status, detail0, detail1);
        }
        if request.code == OS_REQUEST_NANAMI_CONTROL {
            let result = self.handle_nanami_control_request(request);
            let (status, detail0) = map_request_result_to_status(result);
            debug!(
                "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
                request.identifier, status, detail0, 0usize
            );
            return (status, detail0, 0);
        }
        if request.code == OS_REQUEST_NANAMI_INFO {
            return match self.handle_nanami_info_request(request) {
                Ok((detail0, detail1)) => (OS_RESPONSE_OK, detail0, detail1),
                Err(error) => {
                    let (status, detail0) = map_request_result_to_status(Err(error));
                    (status, detail0, 0)
                }
            };
        }

        let result = match request.code {
            OS_REQUEST_IRQ_CONTROL => self.handle_irq_control_request(request),
            OS_REQUEST_IO_PORT_CONTROL => self.handle_io_port_control_request(request),
            OS_REQUEST_NOTIFICATION_PORT_CREATE => {
                self.handle_notification_port_create_request(request)
            }
            OS_REQUEST_NOTIFICATION_PORT_COPY => {
                self.handle_notification_port_copy_request(request)
            }
            _ => {
                warn!(
                    "[ipc.warn] unknown request code={:#018x} id={:>3}",
                    request.code, request.identifier
                );
                Err(CapabilityError::InvalidArgument)
            }
        };

        let (status, detail0) = map_request_result_to_status(result);
        debug!(
            "[ipc] rsp id={:>3} status={:#018x} detail0={:#018x} detail1={:#018x}",
            request.identifier, status, detail0, 0usize
        );
        (status, detail0, 0)
    }

    fn handle_nanami_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let property = decode_control_text(request.arg0, request.arg1);
        let value = decode_control_text(request.arg2, request.arg3);
        if bytes_equal(&property, b"os.log") {
            if bytes_equal(&value, b"enable") {
                crate::nanami_utils::log::set_info_enabled(true);
                info!("[control] os.log enabled");
                return Ok(());
            }
            if bytes_equal(&value, b"disable") {
                info!("[control] os.log disabled");
                crate::nanami_utils::log::set_info_enabled(false);
                return Ok(());
            }
        }
        Err(CapabilityError::InvalidArgument)
    }

    fn handle_nanami_info_request(
        &self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        match request.arg0 {
            NANAMI_INFO_MEMORY => {
                let info = self.memory.physical_memory_info()?;
                Ok((
                    info.total_pages.saturating_mul(PAGE_SIZE),
                    info.free_pages.saturating_mul(PAGE_SIZE),
                ))
            }
            NANAMI_INFO_PROCESS => {
                let info = self.processes.statistics();
                Ok((info.running, info.exited))
            }
            _ => Err(CapabilityError::InvalidArgument),
        }
    }

    fn handle_irq_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let irq_number = request.arg0;
        let notification_slot = request.arg1;
        let interrupt_slot = request.arg2;

        validate_process_device_slot(notification_slot)?;
        validate_process_device_slot(interrupt_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let requested_notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            notification_slot,
        );
        let default_notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_NOTIFICATION_PORT,
        );

        self.processes.assign_irq_to_pid(pid, irq_number)?;

        if notification_slot != PROCESS_SLOT_NOTIFICATION_PORT as Word {
            match arch::node::copy(
                process_entry.root_node,
                notification_slot,
                default_notification_descriptor,
            ) {
                Ok(()) => {}
                Err(_) => {
                    // Reusing the same notification slot for multiple IRQ registrations
                    // is valid. The user-visible slot must keep a stable notification object;
                    // IRQ-specific identifiers are assigned only to the per-IRQ alias below.
                }
            }
        }

        match arch::interrupt_region::make_port(
            self.interrupt_region,
            irq_number,
            process_entry.root_node,
            interrupt_slot,
        ) {
            Ok(()) => {}
            Err(e) => {
                let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
                return Err(e);
            }
        }

        let interrupt_descriptor =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, interrupt_slot);

        let irq_identifier = irq_notification_identifier(irq_number)?;
        let alias_slot =
            select_irq_notification_alias_slot(irq_number, notification_slot, interrupt_slot);
        let alias_descriptor =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, alias_slot);

        if alias_slot == notification_slot || alias_slot == interrupt_slot {
            let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
            return Err(CapabilityError::InvalidArgument);
        }

        // Notification identifiers are slot-local. Binding multiple IRQs through the same
        // process-visible slot would overwrite that slot's identifier on every registration.
        // Bind each interrupt through an alias slot that points at the same notification object,
        // while userland continues to wait on `notification_slot`.
        arch::node::copy(
            process_entry.root_node,
            alias_slot,
            requested_notification_descriptor,
        )?;
        let _ = arch::notification_port::identify(alias_descriptor, irq_identifier);
        if let Err(e) = arch::interrupt_port::bind(interrupt_descriptor, alias_descriptor) {
            let _ = self.processes.unassign_irq_from_pid(pid, irq_number);
            return Err(e);
        }

        info!(
            "[irq] granted pid={:>3} irq={:>3} notification_slot={:>3} alias_slot={:>3} interrupt_slot={:>3}",
            pid, irq_number, notification_slot, alias_slot, interrupt_slot
        );

        Ok(())
    }

    fn handle_notification_port_create_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let notification_slot = request.arg0;
        let identifier = request.arg1;
        validate_process_device_slot(notification_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        arch::generic::convert(
            self.process_arena_for_root(process_entry.root_node)?,
            CapabilityType::NotificationPort,
            0,
            1,
            process_entry.root_node,
            notification_slot,
        )?;

        let notification_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            notification_slot,
        );
        let _ = arch::notification_port::identify(notification_descriptor, identifier);

        Ok(())
    }

    fn handle_notification_port_copy_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        if caller_pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let source_pid = request.arg0;
        let source_notification_slot = request.arg1;
        let destination_slot = request.arg2;
        let identifier = request.arg3;
        validate_process_device_slot(source_notification_slot)?;
        validate_process_device_slot(destination_slot)?;

        let caller_entry = self
            .processes
            .find_entry_by_pid(caller_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        let source_entry = self
            .processes
            .find_entry_by_pid(source_pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let source_notification_descriptor = make_child_slot_descriptor(
            source_entry.root_node,
            PROCESS_ROOT_RADIX,
            source_notification_slot,
        );

        let _ = arch::node::remove(caller_entry.root_node, destination_slot as Word);
        arch::node::copy(
            caller_entry.root_node,
            destination_slot,
            source_notification_descriptor,
        )?;

        let destination_descriptor = make_child_slot_descriptor(
            caller_entry.root_node,
            PROCESS_ROOT_RADIX,
            destination_slot,
        );
        let _ = arch::notification_port::identify(destination_descriptor, identifier);

        Ok(())
    }

    fn handle_service_connect_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let destination_slot = request.arg0;
        validate_process_device_slot(destination_slot)?;
        let (raw_name, raw_len) = decode_service_name_24(request.arg1, request.arg2, request.arg3)
            .ok_or(CapabilityError::InvalidArgument)?;
        let service_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let (service_port, service_pid) = self
            .communication
            .resolve_service_with_owner(service_name)
            .ok_or(CapabilityError::InvalidArgument)?;

        arch::node::copy(process_entry.root_node, destination_slot, service_port)?;
        let destination_descriptor = make_child_slot_descriptor(
            process_entry.root_node,
            PROCESS_ROOT_RADIX,
            destination_slot,
        );
        arch::ipc_port::identify(destination_descriptor, pid as Word)?;

        info!(
            "[svc] connect name={} pid={:>3} dst_slot={:>3} src_port={:#018x}",
            service_name, pid, destination_slot, service_port
        );

        Ok(service_pid)
    }

    fn handle_exit_request(&mut self, request: OsRequestEvent) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if process_entry.pcb == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }

        arch::process_control_block::suspend(process_entry.pcb)?;
        let is_ok = request.arg0;
        let error_value = request.arg1;
        self.processes.mark_exited(pid, is_ok, error_value)?;
        info!(
            "[proc] exited pid={:>3} pcb={:#018x} is_ok={} error={:#018x}",
            pid, process_entry.pcb, is_ok, error_value
        );
        Ok(())
    }

    fn handle_process_spawn_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let image_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let pid = self.spawn_initramfs_image(image_name, request.identifier, None, true, None)?;
        info!(
            "[proc] spawned by request caller={:>3} image={} pid={:>3}",
            request.identifier, image_name, pid
        );
        Ok(pid)
    }

    fn handle_process_spawn_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || self
                .processes
                .find_entry_by_pid(request.identifier)
                .is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-memory.elf",
            image_bytes,
            request.identifier,
            None,
            true,
            Some(priority),
        )?;
        info!(
            "[proc] spawned from memory caller={:>3} pid={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, size_bytes, priority
        );
        Ok(pid)
    }

    fn handle_process_spawn_memory_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        let destination_slot = request.arg3;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || destination_slot == 0
            || destination_slot >= (1 << PROCESS_ROOT_RADIX)
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-memory.elf",
            image_bytes,
            request.identifier,
            None,
            false,
            Some(priority),
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned memory suspended caller={:>3} pid={:>3} pcb_slot={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, destination_slot, size_bytes, priority
        );
        Ok(pid)
    }

    fn handle_process_spawn_memory_fault_handler_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let image_vaddr = request.arg0;
        let size_bytes = request.arg1;
        let priority = request.arg2;
        let destination_slot = request.arg3;
        if image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
            || destination_slot == 0
            || destination_slot >= (1 << PROCESS_ROOT_RADIX)
        {
            error!(
                "[proc.err] memory fault spawn invalid args caller={:>3} image={:#018x} bytes={:#x} priority={:>2} dst_slot={:>3}",
                request.identifier, image_vaddr, size_bytes, priority, destination_slot
            );
            return Err(CapabilityError::InvalidArgument);
        }

        let resolver_port = make_child_slot_descriptor(
            caller.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_SERVICE_PORT,
        );
        let image_bytes = self.read_process_memory_into_static_buffer(
            request.identifier,
            image_vaddr,
            size_bytes,
        )?;
        let pid = self.spawn_process_from_elf(
            "rootfs-linux.elf",
            image_bytes,
            request.identifier,
            Some(resolver_port),
            false,
            Some(priority),
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            error!(
                "[proc.err] memory child pcb copy failed caller={:>3} pid={:>3} dst_slot={:>3} pcb={:#018x} err={:?}",
                request.identifier, pid, destination_slot, child.pcb, error
            );
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned memory with fault-handler caller={:>3} pid={:>3} pcb_slot={:>3} bytes={:#x} priority={:>2}",
            request.identifier, pid, destination_slot, size_bytes, priority
        );
        Ok(pid)
    }

    fn handle_process_exec_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let image_vaddr = request.arg1;
        let size_bytes = request.arg2;
        let priority = request.arg3;
        if caller_pid == 0
            || target_pid == 0
            || image_vaddr == 0
            || size_bytes == 0
            || size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES
        {
            return Err(CapabilityError::InvalidArgument);
        }
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        if target.pcb == 0 || target.root_node == 0 || target.address_space == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }

        let image_bytes =
            self.read_process_memory_into_static_buffer(caller_pid, image_vaddr, size_bytes)?;
        self.replace_process_image_from_memory(target_pid, target, image_bytes, priority)
    }

    fn replace_process_image_from_memory(
        &mut self,
        pid: usize,
        target: crate::nanami_core::process::ProcessEntry,
        image_bytes: &'static [u8],
        _priority: Word,
    ) -> Result<usize, CapabilityError> {
        let elf = parse_elf64(image_bytes)?;
        let mut image_base = usize::MAX;
        let mut image_end = 0usize;
        let mut i = 0usize;
        while i < elf.segment_count {
            let seg = elf.segments[i];
            if seg.memory_size != 0 {
                image_base = image_base.min(align_down(seg.virtual_address, PAGE_SIZE));
                image_end =
                    image_end.max(align_up(seg.virtual_address + seg.memory_size, PAGE_SIZE));
            }
            i += 1;
        }
        if image_base == usize::MAX || image_end <= image_base {
            return Err(CapabilityError::InvalidArgument);
        }

        let image_pages = (image_end - image_base) / PAGE_SIZE;
        let total_frames = image_pages + USER_STACK_PAGES;
        let heap_base = align_up(image_end.max(USER_ANONYMOUS_MAP_BASE), PAGE_SIZE);

        self.drop_process_runtime_mappings(pid, target)?;
        self.recreate_process_address_space_for_exec(target)?;
        self.processes.reset_runtime_memory_for_exec(
            pid,
            total_frames,
            heap_base,
            USER_HEAP_LIMIT,
        )?;
        self.processes.ensure_vm_space_for_pid(pid)?;
        self.processes.register_lazy_mapping(
            pid,
            USER_STACK_BASE,
            USER_STACK_PAGES,
            image_pages,
            ProcessLazyMappingKind::Zero,
        )?;

        let image_kind = ProcessLazyMappingKind::Image {
            image: image_bytes,
            elf,
        };
        let mut image_page = 0usize;
        while image_page < image_pages {
            let image_va = image_base + image_page * PAGE_SIZE;
            self.materialize_exec_image_page(
                pid,
                target.root_node,
                target.address_space,
                image_kind,
                image_va,
                image_page,
            )?;
            image_page += 1;
        }
        let mut stack_page = 0usize;
        while stack_page < USER_STACK_PAGES {
            let stack_va = USER_STACK_BASE + stack_page * PAGE_SIZE;
            self.materialize_lazy_page(pid, target.root_node, target.address_space, stack_va)?;
            stack_page += 1;
        }
        info!(
            "[proc] exec memory pid={:>3} image=[{:#018x}..{:#018x}) pages={} entry={:#018x}",
            pid, image_base, image_end, image_pages, elf.entry_point
        );
        Ok(elf.entry_point)
    }

    fn materialize_exec_image_page(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        address_space: CapabilityDescriptor,
        kind: ProcessLazyMappingKind,
        page_va: usize,
        page_index: usize,
    ) -> Result<(), CapabilityError> {
        let frame_slot = page_index;
        let allocated = self.allocate_process_frames(pid, process_root, frame_slot, 1)?;
        let base_page = allocated
            .first()
            .map(|(_, page)| *page)
            .ok_or(CapabilityError::InvalidArgument)?;
        self.processes
            .register_physical_allocation(pid, page_va, frame_slot, base_page, 1)?;

        let frame = process_frame_descriptor(process_root, frame_slot);
        {
            let vm = self
                .processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            self.memory
                .map_frame_strict(address_space, frame, page_va, vm)?;
        }

        let temp_va = PROCESS_ZERO_TEMP_BASE + page_index * PAGE_SIZE;
        self.map_alpha_temporary_frame(frame, temp_va)?;
        fill_lazy_page(kind, page_va, temp_va)?;
        if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
            error!(
                "[proc.exec.err] unmap image temp pid={:>3} va={:#018x} temp={:#018x} frame={:#018x} err={:?}",
                pid, page_va, temp_va, frame, e
            );
            return Err(e);
        }
        Ok(())
    }

    fn recreate_process_address_space_for_exec(
        &mut self,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        arch::node::remove(target.root_node, PROCESS_SLOT_ADDRESS_SPACE as Word)?;
        arch::generic::convert(
            self.process_arena_for_root(target.root_node)?,
            CapabilityType::AddressSpace,
            0,
            1,
            target.root_node,
            PROCESS_SLOT_ADDRESS_SPACE as Word,
        )?;
        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true, false, false, false, false, false, false, false, false, false,
        );
        arch::process_control_block::configure(
            target.pcb,
            config,
            target.address_space,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        )
    }

    fn drop_process_runtime_mappings(
        &mut self,
        pid: usize,
        target: crate::nanami_core::process::ProcessEntry,
    ) -> Result<(), CapabilityError> {
        let allocations =
            self.processes
                .reset_runtime_memory_for_exec(pid, 0, 0, USER_HEAP_LIMIT)?;
        for (allocation, is_last_reference) in allocations {
            let mut i = 0usize;
            while i < allocation.page_count {
                let slot = allocation.start_slot + i;
                let frame = process_frame_descriptor(target.root_node, slot);
                let va = allocation.base_va + i * PAGE_SIZE;
                if let Err(e) = arch::address_space::unmap(target.address_space, frame, va) {
                    info!(
                        "[proc.exec.warn] unmap failed pid={:>3} va={:#018x} frame={:#018x} err={:?}",
                        pid, va, frame, e
                    );
                }
                if let Err(e) = arch::node::remove(
                    process_frame_chunk_descriptor(
                        target.root_node,
                        slot / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    (slot % PROCESS_FRAME_CHUNK_PAGES) as Word,
                ) {
                    info!(
                        "[proc.exec.warn] frame cap remove failed pid={:>3} frame={:#018x} slot={} err={:?}",
                        pid, frame, slot, e
                    );
                }
                i += 1;
            }
            if is_last_reference {
                self.memory.free_physical(
                    allocation.base_page * PAGE_SIZE,
                    allocation.page_count * PAGE_SIZE,
                )?;
            }
        }
        self.free_deferred_process_allocations(pid, Some(target.root_node));
        Ok(())
    }

    fn handle_process_spawn_fault_handler_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        self.handle_process_spawn_fault_handler_request_with_resume(request, true)
    }

    fn handle_process_spawn_fault_handler_suspended_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        self.handle_process_spawn_fault_handler_request_with_resume(request, false)
    }

    fn handle_process_spawn_fault_handler_request_with_resume(
        &mut self,
        request: OsRequestEvent,
        auto_resume: bool,
    ) -> Result<usize, CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let caller = self
            .processes
            .find_entry_by_pid(request.identifier)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller.root_node == 0 {
            return Err(CapabilityError::InvalidDescriptor);
        }
        let destination_slot = request.arg3;
        if destination_slot == 0 || destination_slot >= (1 << PROCESS_ROOT_RADIX) {
            return Err(CapabilityError::InvalidArgument);
        }

        let resolver_port = make_child_slot_descriptor(
            caller.root_node,
            PROCESS_ROOT_RADIX,
            PROCESS_SLOT_SERVICE_PORT,
        );
        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let image_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;

        let pid = self.spawn_initramfs_image(
            image_name,
            request.identifier,
            Some(resolver_port),
            auto_resume,
            None,
        )?;
        let child = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::Fatal)?;
        let _ = arch::node::remove(caller.root_node, destination_slot as Word);
        if let Err(error) = arch::node::copy(caller.root_node, destination_slot as Word, child.pcb)
        {
            error!(
                "[proc.err] spawned child pcb copy failed caller={:>3} image={} pid={:>3} dst_slot={:>3} pcb={:#018x} err={:?}",
                request.identifier, image_name, pid, destination_slot, child.pcb, error
            );
            self.cleanup_failed_spawn(pid, child.root_slot);
            return Err(error);
        }
        info!(
            "[proc] spawned with fault-handler caller={:>3} image={} pid={:>3} pcb_slot={:>3} suspended={}",
            request.identifier, image_name, pid, destination_slot, !auto_resume
        );
        Ok(pid)
    }

    fn handle_process_status_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        Ok((entry.exited as usize, entry.exit_code))
    }

    fn handle_process_alive_request(
        &self,
        request: OsRequestEvent,
    ) -> Result<bool, CapabilityError> {
        if request.identifier == 0 || request.arg0 == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(self
            .processes
            .find_entry_by_pid(request.arg0)
            .map(|entry| !entry.exited)
            .unwrap_or(false))
    }

    fn handle_process_reap_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        if !entry.exited {
            return Err(CapabilityError::IllegalOperation);
        }
        arch::node::revoke(self.root.root_descriptor, entry.root_slot as Word)?;
        arch::node::remove(self.root.root_descriptor, entry.root_slot as Word)?;
        let physical_allocations = self.processes.releasable_physical_allocations_for_pid(pid);
        for allocation in physical_allocations.iter() {
            self.memory.free_physical(
                allocation.base_page * PAGE_SIZE,
                allocation.page_count * PAGE_SIZE,
            )?;
        }
        self.free_deferred_process_allocations(pid, None);
        self.processes.drop_physical_allocations_for_pid(pid);
        self.memory.reset_process_arena(entry.root_slot)?;
        self.processes.reap_process(pid, true)?;
        info!(
            "[proc] reaped pid={:>3} root_slot={:>3}",
            pid, entry.root_slot
        );
        Ok(())
    }

    fn handle_process_kill_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        if request.identifier == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let pid = request.arg0;
        if pid == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        validate_process_observer(request.identifier, entry.pid, entry.reaper_pid)?;
        if entry.exited {
            return Ok(());
        }
        arch::process_control_block::suspend(entry.pcb)?;
        self.processes.mark_exited(pid, 0, request.arg1)?;
        info!(
            "[proc] killed pid={:>3} pcb={:#018x} signal={:#018x}",
            pid, entry.pcb, request.arg1
        );
        Ok(())
    }

    fn handle_page_alloc_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let page_count = request.arg0;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let heap_base = self.map_process_heap_pages(pid, page_count)?;

        info!(
            "[mem] granted pid={:>3} pages={:>4} va=[{:#018x}..{:#018x})",
            pid,
            page_count,
            heap_base,
            heap_base + page_count * PAGE_SIZE,
        );
        Ok(heap_base)
    }

    fn handle_heap_alloc_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let requested_size = request.arg0;
        if requested_size == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(requested_size, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let heap_base = self.map_process_heap_pages(pid, page_count)?;
        let guard_base =
            self.processes
                .reserve_process_virtual_gap(pid, USER_HEAP_GUARD_PAGES, PAGE_SIZE)?;

        info!(
            "[heap] granted pid={:>3} bytes={:#x} mapped={:#x} va=[{:#018x}..{:#018x}) guard=[{:#018x}..{:#018x})",
            pid,
            requested_size,
            mapped_size,
            heap_base,
            heap_base + mapped_size,
            guard_base,
            guard_base + USER_HEAP_GUARD_PAGES * PAGE_SIZE,
        );
        Ok((heap_base, mapped_size))
    }

    fn map_process_heap_pages(
        &mut self,
        pid: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        if pid == 0 || page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let (root_node, address_space, heap_base, start_slot) = self
            .processes
            .reserve_process_heap(pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)?;
        let allocated_frames =
            self.allocate_process_frames(pid, root_node, start_slot, page_count)?;
        for (slot, page) in allocated_frames {
            let va = heap_base + (slot - start_slot) * PAGE_SIZE;
            self.processes
                .register_physical_allocation(pid, va, slot, page, 1)?;
        }
        self.zero_process_frames(root_node, start_slot, page_count)?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + i);
            let va = heap_base + i * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(address_space, frame, va, vm)?;
            i += 1;
        }

        Ok(heap_base)
    }

    fn map_process_heap_pages_at(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Result<usize, CapabilityError> {
        if pid == 0 || base_va == 0 || page_count == 0 || (base_va & (PAGE_SIZE - 1)) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let (root_node, address_space, start_slot) = self.processes.reserve_process_heap_at(
            pid,
            base_va,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        let reuse_allocation = self
            .processes
            .take_deferred_physical_allocation(pid, base_va, page_count);
        let allocated_frames = if let Some(allocation) = reuse_allocation {
            self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
            let mut frames = Vec::new();
            let mut i = 0usize;
            while i < page_count {
                let frame_index = allocation.base_page + i;
                self.memory
                    .ensure_alpha_frame_at_physical_index(frame_index)?;
                let source_frame = self
                    .memory
                    .physical_frame_descriptor_from_index(frame_index)
                    .ok_or(CapabilityError::InvalidArgument)?;
                arch::node::copy(
                    process_frame_chunk_descriptor(
                        root_node,
                        (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                    ),
                    ((start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                    source_frame,
                )?;
                frames.push((start_slot + i, frame_index));
                i += 1;
            }
            frames
        } else {
            self.allocate_process_frames(pid, root_node, start_slot, page_count)?
        };
        for (slot, page) in allocated_frames {
            let va = base_va + (slot - start_slot) * PAGE_SIZE;
            self.processes
                .register_physical_allocation(pid, va, slot, page, 1)?;
        }
        self.zero_process_frames(root_node, start_slot, page_count)?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + i);
            let va = base_va + i * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame_strict(address_space, frame, va, vm)?;
            i += 1;
        }

        Ok(base_va)
    }

    fn zero_process_frames(
        &mut self,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        let mut i = 0usize;
        while i < page_count {
            let frame = process_frame_descriptor(process_root, start_slot + i);
            let temp_va = PROCESS_ZERO_TEMP_BASE + i * PAGE_SIZE;
            self.map_alpha_temporary_frame(frame, temp_va)?;
            unsafe {
                ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
            }
            if let Err(e) = self.unmap_alpha_temporary_frame(frame, temp_va) {
                error!(
                    "[proc.err] unmap zero temp frame={:#018x} temp={:#018x} err={:?}",
                    frame, temp_va, e
                );
                return Err(e);
            }
            i += 1;
        }
        Ok(())
    }

    fn handle_dma_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let size_bytes = request.arg0;
        if size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let (root_node, address_space, base_va, start_slot) = self.processes.reserve_process_heap(
            pid,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        let base_page = self.memory.allocate_physical_any(mapped_size)?;
        let base_paddr = base_page * PAGE_SIZE;
        self.processes
            .register_physical_allocation(pid, base_va, start_slot, base_page, page_count)?;

        self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_page + i;
            self.memory.copy_alpha_frame_to_process_node(
                frame_index,
                process_frame_chunk_descriptor(
                    root_node,
                    (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                PROCESS_FRAME_NODE_RADIX,
                (start_slot + i) % PROCESS_FRAME_CHUNK_PAGES,
            )?;
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + j);
            let va = base_va + j * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(address_space, frame, va, vm)?;
            j += 1;
        }

        info!(
            "[dma] granted pid={:>3} size={:#x} paddr={:#018x} vaddr={:#018x}",
            pid, mapped_size, base_paddr, base_va
        );
        Ok((base_paddr, base_va))
    }

    fn handle_initial_framebuffer_information_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        match request.arg0 {
            FRAMEBUFFER_INFORMATION_REGION => Ok((
                self.initial_framebuffer.address,
                self.initial_framebuffer.size_bytes,
            )),
            FRAMEBUFFER_INFORMATION_GEOMETRY => Ok((
                self.initial_framebuffer.width,
                self.initial_framebuffer.height,
            )),
            FRAMEBUFFER_INFORMATION_FORMAT => Ok((
                self.initial_framebuffer.stride,
                self.initial_framebuffer.bits_per_pixel,
            )),
            FRAMEBUFFER_INFORMATION_COLOR_AND_ID => Ok((
                self.initial_framebuffer.display_id,
                pack_framebuffer_color_information(
                    self.initial_framebuffer.red_position,
                    self.initial_framebuffer.red_size,
                    self.initial_framebuffer.green_position,
                    self.initial_framebuffer.green_size,
                    self.initial_framebuffer.blue_position,
                    self.initial_framebuffer.blue_size,
                ),
            )),
            _ => Err(CapabilityError::InvalidArgument),
        }
    }

    fn handle_shared_memory_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let peer_pid = request.arg0;
        let size_bytes = request.arg1;
        if peer_pid == 0 || peer_pid == pid || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        if self.processes.find_entry_by_pid(pid).is_none()
            || self.processes.find_entry_by_pid(peer_pid).is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let (caller_root, caller_as, caller_va, caller_start_slot) = self
            .processes
            .reserve_process_heap(pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)?;
        let (peer_root, peer_as, peer_va, peer_start_slot) = self.processes.reserve_process_heap(
            peer_pid,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;

        let base_page = self.memory.allocate_physical_any(mapped_size)?;
        let base_paddr = base_page * PAGE_SIZE;
        self.processes.register_physical_allocation(
            pid,
            caller_va,
            caller_start_slot,
            base_page,
            page_count,
        )?;
        self.processes.register_physical_allocation(
            peer_pid,
            peer_va,
            peer_start_slot,
            base_page,
            page_count,
        )?;

        self.ensure_process_frame_chunks(pid, caller_root, caller_start_slot, page_count)?;
        self.ensure_process_frame_chunks(peer_pid, peer_root, peer_start_slot, page_count)?;

        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_page + i;
            // Convert Generic->Frame only once per physical frame, then fan-out copy to both processes.
            // Calling ensure twice for the same frame causes kernel-side "out of memory" noise
            // because each 4KiB generic is single-shot allocatable.
            self.memory
                .ensure_alpha_frame_at_physical_index(frame_index)?;
            let source_frame = self
                .memory
                .physical_frame_descriptor_from_index(frame_index)
                .ok_or(CapabilityError::InvalidArgument)?;
            arch::node::copy(
                process_frame_chunk_descriptor(
                    caller_root,
                    (caller_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                ((caller_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                source_frame,
            )?;
            arch::node::copy(
                process_frame_chunk_descriptor(
                    peer_root,
                    (peer_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                ((peer_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                source_frame,
            )?;
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let caller_frame = process_frame_descriptor(caller_root, caller_start_slot + j);
            let caller_page_va = caller_va + j * PAGE_SIZE;
            let caller_vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(caller_as, caller_frame, caller_page_va, caller_vm)?;

            let peer_frame = process_frame_descriptor(peer_root, peer_start_slot + j);
            let peer_page_va = peer_va + j * PAGE_SIZE;
            let peer_vm = processes
                .vm_space_mut(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(peer_as, peer_frame, peer_page_va, peer_vm)?;
            j += 1;
        }

        info!(
            "[shm] granted pid={:>3}<->pid={:>3} size={:#x} paddr={:#018x} local={:#018x} peer={:#018x}",
            pid, peer_pid, mapped_size, base_paddr, caller_va, peer_va
        );
        Ok((caller_va, peer_va))
    }

    fn handle_mapping_release_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        if caller_pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let target_pid = if request.arg2 == 0 {
            caller_pid
        } else {
            request.arg2
        };
        let base_va = request.arg0;
        let size_bytes = request.arg1;
        if size_bytes == 0 || (base_va & (PAGE_SIZE - 1)) != 0 {
            info!(
                "[map.err] release invalid args caller={:>3} target={:>3} va={:#018x} bytes={:#x}",
                caller_pid, target_pid, base_va, size_bytes
            );
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let entry = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or_else(|| {
                info!("[map.err] release unknown target={:>3}", target_pid);
                CapabilityError::InvalidArgument
            })?;
        if caller_pid != target_pid && entry.reaper_pid != caller_pid {
            info!(
                "[map.err] release denied caller={:>3} target={:>3} reaper={:>3}",
                caller_pid, target_pid, entry.reaper_pid
            );
            return Err(CapabilityError::PermissionDenied);
        }
        let exact_allocation = self
            .processes
            .find_active_physical_allocation_reference(target_pid, base_va, page_count);

        let mut i = 0usize;
        while i < page_count {
            let va = base_va + i * PAGE_SIZE;
            let allocation = if let Some(allocation) = exact_allocation {
                allocation
            } else {
                self.processes
                    .find_active_physical_allocation_reference(target_pid, va, 1)
                    .ok_or_else(|| {
                        info!(
                            "[map.err] release lookup failed caller={:>3} target={:>3} va={:#018x} bytes={:#x} page={}/{}",
                            caller_pid, target_pid, va, mapped_size, i, page_count
                        );
                        CapabilityError::InvalidArgument
                    })?
            };
            let slot = allocation.start_slot + if allocation.page_count == 1 { 0 } else { i };
            let frame = process_frame_descriptor(entry.root_node, slot);
            if let Err(e) = arch::address_space::unmap(entry.address_space, frame, va) {
                info!(
                    "[map.err] unmap target={:>3} va={:#018x} frame={:#018x} slot={} err={:?}",
                    target_pid, va, frame, slot, e
                );
                return Err(e);
            }
            if let Some(vm) = self.processes.vm_space_mut(target_pid) {
                vm.forget_frame(va);
            }
            i += 1;
        }

        if exact_allocation.is_some() {
            let (allocation, _) = self
                .processes
                .release_physical_allocation_reference(target_pid, base_va, page_count)?;
            self.processes
                .defer_physical_allocation_for_pid(target_pid, allocation);
        } else {
            let mut page = 0usize;
            while page < page_count {
                let va = base_va + page * PAGE_SIZE;
                let (allocation, _) = self
                    .processes
                    .release_physical_allocation_reference(target_pid, va, 1)?;
                self.processes
                    .defer_physical_allocation_for_pid(target_pid, allocation);
                page += 1;
            }
        }
        info!(
            "[map] released caller={:>3} target={:>3} va=[{:#018x}..{:#018x}) pages={}",
            caller_pid,
            target_pid,
            base_va,
            base_va + mapped_size,
            page_count
        );
        Ok(())
    }

    fn handle_process_memory_copy_request(
        &mut self,
        request: OsRequestEvent,
        write_to_target: bool,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let target_va = request.arg1;
        let caller_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0 || target_pid == 0 || target_va == 0 || caller_va == 0 || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        if self.processes.find_entry_by_pid(caller_pid).is_none() {
            return Err(CapabilityError::InvalidArgument);
        }

        let (source_pid, source_va, destination_pid, destination_va) = if write_to_target {
            (caller_pid, caller_va, target_pid, target_va)
        } else {
            (target_pid, target_va, caller_pid, caller_va)
        };
        self.copy_process_memory(
            source_pid,
            source_va,
            destination_pid,
            destination_va,
            size_bytes,
        )
    }

    fn validate_process_memory_access(
        &self,
        caller_pid: usize,
        target_pid: usize,
    ) -> Result<(), CapabilityError> {
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if caller_pid != target_pid && target.reaper_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        Ok(())
    }

    fn handle_process_memory_clone_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let source_pid = request.arg0;
        let destination_pid = request.arg1;
        let base_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0
            || source_pid == 0
            || destination_pid == 0
            || base_va == 0
            || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.validate_process_memory_access(caller_pid, source_pid)?;
        self.validate_process_memory_access(caller_pid, destination_pid)?;
        self.copy_process_memory(source_pid, base_va, destination_pid, base_va, size_bytes)
    }

    fn handle_process_memory_copy_within_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let source_va = request.arg1;
        let destination_va = request.arg2;
        let size_bytes = request.arg3;
        if caller_pid == 0
            || target_pid == 0
            || source_va == 0
            || destination_va == 0
            || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.validate_process_memory_access(caller_pid, target_pid)?;
        self.copy_process_memory(
            target_pid,
            source_va,
            target_pid,
            destination_va,
            size_bytes,
        )
    }

    fn handle_process_map_anonymous_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let size_bytes = request.arg1;
        let requested_base = request.arg2;
        if caller_pid == 0 || target_pid == 0 || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let base = if requested_base == 0 {
            self.map_process_heap_pages(target_pid, page_count)
                .map_err(|e| {
                    info!(
                        "[proc.map.err] caller={:>3} target={:>3} bytes={:#x} pages={} err={:?}",
                        caller_pid, target_pid, size_bytes, page_count, e
                    );
                    e
                })?
        } else {
            self.map_process_heap_pages_at(target_pid, requested_base, page_count)
                .map_err(|e| {
                    info!(
                        "[proc.map.err] caller={:>3} target={:>3} fixed={:#018x} bytes={:#x} pages={} err={:?}",
                        caller_pid, target_pid, requested_base, size_bytes, page_count, e
                    );
                    e
                })?
        };
        info!(
            "[proc.map] granted caller={:>3} target={:>3} bytes={:#x} mapped={:#x} va=[{:#018x}..{:#018x})",
            caller_pid,
            target_pid,
            size_bytes,
            mapped_size,
            base,
            base + mapped_size
        );
        Ok((base, mapped_size))
    }

    fn copy_process_memory(
        &mut self,
        source_pid: usize,
        source_va: usize,
        destination_pid: usize,
        destination_va: usize,
        size_bytes: usize,
    ) -> Result<(), CapabilityError> {
        if source_pid == destination_pid && source_va == destination_va {
            return Ok(());
        }
        let source_end = source_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let _ = destination_va
            .checked_add(size_bytes)
            .ok_or(CapabilityError::InvalidArgument)?;
        let copy_backward = source_pid == destination_pid
            && destination_va > source_va
            && destination_va < source_end;
        let src_temp_va = PROCESS_COPY_TEMP_BASE;
        let dst_temp_va = PROCESS_COPY_TEMP_BASE + PAGE_SIZE;
        let mut copied = 0usize;
        while copied < size_bytes {
            let (src, dst, chunk) = if copy_backward {
                let remaining = size_bytes - copied;
                let src_end = source_va + remaining;
                let dst_end = destination_va + remaining;
                let src_page_tail = ((src_end - 1) & (PAGE_SIZE - 1)) + 1;
                let dst_page_tail = ((dst_end - 1) & (PAGE_SIZE - 1)) + 1;
                let chunk = min_usize(remaining, min_usize(src_page_tail, dst_page_tail));
                (src_end - chunk, dst_end - chunk, chunk)
            } else {
                let src = source_va + copied;
                let dst = destination_va + copied;
                let src_offset = src & (PAGE_SIZE - 1);
                let dst_offset = dst & (PAGE_SIZE - 1);
                let chunk = min_usize(
                    size_bytes - copied,
                    min_usize(PAGE_SIZE - src_offset, PAGE_SIZE - dst_offset),
                );
                (src, dst, chunk)
            };
            let src_page = align_down(src, PAGE_SIZE);
            let dst_page = align_down(dst, PAGE_SIZE);
            let src_offset = src - src_page;
            let dst_offset = dst - dst_page;
            let src_frame = self.process_frame_for_page(source_pid, src_page)?;
            let dst_frame = self.process_frame_for_page(destination_pid, dst_page)?;
            self.map_alpha_temporary_frame(src_frame, src_temp_va)?;

            if src_frame == dst_frame {
                let bounce = core::ptr::addr_of_mut!(PROCESS_COPY_BOUNCE_BUFFER) as *mut u8;
                unsafe {
                    ptr::copy_nonoverlapping(
                        (src_temp_va + src_offset) as *const u8,
                        bounce,
                        chunk,
                    );
                    ptr::copy_nonoverlapping(bounce, (src_temp_va + dst_offset) as *mut u8, chunk);
                }
                self.unmap_alpha_temporary_frame(src_frame, src_temp_va)?;
            } else {
                if let Err(error) = self.map_alpha_temporary_frame(dst_frame, dst_temp_va) {
                    let _ = self.unmap_alpha_temporary_frame(src_frame, src_temp_va);
                    return Err(error);
                }
                unsafe {
                    ptr::copy_nonoverlapping(
                        (src_temp_va + src_offset) as *const u8,
                        (dst_temp_va + dst_offset) as *mut u8,
                        chunk,
                    );
                }
                let src_unmap = self.unmap_alpha_temporary_frame(src_frame, src_temp_va);
                let dst_unmap = self.unmap_alpha_temporary_frame(dst_frame, dst_temp_va);
                src_unmap?;
                dst_unmap?;
            }
            copied += chunk;
        }

        Ok(())
    }

    fn read_process_memory_into_static_buffer(
        &mut self,
        source_pid: usize,
        source_va: usize,
        size_bytes: usize,
    ) -> Result<&'static [u8], CapabilityError> {
        if size_bytes > PROCESS_SPAWN_MEMORY_MAX_BYTES {
            return Err(CapabilityError::InvalidArgument);
        }
        let out = core::ptr::addr_of_mut!(PROCESS_MEMORY_IMAGE_BUFFER) as *mut u8;
        let mut copied = 0usize;
        while copied < size_bytes {
            let src = source_va
                .checked_add(copied)
                .ok_or(CapabilityError::InvalidArgument)?;
            let src_page = align_down(src, PAGE_SIZE);
            let src_offset = src - src_page;
            let chunk = min_usize(size_bytes - copied, PAGE_SIZE - src_offset);
            let temp_offset = (copied / PAGE_SIZE)
                .checked_mul(PAGE_SIZE)
                .ok_or(CapabilityError::InvalidArgument)?;
            if temp_offset + PAGE_SIZE > PROCESS_COPY_TEMP_WINDOW_SIZE {
                return Err(CapabilityError::InvalidArgument);
            }
            let temp_va = PROCESS_COPY_TEMP_BASE + temp_offset;
            let (src_frame, src_temp) =
                self.map_process_page_into_alpha(source_pid, src_page, temp_va)?;

            unsafe {
                ptr::copy_nonoverlapping(
                    (src_temp + src_offset) as *const u8,
                    out.add(copied),
                    chunk,
                );
            }

            if let Err(e) = self.unmap_alpha_temporary_frame(src_frame, src_temp) {
                info!(
                    "[proc.copy.err] unmap spawn-memory temp={:#018x} frame={:#018x} err={:?}",
                    src_temp, src_frame, e
                );
                return Err(e);
            }
            copied += chunk;
        }
        Ok(unsafe { core::slice::from_raw_parts(out as *const u8, size_bytes) })
    }

    fn map_process_page_into_alpha(
        &mut self,
        pid: usize,
        page_va: usize,
        temp_va: usize,
    ) -> Result<(CapabilityDescriptor, usize), CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 || temp_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let frame = self.process_frame_for_page(pid, page_va)?;
        self.map_alpha_temporary_frame(frame, temp_va)?;
        Ok((frame, temp_va))
    }

    fn process_frame_for_page(
        &mut self,
        pid: usize,
        page_va: usize,
    ) -> Result<CapabilityDescriptor, CapabilityError> {
        if page_va & (PAGE_SIZE - 1) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        let entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let frame = match self
            .processes
            .vm_space_mut(pid)
            .and_then(|vm| vm.find_frame(page_va))
        {
            Some(frame) => frame,
            None => {
                self.materialize_lazy_page(pid, entry.root_node, entry.address_space, page_va)?;
                self.processes
                    .vm_space_mut(pid)
                    .and_then(|vm| vm.find_frame(page_va))
                    .ok_or(CapabilityError::InvalidArgument)?
            }
        };
        Ok(frame)
    }

    fn map_alpha_temporary_frame(
        &mut self,
        frame: CapabilityDescriptor,
        temp_va: usize,
    ) -> Result<(), CapabilityError> {
        let alpha_as = self.processes.alpha_entry().address_space;
        if let Some(old_frame) = self.processes.alpha_vm_space_mut().find_frame(temp_va) {
            match arch::address_space::unmap(alpha_as, old_frame, temp_va) {
                Ok(()) | Err(CapabilityError::IllegalOperation) => {
                    self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                }
                Err(error) => return Err(error),
            }
        }
        let vm = self.processes.alpha_vm_space_mut();
        self.memory.map_frame_strict(alpha_as, frame, temp_va, vm)
    }

    fn unmap_alpha_temporary_frame(
        &mut self,
        frame: CapabilityDescriptor,
        temp_va: usize,
    ) -> Result<(), CapabilityError> {
        let alpha_as = self.processes.alpha_entry().address_space;
        match arch::address_space::unmap(alpha_as, frame, temp_va) {
            Ok(()) | Err(CapabilityError::IllegalOperation) => {
                self.processes.alpha_vm_space_mut().forget_frame(temp_va);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn handle_shared_framebuffer_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let peer_pid = request.arg0;
        let physical_address = request.arg1;
        let size_bytes = request.arg2;
        if peer_pid == 0 || peer_pid == pid || physical_address == 0 || size_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let framebuffer_start = self.initial_framebuffer.address;
        let framebuffer_end = framebuffer_start.saturating_add(self.initial_framebuffer.size_bytes);
        let request_end = physical_address.saturating_add(size_bytes);
        if physical_address < framebuffer_start || request_end > framebuffer_end {
            return Err(CapabilityError::PermissionDenied);
        }

        if self.processes.find_entry_by_pid(pid).is_none()
            || self.processes.find_entry_by_pid(peer_pid).is_none()
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let page_base = physical_address & !(PAGE_SIZE - 1);
        let offset = physical_address - page_base;
        let mapped_size = align_up(offset.saturating_add(size_bytes), PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        if page_count == 0 {
            return Err(CapabilityError::InvalidArgument);
        }
        info!(
            "[fb-shm] request pid={:>3}->pid={:>3} paddr={:#018x} size={:#x} mapped={:#x} pages={}",
            pid, peer_pid, physical_address, size_bytes, mapped_size, page_count
        );

        // The caller is fb-server, which already mapped the physical framebuffer via MMIO.
        // Only the peer compositor needs a new mapping here; mapping the caller again
        // consumes thousands of process frame slots at 1920x1080.
        let (peer_root, peer_as, peer_base_va, peer_start_slot) = match self
            .processes
            .reserve_process_heap(peer_pid, page_count, PAGE_SIZE, PROCESS_FRAME_TOTAL_PAGES)
        {
            Ok(v) => v,
            Err(e) => {
                match self.processes.find_entry_by_pid(peer_pid) {
                    Some(entry) => {
                        info!(
                            "[fb-shm.err] reserve peer pid={:>3} pages={} next_slot={} max_slots={} heap_next={:#018x} heap_limit={:#018x} err={:?}",
                            peer_pid,
                            page_count,
                            entry.next_frame_slot,
                            PROCESS_FRAME_TOTAL_PAGES,
                            entry.user_heap_next_va,
                            entry.user_heap_limit_va,
                            e
                        );
                    }
                    None => {
                        info!(
                            "[fb-shm.err] reserve peer pid={:>3} missing err={:?}",
                            peer_pid, e
                        );
                    }
                }
                return Err(e);
            }
        };

        let (converted_base_index, skip_pages, converted_page_count) = match self
            .memory
            .ensure_alpha_frames_for_range_from_initial_generic(page_base, mapped_size, true)
        {
            Ok(v) => v,
            Err(e) => {
                info!(
                    "[fb-shm.err] ensure frames paddr={:#018x} mapped={:#x} pages={} err={:?}",
                    page_base, mapped_size, page_count, e
                );
                return Err(e);
            }
        };
        if converted_page_count != page_count {
            info!(
                "[fb-shm.err] converted count mismatch expected={} actual={} base_index={} skip={}",
                page_count, converted_page_count, converted_base_index, skip_pages
            );
            return Err(CapabilityError::InvalidArgument);
        }

        self.ensure_process_frame_chunks(peer_pid, peer_root, peer_start_slot, page_count)?;

        let mut i = 0usize;
        while i < page_count {
            let source_frame = self
                .memory
                .physical_frame_descriptor_from_index(converted_base_index + skip_pages + i)
                .ok_or(CapabilityError::InvalidArgument)?;
            let dst_node = process_frame_chunk_descriptor(
                peer_root,
                (peer_start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
            );
            let dst_slot = (peer_start_slot + i) % PROCESS_FRAME_CHUNK_PAGES;
            if let Err(e) = arch::node::copy(dst_node, dst_slot as Word, source_frame) {
                info!(
                    "[fb-shm.err] copy frame i={} dst_slot={} src={:#018x} peer_node={:#018x} err={:?}",
                    i,
                    dst_slot,
                    source_frame,
                    dst_node,
                    e
                );
                return Err(e);
            }
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let peer_frame = process_frame_descriptor(peer_root, peer_start_slot + j);
            let peer_page_va = peer_base_va + j * PAGE_SIZE;
            let peer_vm = processes
                .vm_space_mut(peer_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            if let Err(e) = memory.map_frame(peer_as, peer_frame, peer_page_va, peer_vm) {
                info!(
                    "[fb-shm.err] map frame j={} va={:#018x} frame={:#018x} as={:#018x} err={:?}",
                    j, peer_page_va, peer_frame, peer_as, e
                );
                return Err(e);
            }
            j += 1;
        }

        let peer_va = peer_base_va.saturating_add(offset);
        info!(
            "[fb-shm] granted pid={:>3}->pid={:>3} size={:#x} paddr={:#018x} peer={:#018x}",
            pid, peer_pid, mapped_size, page_base, peer_va
        );
        Ok((0, peer_va))
    }

    fn handle_mmio_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(usize, usize), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }
        let physical_address = request.arg0;
        let size_bytes = request.arg1;
        if size_bytes == 0 || (physical_address & (PAGE_SIZE - 1)) != 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let mapped_size = align_up(size_bytes, PAGE_SIZE);
        let page_count = mapped_size / PAGE_SIZE;
        let (root_node, address_space, base_va, start_slot) = self.processes.reserve_process_heap(
            pid,
            page_count,
            PAGE_SIZE,
            PROCESS_FRAME_TOTAL_PAGES,
        )?;
        let base_page = self
            .memory
            .allocate_physical_at(physical_address, mapped_size, true)?;
        let base_paddr = base_page * PAGE_SIZE;
        if base_paddr != physical_address {
            return Err(CapabilityError::InvalidArgument);
        }
        let (converted_base_index, skip_pages, converted_page_count) = self
            .memory
            .ensure_alpha_frames_for_range_from_initial_generic(
                physical_address,
                mapped_size,
                true,
            )?;
        if converted_page_count != page_count {
            return Err(CapabilityError::InvalidArgument);
        }

        self.ensure_process_frame_chunks(pid, root_node, start_slot, page_count)?;
        let mut i = 0usize;
        while i < page_count {
            let source_frame = self
                .memory
                .physical_frame_descriptor_from_index(converted_base_index + skip_pages + i)
                .ok_or(CapabilityError::InvalidArgument)?;
            arch::node::copy(
                process_frame_chunk_descriptor(
                    root_node,
                    (start_slot + i) / PROCESS_FRAME_CHUNK_PAGES,
                ),
                ((start_slot + i) % PROCESS_FRAME_CHUNK_PAGES) as Word,
                source_frame,
            )?;
            i += 1;
        }

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let mut j = 0usize;
        while j < page_count {
            let frame = process_frame_descriptor(root_node, start_slot + j);
            let va = base_va + j * PAGE_SIZE;
            let vm = processes
                .vm_space_mut(pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            memory.map_frame(address_space, frame, va, vm)?;
            j += 1;
        }

        info!(
            "[mmio] granted pid={:>3} size={:#x} paddr={:#018x} vaddr={:#018x}",
            pid, mapped_size, physical_address, base_va
        );
        Ok((physical_address, base_va))
    }

    fn handle_io_port_control_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<(), CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let range_min = request.arg0;
        let range_max = request.arg1;
        let io_port_slot = request.arg2;

        if range_min > range_max {
            return Err(CapabilityError::InvalidArgument);
        }

        validate_process_device_slot(io_port_slot)?;

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        io_port_mint(
            self.root_io_port,
            range_min,
            range_max,
            process_entry.root_node,
            io_port_slot,
        )?;

        self.processes
            .add_io_range_to_pid(pid, range_min, range_max)?;

        info!(
            "[io] granted pid={:>3} range=[{:#018x}..={:#018x}] slot={:>3}",
            pid, range_min, range_max, io_port_slot
        );

        Ok(())
    }

    fn handle_service_register_request(
        &mut self,
        request: OsRequestEvent,
    ) -> Result<usize, CapabilityError> {
        let pid = request.identifier;
        if pid == 0 {
            return Err(CapabilityError::PermissionDenied);
        }

        let process_entry = self.processes.find_entry_by_pid(pid).ok_or_else(|| {
            error!("[svc.err] register unknown pid={:>3}", pid);
            CapabilityError::InvalidArgument
        })?;

        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or_else(|| {
                error!(
                    "[svc.err] register decode failed pid={:>3} args=[{:#018x},{:#018x},{:#018x}] slot={:#x}",
                    pid, request.arg0, request.arg1, request.arg2, request.arg3
                );
                CapabilityError::InvalidArgument
            })?;
        let service_name = core::str::from_utf8(&raw_name[..raw_len]).map_err(|_| {
            error!(
                "[svc.err] register non-utf8 pid={:>3} len={} args=[{:#018x},{:#018x},{:#018x}]",
                pid, raw_len, request.arg0, request.arg1, request.arg2
            );
            CapabilityError::InvalidArgument
        })?;
        let service_slot = request.arg3;
        validate_process_device_slot(service_slot).map_err(|e| {
            error!(
                "[svc.err] register invalid slot pid={:>3} name={} slot={:#x}",
                pid, service_name, service_slot
            );
            e
        })?;
        let service_port =
            make_child_slot_descriptor(process_entry.root_node, PROCESS_ROOT_RADIX, service_slot);

        self.communication
            .register_service(pid, service_name, service_port)?;

        info!(
            "[svc] registered name={} pid={:>3} port={:#018x}",
            service_name, pid, service_port
        );

        Ok(pid)
    }

    fn handle_service_list_request(&self, request: OsRequestEvent) -> Option<(usize, usize)> {
        self.communication.service_info_by_ordinal(request.arg0)
    }

    fn prepare_alpha_heap(
        init_info: &InitInfo,
        memory: &mut MemoryManager,
        root_radix: usize,
        bootstrap_generic: CapabilityDescriptor,
        alpha_address_space: CapabilityDescriptor,
        alpha_vm_space: &mut crate::nanami_core::vm_space::BootstrapVmSpace,
    ) -> Result<usize, CapabilityError> {
        let mut selected: Option<(usize, usize, usize, usize)> = None;
        let needed_bytes = ALPHA_HEAP_PAGES * PAGE_SIZE;
        let count = init_info.generic_list_count as usize;

        for i in 0..count {
            let g = init_info.generic_list[i];
            if g.is_device || g.size_radix < 12 {
                continue;
            }
            let size_bytes = 1usize << g.size_radix;
            if size_bytes < needed_bytes {
                continue;
            }
            let desc = make_generic_descriptor(root_radix, i);
            if desc == bootstrap_generic {
                continue;
            }
            let consumed = memory.initial_generic_consumed_bytes_for_public(i);
            let Some(split_start) =
                align_up_checked((g.address as usize).saturating_add(consumed), PAGE_SIZE)
            else {
                continue;
            };
            let end = (g.address as usize).saturating_add(size_bytes);
            let Some(heap_end) = split_start.checked_add(needed_bytes) else {
                continue;
            };
            if heap_end > end {
                continue;
            }
            let Some(base_page) = memory.physical_page_index_from_address(split_start) else {
                continue;
            };
            if memory
                .physical_frame_descriptor_from_index(base_page + ALPHA_HEAP_PAGES - 1)
                .is_none()
            {
                continue;
            }
            let usable_bytes = end - split_start;

            match selected {
                None => selected = Some((i, split_start, base_page, usable_bytes)),
                Some((_, _, _, best_size)) if usable_bytes < best_size => {
                    selected = Some((i, split_start, base_page, usable_bytes))
                }
                _ => {}
            }
        }

        let (generic_idx, base_address, base_page, usable_bytes) =
            selected.ok_or(CapabilityError::InvalidArgument)?;
        info!(
            "heap generic idx={:>3} addr={:#018x} usable={:#x} pages={:>4}",
            generic_idx, base_address, usable_bytes, ALPHA_HEAP_PAGES
        );

        let mut i = 0usize;
        while i < ALPHA_HEAP_PAGES {
            if let Err(e) = memory.ensure_alpha_frame_at_physical_index(base_page + i) {
                info!(
                    "[heap.err] ensure frame idx={:>3} page={} frame_index={} err={:?}",
                    generic_idx,
                    i,
                    base_page + i,
                    e
                );
                return Err(e);
            }
            i += 1;
        }

        let mut j = 0usize;
        while j < ALPHA_HEAP_PAGES {
            let frame = memory
                .physical_frame_descriptor_from_index(base_page + j)
                .ok_or(CapabilityError::InvalidArgument)?;
            let va = ALPHA_HEAP_BASE + j * PAGE_SIZE;
            if let Err(e) = memory.map_frame(alpha_address_space, frame, va, alpha_vm_space) {
                info!(
                    "[heap.err] map idx={:>3} page={} va={:#018x} frame={:#018x} err={:?}",
                    generic_idx, j, va, frame, e
                );
                return Err(e);
            }
            unsafe {
                ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE);
            }
            j += 1;
        }

        Ok(base_address)
    }

    fn prepare_runtime_stack(&mut self) -> Result<usize, CapabilityError> {
        let stack_node =
            make_root_slot_descriptor(self.root.root_radix, ALPHA_RUNTIME_STACK_NODE_SLOT);
        match arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            ALPHA_RUNTIME_STACK_NODE_RADIX as Word,
            1,
            self.root.root_descriptor,
            ALPHA_RUNTIME_STACK_NODE_SLOT as Word,
        ) {
            Ok(()) | Err(CapabilityError::InvalidArgument) => {}
            Err(e) => return Err(e),
        }

        let _ = self.memory.allocate_process_frames(
            stack_node,
            ALPHA_RUNTIME_STACK_NODE_RADIX,
            0,
            ALPHA_RUNTIME_STACK_PAGES,
        )?;

        let memory = &mut self.memory;
        let processes = &mut self.processes;
        let alpha_as = processes.alpha_entry().address_space;
        let vm_space = processes.alpha_vm_space_mut();
        let mut i = 0usize;
        while i < ALPHA_RUNTIME_STACK_PAGES {
            let frame = make_child_slot_descriptor(stack_node, ALPHA_RUNTIME_STACK_NODE_RADIX, i);
            let va = ALPHA_RUNTIME_STACK_BASE + i * PAGE_SIZE;
            memory.map_frame(alpha_as, frame, va, vm_space)?;
            unsafe {
                ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE);
            }
            i += 1;
        }

        Ok((ALPHA_RUNTIME_STACK_BASE + ALPHA_RUNTIME_STACK_PAGES * PAGE_SIZE - 16) & !0xFusize)
    }
}

fn validate_process_device_slot(slot: usize) -> Result<(), CapabilityError> {
    if slot < PROCESS_DEVICE_SLOT_MIN || slot > PROCESS_DEVICE_SLOT_MAX {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(())
}

fn validate_process_observer(
    caller_pid: usize,
    target_pid: usize,
    reaper_pid: usize,
) -> Result<(), CapabilityError> {
    if caller_pid == target_pid || reaper_pid == 0 || caller_pid == reaper_pid {
        return Ok(());
    }
    Err(CapabilityError::PermissionDenied)
}

fn process_frame_directory_descriptor(process_root: CapabilityDescriptor) -> CapabilityDescriptor {
    make_child_slot_descriptor(process_root, PROCESS_ROOT_RADIX, PROCESS_SLOT_FRAME_NODE)
}

fn process_frame_chunk_descriptor(
    process_root: CapabilityDescriptor,
    chunk_index: usize,
) -> CapabilityDescriptor {
    make_child_slot_descriptor(
        process_frame_directory_descriptor(process_root),
        PROCESS_FRAME_DIRECTORY_RADIX,
        chunk_index,
    )
}

fn process_frame_descriptor(
    process_root: CapabilityDescriptor,
    global_slot: usize,
) -> CapabilityDescriptor {
    make_child_slot_descriptor(
        process_frame_chunk_descriptor(process_root, global_slot / PROCESS_FRAME_CHUNK_PAGES),
        PROCESS_FRAME_NODE_RADIX,
        global_slot % PROCESS_FRAME_CHUNK_PAGES,
    )
}

fn fill_lazy_page(
    kind: ProcessLazyMappingKind,
    page_va: usize,
    temp_va: usize,
) -> Result<(), CapabilityError> {
    unsafe {
        ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
    }

    let ProcessLazyMappingKind::Image { image, elf } = kind else {
        return Ok(());
    };

    let page_end = page_va
        .checked_add(PAGE_SIZE)
        .ok_or(CapabilityError::InvalidArgument)?;
    let mut i = 0usize;
    while i < elf.segment_count {
        let seg = elf.segments[i];
        let seg_start = seg.virtual_address;
        let seg_mem_end = seg
            .virtual_address
            .checked_add(seg.memory_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        if seg.memory_size == 0 || page_va >= seg_mem_end || page_end <= seg_start {
            i += 1;
            continue;
        }

        let file_end = seg
            .virtual_address
            .checked_add(seg.file_size)
            .ok_or(CapabilityError::InvalidArgument)?;
        let copy_start = max_usize(page_va, seg_start);
        let copy_end = min_usize(page_end, file_end);
        if copy_start < copy_end {
            let src_offset = seg
                .offset
                .checked_add(copy_start - seg.virtual_address)
                .ok_or(CapabilityError::InvalidArgument)?;
            let len = copy_end - copy_start;
            if src_offset
                .checked_add(len)
                .filter(|end| *end <= image.len())
                .is_none()
            {
                return Err(CapabilityError::InvalidArgument);
            }
            let dst = temp_va + (copy_start - page_va);
            unsafe {
                ptr::copy_nonoverlapping(image.as_ptr().add(src_offset), dst as *mut u8, len);
            }
        }

        i += 1;
    }

    if let Some(ipc_buffer_va) = elf.ipc_buffer_start {
        let tls_slot_va = ipc_buffer_va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS;
        if tls_slot_va >= page_va && tls_slot_va + core::mem::size_of::<Word>() <= page_end {
            let dst = temp_va + (tls_slot_va - page_va);
            unsafe {
                ptr::write(dst as *mut Word, ipc_buffer_va as Word);
            }
        }
    }

    Ok(())
}

fn max_usize(a: usize, b: usize) -> usize {
    if a > b {
        a
    } else {
        b
    }
}

fn min_usize(a: usize, b: usize) -> usize {
    if a < b {
        a
    } else {
        b
    }
}

fn select_irq_notification_alias_slot(
    irq_number: usize,
    notification_slot: usize,
    interrupt_slot: usize,
) -> usize {
    let mut slot =
        PROCESS_IRQ_NOTIFICATION_ALIAS_MIN + (irq_number % PROCESS_IRQ_NOTIFICATION_ALIAS_COUNT);
    if slot == notification_slot || slot == interrupt_slot {
        slot = PROCESS_IRQ_NOTIFICATION_ALIAS_MIN
            + ((irq_number + 1) % PROCESS_IRQ_NOTIFICATION_ALIAS_COUNT);
    }
    slot
}

fn irq_notification_identifier(irq_number: usize) -> Result<usize, CapabilityError> {
    let bits = usize::BITS as usize;
    if irq_number >= bits {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(1usize << irq_number)
}

fn map_request_result_to_status(result: Result<(), CapabilityError>) -> (usize, usize) {
    match result {
        Ok(()) => (OS_RESPONSE_OK, 0),
        Err(CapabilityError::InvalidArgument) => (OS_RESPONSE_INVALID_ARGUMENT, 0),
        Err(CapabilityError::PermissionDenied) => (OS_RESPONSE_PERMISSION_DENIED, 0),
        Err(CapabilityError::InvalidDescriptor) => (OS_RESPONSE_INVALID_DESCRIPTOR, 0),
        Err(CapabilityError::IllegalOperation) => (OS_RESPONSE_ILLEGAL_OPERATION, 0),
        Err(CapabilityError::InvalidDepth) => (OS_RESPONSE_INVALID_ARGUMENT, 0),
        Err(CapabilityError::Fatal) => (OS_RESPONSE_FATAL, 0),
        Err(CapabilityError::DebugUnimplemented) => (OS_RESPONSE_FATAL, 0),
    }
}

fn io_port_mint(
    root_io_port: CapabilityDescriptor,
    range_min: Word,
    range_max: Word,
    destination_node: CapabilityDescriptor,
    destination_index: Word,
) -> Result<(), CapabilityError> {
    let mut a0 = root_io_port;
    let mut a1 = nun::capability_call::io_port::OperationType::Mint as Word;
    let a2 = range_min;
    let a3 = range_max;
    let a4 = destination_node as Word;
    let a5 = destination_index;

    unsafe {
        asm!(
            "syscall",
            in("rax") KernelCallType::CapabilityCall as Sword,
            inout("rdi") a0 => a0,
            inout("rsi") a1 => a1,
            in("rdx") a2,
            in("r8") a3,
            in("r9") a4,
            in("r10") a5,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }

    convert_capability_result(a0, a1)
}

extern "C" fn run_on_relocated_stack(alpha_ptr: *mut Alpha) -> ! {
    let alpha = unsafe { &mut *alpha_ptr };
    info!("[stack] switched to runtime stack");
    alpha.run_event_loop();
}

unsafe fn jump_to_relocated_stack(alpha_ptr: *mut Alpha, new_sp: usize) -> ! {
    asm!(
        "mov rdi, {alpha}",
        "mov rsp, {stack}",
        // We enter by `jmp` (not `call`), so synthesize a call frame to satisfy SysV ABI.
        // On function entry, rsp must be 8 mod 16.
        "and rsp, -16",
        "sub rsp, 8",
        "mov rbp, rsp",
        "jmp {entry}",
        alpha = in(reg) alpha_ptr,
        stack = in(reg) new_sp,
        entry = in(reg) run_on_relocated_stack as extern "C" fn(*mut Alpha) -> !,
        options(noreturn)
    )
}

#[inline(always)]
fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

#[inline(always)]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn align_up_checked(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value.checked_add(align - 1).map(|v| v & !(align - 1))
}

fn create_alpha_os_port(
    root_descriptor: CapabilityDescriptor,
    _root_radix: usize,
    bootstrap_generic: CapabilityDescriptor,
) -> Result<CapabilityDescriptor, CapabilityError> {
    info!(
        "convert IpcPort into root slot {:>3}",
        ORIGINAL_OS_PORT_SLOT
    );

    arch::generic::convert(
        bootstrap_generic,
        nun::CapabilityType::IpcPort,
        0,
        1,
        root_descriptor,
        ORIGINAL_OS_PORT_SLOT,
    )?;

    let descriptor =
        crate::nanami_utils::descriptor::make_root_slot_descriptor(12, ORIGINAL_OS_PORT_SLOT);
    let _ = arch::ipc_port::identify(descriptor, 0);
    info!(
        "slot {:>3} descriptor={:#018x}",
        ORIGINAL_OS_PORT_SLOT, descriptor
    );
    Ok(descriptor)
}

fn make_generic_descriptor(root_radix: usize, generic_index: usize) -> CapabilityDescriptor {
    let generic_node = make_root_slot_descriptor(root_radix, InitSlotOffset::GenericNode as usize);
    make_child_slot_descriptor(generic_node, GENERIC_NODE_RADIX, generic_index)
}

fn extract_initial_framebuffer_information(
    init_info: &InitInfo,
) -> Option<InitialFramebufferInformation> {
    let mut raw = [0usize; 13];
    raw.copy_from_slice(&init_info.arch_info[1..14]);
    let fb = FramebufferInfo::deserialize(&raw);
    if fb.address == 0 || fb.width == 0 || fb.height == 0 || fb.bits_per_pixel == 0 {
        return None;
    }

    let bytes_per_pixel = (fb.bits_per_pixel as usize).saturating_div(8);
    if bytes_per_pixel == 0 {
        return None;
    }

    let stride_raw = fb.stride as usize;
    let stride_bytes = if stride_raw >= fb.width as usize * bytes_per_pixel {
        stride_raw
    } else {
        stride_raw.saturating_mul(bytes_per_pixel)
    };
    let size_bytes = stride_bytes.saturating_mul(fb.height as usize);
    if size_bytes == 0 {
        return None;
    }

    Some(InitialFramebufferInformation {
        display_id: 0,
        address: fb.address,
        size_bytes,
        width: fb.width as usize,
        height: fb.height as usize,
        stride: fb.stride as usize,
        bits_per_pixel: fb.bits_per_pixel as usize,
        red_position: fb.red.position as usize,
        red_size: fb.red.size as usize,
        green_position: fb.green.position as usize,
        green_size: fb.green.size as usize,
        blue_position: fb.blue.position as usize,
        blue_size: fb.blue.size as usize,
    })
}

fn pack_framebuffer_color_information(
    red_position: usize,
    red_size: usize,
    green_position: usize,
    green_size: usize,
    blue_position: usize,
    blue_size: usize,
) -> usize {
    (red_position & 0x1f)
        | ((red_size & 0x1f) << 5)
        | ((green_position & 0x1f) << 10)
        | ((green_size & 0x1f) << 15)
        | ((blue_position & 0x1f) << 20)
        | ((blue_size & 0x1f) << 25)
}

fn process_priority_for_image(image_name: &str) -> Word {
    match strip_elf_suffix(basename(image_name)) {
        // Timer must preempt clients promptly; animation and network timeouts depend on it.
        "timer-server" => PROCESS_PRIORITY_TIMER_SERVER,
        // Input pipeline must stay above the compositor and every input consumer.
        "input-server" | "ps2-server" => PROCESS_PRIORITY_INPUT_SERVER,
        // GUI servers are above GUI clients, but below timer/input IRQ-facing services.
        "fb-server" | "honoka" => PROCESS_PRIORITY_GUI_SERVER,
        // Background servers stay above clients, but below the GUI critical path.
        "block-device-server" | "virtio-blk-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "virtio-net" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "ext2-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "net-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "rtc-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "http-server" => PROCESS_PRIORITY_BACKGROUND_SERVER,
        "honoka-client" | "eg-test" | "image-viewer" | "performance-monitor" => {
            PROCESS_PRIORITY_INTERACTIVE_CLIENT
        }
        "shell" => PROCESS_PRIORITY_CLIENT,
        "cpp-hello" | "rust-hello" => PROCESS_PRIORITY_BACKGROUND_CLIENT,
        _ => PROCESS_PRIORITY_LOW,
    }
}

fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    }
}

fn decode_service_name_24(arg1: Word, arg2: Word, arg3: Word) -> Option<([u8; 24], usize)> {
    let mut raw = [0u8; 24];
    raw[0..8].copy_from_slice(&arg1.to_le_bytes());
    raw[8..16].copy_from_slice(&arg2.to_le_bytes());
    raw[16..24].copy_from_slice(&arg3.to_le_bytes());

    let mut len = 0usize;
    while len < raw.len() && raw[len] != 0 {
        len += 1;
    }
    if len == 0 {
        return None;
    }
    Some((raw, len))
}

struct ControlText {
    raw: [u8; 16],
    len: usize,
}

fn decode_control_text(arg0: Word, arg1: Word) -> ControlText {
    let mut raw = [0u8; 16];
    raw[0..8].copy_from_slice(&arg0.to_le_bytes());
    raw[8..16].copy_from_slice(&arg1.to_le_bytes());
    let mut len = 0usize;
    while len < raw.len() && raw[len] != 0 {
        len += 1;
    }
    ControlText { raw, len }
}

fn bytes_equal(text: &ControlText, expected: &[u8]) -> bool {
    text.len == expected.len() && &text.raw[..text.len] == expected
}

fn initramfs_entry_data(requested_name: &str) -> Option<&'static [u8]> {
    let mut found = None;
    let _ = cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
        if found.is_none() && initramfs_path_matches(entry.name, requested_name) {
            found = Some(entry.data);
        }
        Ok(())
    });
    found
}

fn parse_boot_list_line(line: &str) -> Option<BootListEntry<'_>> {
    let line = line.split('#').next().unwrap_or("");
    let mut tokens = line.split_whitespace();
    let name = tokens.next()?;
    let priority = parse_decimal_word(tokens.next()?)?;
    let image_path = tokens.next()?;
    Some(BootListEntry {
        _name: name,
        priority,
        image_path,
    })
}

fn parse_decimal_word(text: &str) -> Option<Word> {
    if text.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((byte - b'0') as usize)?;
    }
    Some(value as Word)
}

fn initramfs_image_name_matches(entry_name: &str, requested_name: &str) -> bool {
    if initramfs_path_matches(entry_name, requested_name) {
        return true;
    }
    let entry_base = path_basename(entry_name);
    let requested_base = path_basename(requested_name);
    if initramfs_path_matches(entry_base, requested_name)
        || initramfs_path_matches(entry_name, requested_base)
        || entry_base == requested_base
    {
        return true;
    }

    let entry_stem = strip_elf_suffix(entry_base);
    let requested_stem = strip_elf_suffix(requested_base);
    entry_stem == strip_elf_suffix(requested_name)
        || strip_elf_suffix(entry_name) == requested_stem
        || entry_stem == requested_stem
}

fn initramfs_path_matches(entry_name: &str, requested_name: &str) -> bool {
    entry_name == requested_name
        || entry_name.strip_prefix("./") == Some(requested_name)
        || requested_name.strip_prefix("./") == Some(entry_name)
}

fn initramfs_image_is_explicit_only(name: &str) -> bool {
    path_basename(name).starts_with('_')
}

fn initramfs_image_is_auto_spawn_candidate(name: &str) -> bool {
    name.starts_with("./bin/") || name.starts_with("bin/")
}

fn strip_elf_suffix(name: &str) -> &str {
    name.strip_suffix(".elf").unwrap_or(name)
}

fn path_basename(path: &str) -> &str {
    let bytes = path.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    &path[start..]
}
