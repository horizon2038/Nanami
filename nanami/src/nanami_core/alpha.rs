use crate::nanami_core::capability_space::RootCapabilitySpace;
use crate::nanami_core::communication::{
    CommunicationEvent, CommunicationManager, KernelFaultEvent, NotificationEvent, OsRequestEvent,
    OS_REQUEST_DEBUG_PING, OS_REQUEST_DMA_REQUEST, OS_REQUEST_EXIT, OS_REQUEST_HEAP_ALLOC,
    OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION, OS_REQUEST_IO_PORT_CONTROL, OS_REQUEST_IRQ_CONTROL,
    OS_REQUEST_MMIO_REQUEST, OS_REQUEST_NOTIFICATION_PORT_COPY,
    OS_REQUEST_NOTIFICATION_PORT_CREATE, OS_REQUEST_PAGE_ALLOC, OS_REQUEST_SELF_PID,
    OS_REQUEST_SERVICE_CONNECT, OS_REQUEST_SERVICE_LIST, OS_REQUEST_SERVICE_REGISTER,
    OS_REQUEST_SHARED_FRAMEBUFFER_CREATE, OS_REQUEST_SHARED_MEMORY_CREATE, OS_RESPONSE_FATAL,
    OS_RESPONSE_ILLEGAL_OPERATION, OS_RESPONSE_INVALID_ARGUMENT, OS_RESPONSE_INVALID_DESCRIPTOR,
    OS_RESPONSE_OK, OS_RESPONSE_PERMISSION_DENIED, OS_RESPONSE_PONG_MAGIC,
};
use crate::nanami_core::cpio;
use crate::nanami_core::elf_loader::parse_elf64;
use crate::nanami_core::memory::MemoryManager;
use crate::nanami_core::process::ProcessManager;
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use crate::nanami_utils::heap::init_global_heap;
use crate::{debug, error, info, warn};
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
const PROCESS_FRAME_DIRECTORY_RADIX: usize = 8;
const PROCESS_FRAME_NODE_RADIX: usize = 14;
const PROCESS_FRAME_CHUNK_PAGES: usize = 1 << PROCESS_FRAME_NODE_RADIX;
const PROCESS_FRAME_TOTAL_PAGES: usize =
    (1 << PROCESS_FRAME_DIRECTORY_RADIX) * PROCESS_FRAME_CHUNK_PAGES;
