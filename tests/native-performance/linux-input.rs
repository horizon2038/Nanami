//! Production absolute normalization and event ingestion, including queue overflow.
#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as nanami_runtime;
pub use std::println;
type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/console.rs"]
mod codec;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/input_device.rs"]
mod input_device;
mod state {
    pub const ALTER_EVDEV_QUEUE_CAPACITY: usize = 128;
}
mod input {
    pub use nanami_uapi::input::*;
    pub fn unpack_input_event(_: usize) -> (usize, usize, i16, i16, usize) {
        unreachable!()
    }
}
mod honoka {
    pub const HONOKA_INPUT_FLAG_ABSOLUTE: usize = 1;
    pub struct WindowEventQueue;
    impl WindowEventQueue {
        pub fn new(_: usize) -> Self {
            Self
        }
        pub fn pop(&mut self) -> Option<usize> {
            None
        }
    }
    pub fn honoka_resize_viewport(_: usize, _: usize, _: usize, _: usize) -> Result<(), i32> {
        unreachable!()
    }
}
mod common {
    pub mod signals {
        pub const SIGTERM: usize = 15;
        pub struct Info;
        impl Info {
            pub fn new(_: usize, _: usize, _: usize) -> Self {
                Self
            }
        }
    }
}
mod signals_runtime {
    pub fn queue_group(
        _: &mut super::Runtime,
        _: usize,
        _: super::common::signals::Info,
    ) -> Result<(), i32> {
        unreachable!()
    }
}
mod console {
    pub fn device(_: super::LinuxFile) -> Option<()> {
        None
    }
    pub fn read(
        _: &mut super::Runtime,
        _: usize,
        _: super::LinuxFile,
        _: usize,
        _: usize,
    ) -> Result<usize, i32> {
        unreachable!()
    }
}
mod process {
    pub fn write_personality_syscall_return(
        _: usize,
        _: super::LinuxSyscallContext,
        _: isize,
        _: usize,
    ) -> Result<(), i32> {
        unreachable!()
    }
}
mod arch {
    pub mod process_control_block {
        pub fn resume(_: usize) -> Result<(), i32> {
            unreachable!()
        }
    }
}
const EAGAIN: i32 = 11;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const ENODEV: i32 = 19;
const ENOTTY: i32 = 25;
const LINUX_EVIOCGID: usize = 0x8008_4502;
const LINUX_EVIOCGVERSION: usize = 0x8004_4501;
const LINUX_EV_KEY: u16 = 1;
const LINUX_EV_REL: u16 = 2;
const LINUX_EV_SYN: u16 = 0;
const LINUX_INPUT_EVENT_BYTES: usize = 24;
const LINUX_REL_WHEEL: u16 = 8;
const LINUX_REL_X: u16 = 0;
const LINUX_REL_Y: u16 = 1;
const LINUX_SYN_REPORT: u16 = 0;
fn write_target_memory(_: &Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    unreachable!()
}
unsafe fn write_u16(_: Word, _: u16) {
    unreachable!()
}
unsafe fn write_u32(_: Word, _: u32) {
    unreachable!()
}
unsafe fn write_u64(_: Word, _: u64) {
    unreachable!()
}
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    Empty,
    EvdevMouse,
    EvdevKeyboard,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    resource: Word,
}
#[derive(Clone, Copy)]
struct LinuxSyscallContext;
impl LinuxSyscallContext {
    const EMPTY: Self = Self;
}
#[derive(Clone, Copy)]
struct Process {
    pid: Word,
    pcb: Word,
    personality: Word,
    device_read_waiting: bool,
    device_read_fd: Word,
    device_read_buffer: Word,
    device_read_len: Word,
    device_read_context: LinuxSyscallContext,
}
struct Graphics {
    active: bool,
    root_pid: Word,
    input_queue: Word,
    honoka_port: Word,
    window_id: Word,
    width: Word,
    height: Word,
    mouse_x: i32,
    mouse_y: i32,
    mouse_position_valid: bool,
    console: codec::Console,
    keyboard_events: [Word; 128],
    keyboard_head: usize,
    keyboard_tail: usize,
    mouse_events: [Word; 128],
    mouse_head: usize,
    mouse_tail: usize,
}
impl Graphics {
    fn new() -> Self {
        Self {
            active: true,
            root_pid: 1,
            input_queue: 0,
            honoka_port: 0,
            window_id: 1,
            width: 800,
            height: 600,
            mouse_x: 0,
            mouse_y: 0,
            mouse_position_valid: false,
            console: codec::Console::EMPTY,
            keyboard_events: [0; 128],
            keyboard_head: 0,
            keyboard_tail: 0,
            mouse_events: [0; 128],
            mouse_head: 0,
            mouse_tail: 0,
        }
    }
}
struct Runtime {
    input_queue: Word,
    graphics: [Graphics; 2],
    managed: [Process; 0],
    posix_shm: Word,
    posix_shm_size: Word,
}
impl Runtime {
    fn new() -> Self {
        Self {
            input_queue: 0,
            graphics: [Graphics::new(), Graphics::new()],
            managed: [],
            posix_shm: 0,
            posix_shm_size: 0,
        }
    }
    fn push_keyboard_event(&mut self, _: Word) {
        unreachable!()
    }
    fn push_mouse_event(&mut self, _: Word) {
        unreachable!()
    }
    fn pop_keyboard_event(&mut self) -> Option<Word> {
        unreachable!()
    }
    fn pop_mouse_event(&mut self) -> Option<Word> {
        unreachable!()
    }
    fn keyboard_event_ready(&self) -> bool {
        unreachable!()
    }
    fn mouse_event_ready(&self) -> bool {
        unreachable!()
    }
    fn linux_file(&self, _: Word, _: Word) -> Option<LinuxFile> {
        None
    }
    fn is_signal_stopped(&self, _: Word) -> bool {
        false
    }
}
fn absolute(runtime: &mut Runtime, id: Word, x: i32, y: i32) {
    let (dx, dy) = input_device::normalize_mouse_movement(runtime, id, x, y, 1);
    for (kind, code, value) in [(2, 0, dx), (2, 1, dy), (0, 0, 0)] {
        input_device::push_mouse_event(
            runtime,
            id,
            input_device::pack_linux_input_event(kind, code, value),
        );
    }
}
#[test]
fn evdev_overflow_cannot_drop_legacy_mouse_displacement() {
    let mut runtime = Runtime::new();
    let mouse = &mut runtime.graphics[0].console.mouse;
    mouse.announce();
    while mouse.pop().is_some() {}
    runtime.graphics[0].console.display_mode = 1;
    // Deliberately exceed both the byte queue and the evdev ring before a read.
    for pass in 0..4 {
        for x in 0..800 {
            let x = if pass % 2 == 0 { x } else { 799 - x };
            absolute(&mut runtime, 1, x, 123);
        }
    }
    absolute(&mut runtime, 1, 731, 417);
    let mut bytes = Vec::new();
    while let Some(byte) = runtime.graphics[0].console.mouse.pop() {
        bytes.push(byte);
    }
    assert_eq!(bytes.len() % 3, 0);
    let dx: i32 = bytes.chunks(3).map(|p| p[1] as i8 as i32).sum();
    let dy: i32 = bytes.chunks(3).map(|p| -(p[2] as i8 as i32)).sum();
    assert_eq!((400 + dx, 300 + dy), (731, 417));
    let mut pointer = (400, 300);
    for packet in bytes.chunks(3) {
        pointer.0 = (pointer.0 + packet[1] as i8 as i32).clamp(0, 799);
        pointer.1 = (pointer.1 - packet[2] as i8 as i32).clamp(0, 599);
    }
    assert_eq!(pointer, (731, 417));
    // The independent evdev reader still has records, not duplicated PS/2 bytes.
    assert!(input_device::mouse_event_ready(&runtime, 1));
    assert!(!runtime.graphics[0].console.mouse.ready());
    assert!(!runtime.graphics[1].console.mouse.ready());
}
#[test]
fn oversized_viewport_edges_and_reentry_do_not_advance_the_baseline_outside_x() {
    let mut runtime = Runtime::new();
    runtime.graphics[0].console.display_mode = 1;
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 1, 1200, 900, 1),
        (399, 299)
    );
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 1, 700, 500, 1),
        (-99, -99)
    );
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 1, -100, -100, 1),
        (-700, -500)
    );
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 1, 400, 300, 1),
        (400, 300)
    );
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 2, 400, 300, 1),
        (0, 0)
    );
    assert_eq!(
        input_device::normalize_mouse_movement(&mut runtime, 1, -3, 2, 0),
        (-3, 2)
    );
}