const PAGE_TABLE_NODE_RADIX: usize = 7;
const PAGE_SIZE: usize = 4096;
const USER_STACK_BASE: usize = 0x0400_0000;
const USER_STACK_PAGES: usize = 64;
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
const TEMP_MAP_STRIDE: usize = 0x0200_0000;
const ALPHA_RUNTIME_STACK_NODE_SLOT: usize = 1300;
const ALPHA_RUNTIME_STACK_NODE_RADIX: usize = 12;
const ALPHA_RUNTIME_STACK_BASE: usize = 0x5000_0000;
const ALPHA_RUNTIME_STACK_PAGES: usize = 1024;
const ALPHA_HEAP_BASE: usize = 0x5800_0000;
const ALPHA_HEAP_PAGES: usize = 1024;
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
const PROCESS_ROOT_RESERVED_SLOTS: [usize; 13] = [
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

impl Alpha {
    pub fn bootstrap(init_info: &InitInfo) -> Result<Self, CapabilityError> {
        info!("alpha bootstrap start");

        info!("root capability space bootstrap");
        let root = RootCapabilitySpace::bootstrap(init_info)?;
        info!(
            "root={:#018x} radix={:>2} bootstrap_generic={:#018x}",
            root.root_descriptor, root.root_radix, root.bootstrap_generic
        );

        info!("memory manager bootstrap");
        let mut memory = MemoryManager::bootstrap(
            init_info,
            root.root_descriptor,
            root.root_radix,
            root.bootstrap_generic,
        )?;
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
        let mut chunk = start_slot / PROCESS_FRAME_CHUNK_PAGES;
        let last_chunk = (end_slot - 1) / PROCESS_FRAME_CHUNK_PAGES;
        while chunk <= last_chunk {
            if !self.processes.has_frame_chunk(pid, chunk) {
                arch::generic::convert(
                    self.root.bootstrap_generic,
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

    fn allocate_process_frames(
        &mut self,
        pid: usize,
        process_root: CapabilityDescriptor,
        start_slot: usize,
        page_count: usize,
    ) -> Result<(), CapabilityError> {
        self.ensure_process_frame_chunks(pid, process_root, start_slot, page_count)?;
        let mut done = 0usize;
        while done < page_count {
            let global_slot = start_slot + done;
            let chunk = global_slot / PROCESS_FRAME_CHUNK_PAGES;
            let chunk_offset = global_slot % PROCESS_FRAME_CHUNK_PAGES;
            let chunk_remaining = PROCESS_FRAME_CHUNK_PAGES - chunk_offset;
            let batch = chunk_remaining.min(page_count - done);
            self.memory.allocate_process_frames(
                process_frame_chunk_descriptor(process_root, chunk),
                PROCESS_FRAME_NODE_RADIX,
                chunk_offset,
                batch,
            )?;
            done += batch;
        }
        Ok(())
    }

    fn spawn_components_from_initramfs(&mut self) {
        info!("[proc] scan initramfs");
        let mut spawned = 0usize;
        let mut failed = 0usize;

        let result = cpio::for_each_newc_entry(INITRAMFS_IMAGE, |entry| {
            if !entry.name.ends_with(".elf") {
                return Ok(());
            }
            match self.spawn_process_from_elf(entry.name, entry.data) {
                Ok(()) => {
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
                    self.handle_kernel_fault_event(fault);
                    Some((OS_RESPONSE_FATAL, 0, 0))
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
                    Some(self.handle_os_request(request))
                }
            };

            event = if let Some((status, detail0, detail1)) = pending_reply {
                match self
                    .communication
                    .reply_receive_status(status, detail0, detail1)
                {
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
        image_bytes: &[u8],
    ) -> Result<(), CapabilityError> {
        info!("[proc] parse elf: {}", image_name);
        let elf = parse_elf64(image_bytes)?;
        info!(
            "[proc] elf entry={:#018x} segments={:>2}",
            elf.entry_point, elf.segment_count
        );

        let (pid, process_root_slot) = self.processes.alloc_process_slot()?;
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

        info!(
            "[proc] create child root slot={:>3} desc={:#018x}",
            process_root_slot, child_root
        );
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            PROCESS_ROOT_RADIX as Word,
            1,
            self.root.root_descriptor,
            process_root_slot as Word,
        )?;

        info!("[proc] populate child root");
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::ProcessControlBlock,
            14,
            1,
            child_root,
            PROCESS_SLOT_PCB as Word,
        )?;
        arch::node::copy(
            child_root,
            PROCESS_SLOT_OS_PORT as Word,
            self.processes.alpha_entry().os_port,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::AddressSpace,
            0,
            1,
            child_root,
            PROCESS_SLOT_ADDRESS_SPACE as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L3_NODE as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L2_NODE as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            PAGE_TABLE_NODE_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_L1_NODE as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::Node,
            PROCESS_FRAME_DIRECTORY_RADIX as Word,
            1,
            child_root,
            PROCESS_SLOT_FRAME_NODE as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::IpcPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_SERVICE_PORT as Word,
        )?;
        arch::generic::convert(
            self.root.bootstrap_generic,
            CapabilityType::NotificationPort,
            0,
            1,
            child_root,
            PROCESS_SLOT_NOTIFICATION_PORT as Word,
        )?;
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
        let heap_base = align_up(image_end.max(stack_top), PAGE_SIZE);
        let temp_base = TEMP_MAP_BASE + pid * TEMP_MAP_STRIDE;
        let ipc_buffer_va = match elf.ipc_buffer_start {
            Some(va) => va,
            None => {
                error!("[proc.err] missing required symbol __ipc_buffer_start");
                return Err(CapabilityError::InvalidArgument);
            }
        };
        if ipc_buffer_va < image_base
            || ipc_buffer_va >= image_end
            || (ipc_buffer_va & (PAGE_SIZE - 1)) != 0
        {
            error!(
                "[proc.err] invalid __ipc_buffer_start={:#018x} image=[{:#018x}..{:#018x})",
                ipc_buffer_va, image_base, image_end
            );
            return Err(CapabilityError::InvalidArgument);
        }
        let ipc_buffer_frame_slot = (ipc_buffer_va - image_base) / PAGE_SIZE;
        self.ensure_process_frame_chunks(pid, child_root, 0, total_frames)?;
        let ipc_buffer_frame = process_frame_descriptor(child_root, ipc_buffer_frame_slot);
        let ipc_buffer_tls_base = ipc_buffer_va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS;

        debug!(
            "[proc] map plan image=[{:#018x}..{:#018x}) image_pages={:>3} stack_pages={:>3} ipc={:#018x} temp={:#018x}",
            image_base,
            image_end,
            image_pages,
            USER_STACK_PAGES,
            ipc_buffer_va,
            temp_base
        );

        self.allocate_process_frames(pid, child_root, 0, total_frames)?;
        self.processes.ensure_vm_space_for_pid(pid)?;

        let alpha_address_space = self.processes.alpha_entry().address_space;
        let memory = &mut self.memory;
        let processes = &mut self.processes;

        let mut page = 0usize;
        while page < image_pages {
            let frame = process_frame_descriptor(child_root, page);
            let user_va = image_base + page * PAGE_SIZE;
            let temp_va = temp_base + page * PAGE_SIZE;
            {
                let vm = processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                memory.map_frame(child_address_space, frame, user_va, vm)?;
            }
            {
                let vm = processes.alpha_vm_space_mut();
                memory.map_frame(alpha_address_space, frame, temp_va, vm)?;
            }
            unsafe {
                ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
            }
            page += 1;
        }

        let mut sp = 0usize;
        while sp < USER_STACK_PAGES {
            let frame = process_frame_descriptor(child_root, image_pages + sp);
            let user_va = USER_STACK_BASE + sp * PAGE_SIZE;
            let temp_va = temp_base + (image_pages + sp) * PAGE_SIZE;
            {
                let vm = processes
                    .vm_space_mut(pid)
                    .ok_or(CapabilityError::InvalidArgument)?;
                memory.map_frame(child_address_space, frame, user_va, vm)?;
            }
            {
                let vm = processes.alpha_vm_space_mut();
                memory.map_frame(alpha_address_space, frame, temp_va, vm)?;
            }
            unsafe {
                ptr::write_bytes(temp_va as *mut u8, 0, PAGE_SIZE);
            }
            sp += 1;
        }

        let mut si = 0usize;
        while si < elf.segment_count {
            let seg = elf.segments[si];
            if seg.memory_size == 0 {
                si += 1;
                continue;
            }
            let copy_va = temp_base + (seg.virtual_address - image_base);
            unsafe {
                ptr::copy_nonoverlapping(
                    image_bytes.as_ptr().add(seg.offset),
                    copy_va as *mut u8,
                    seg.file_size,
                );
                if seg.memory_size > seg.file_size {
                    ptr::write_bytes(
                        (copy_va + seg.file_size) as *mut u8,
                        0,
                        seg.memory_size - seg.file_size,
                    );
                }
            }
            si += 1;
        }

        let ipc_buffer_temp_va = temp_base + (ipc_buffer_va - image_base);
        unsafe {
            ptr::write(
                (ipc_buffer_temp_va + (nun::TLS_BASE_OFFSET as usize) * nun::BYTE_BITS)
                    as *mut Word,
                ipc_buffer_va as Word,
            );
        }

        let config = nun::capability_call::process_control_block::ConfigurationInfo::new(
            true,  // address_space
            true,  // root_node
            true,  // frame_ipc_buffer
            true,  // notification_port
            true,  // ipc_port_resolver
            true,  // instruction_pointer
            true,  // stack_pointer
            true,  // thread_local_base
            true,  // priority
            false, // affinity
        );

        debug!(
            "[proc] configure pcb={:#018x} root={:#018x} as={:#018x} ip={:#018x} sp={:#018x}",
            child_pcb, child_root, child_address_space, elf.entry_point, stack_top
        );
        let priority = process_priority_for_image(image_name);
        arch::process_control_block::configure(
            child_pcb,
            config,
            child_address_space,
            child_root,
            ipc_buffer_frame,
            child_notification,
            child_os_port,
            elf.entry_point,
            stack_top,
            ipc_buffer_tls_base,
            priority,
            0,
        )?;

        processes.install_process(
            pid,
            child_root,
            child_pcb,
            child_address_space,
            child_os_port,
            pid as Word,
            total_frames,
            heap_base,
            USER_HEAP_LIMIT,
        )?;
        arch::process_control_block::resume(child_pcb)?;
        info!(
            "[proc] child resumed image={} pid={:>3} priority={:>2} root={:#018x} entry={:#018x}",
            image_name, pid, priority, child_root, elf.entry_point
        );
        Ok(())
    }

    fn handle_kernel_fault_event(&mut self, fault: KernelFaultEvent) {
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
            return;
        }
        let Some(entry) = self.processes.find_entry_by_pid(pid) else {
            error!("[fault] unknown sender id={:>3}, no entry", pid);
            return;
        };
        if entry.pcb == 0 {
            error!("[fault] pid={:>3} has no active pcb", pid);
            return;
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
            self.root.bootstrap_generic,
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
        info!(
            "[proc] exited pid={:>3} pcb={:#018x} is_ok={} error={:#018x}",
            pid, process_entry.pcb, is_ok, error_value
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
        self.allocate_process_frames(pid, root_node, start_slot, page_count)?;

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

        let process_entry = self
            .processes
            .find_entry_by_pid(pid)
            .ok_or(CapabilityError::InvalidArgument)?;

        let (raw_name, raw_len) = decode_service_name_24(request.arg0, request.arg1, request.arg2)
            .ok_or(CapabilityError::InvalidArgument)?;
        let service_name = core::str::from_utf8(&raw_name[..raw_len])
            .map_err(|_| CapabilityError::InvalidArgument)?;
        let service_slot = request.arg3;
        validate_process_device_slot(service_slot)?;
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
        let mut selected: Option<(usize, usize)> = None;
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

            match selected {
                None => selected = Some((i, size_bytes)),
                Some((_, best_size)) if size_bytes < best_size => selected = Some((i, size_bytes)),
                _ => {}
            }
        }

        let (generic_idx, _) = selected.ok_or(CapabilityError::InvalidArgument)?;
        let g = init_info.generic_list[generic_idx];
        let base_address = g.address as usize;
        let base_page = memory
            .physical_page_index_from_address(base_address)
            .ok_or(CapabilityError::InvalidArgument)?;

        debug!(
            "heap generic idx={:>3} addr={:#018x} size_radix={:>2} pages={:>4}",
            generic_idx, base_address, g.size_radix, ALPHA_HEAP_PAGES
        );

        let mut i = 0usize;
        while i < ALPHA_HEAP_PAGES {
            memory.ensure_alpha_frame_at_physical_index(base_page + i)?;
            i += 1;
        }

        let mut j = 0usize;
        while j < ALPHA_HEAP_PAGES {
            let frame = memory
                .physical_frame_descriptor_from_index(base_page + j)
                .ok_or(CapabilityError::InvalidArgument)?;
            let va = ALPHA_HEAP_BASE + j * PAGE_SIZE;
            memory.map_frame(alpha_address_space, frame, va, alpha_vm_space)?;
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

        self.memory.allocate_process_frames(
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
    match basename(image_name) {
        // Timer must preempt clients promptly; animation and network timeouts depend on it.
        "timer-server.elf" => PROCESS_PRIORITY_TIMER_SERVER,
        // Input pipeline must stay above the compositor and every input consumer.
        "input-server.elf" | "ps2-server.elf" => PROCESS_PRIORITY_INPUT_SERVER,
        // GUI servers are above GUI clients, but below timer/input IRQ-facing services.
        "fb-server.elf" | "honoka.elf" => PROCESS_PRIORITY_GUI_SERVER,
        // Background servers stay above clients, but below the GUI critical path.
        "virtio-net.elf" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "net-server.elf" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "rtc-server.elf" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "http-server.elf" => PROCESS_PRIORITY_BACKGROUND_SERVER,
        "honoka-client.elf" | "eg-test.elf" | "image-viewer.elf" => {
            PROCESS_PRIORITY_INTERACTIVE_CLIENT
        }
        "shell.elf" => PROCESS_PRIORITY_CLIENT,
        "cpp-hello.elf" | "rust-hello.elf" => PROCESS_PRIORITY_BACKGROUND_CLIENT,
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
