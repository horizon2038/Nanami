//! Test the production EP0 quiescence policy, not an assumed DMA completion.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::collections::VecDeque;
pub use std::println;

#[derive(Debug, PartialEq)]
enum RequestError {
    Transport,
    Protocol,
}
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/ring.rs"]
mod ring;
use ring::Trb;
const SLOT_BASE: usize = 0x4000;
const SLOT_BYTES: usize = 0x2e000;
const PORTSC: usize = 0x400;
unsafe fn read(base: usize, offset: usize) -> u32 {
    ((base + offset) as *const u32).read_volatile()
}
struct Dma {
    virtual_address: usize,
}
impl Dma {
    fn virtual_at(&self, offset: usize) -> usize {
        self.virtual_address + offset
    }
}
struct Input;
#[derive(Default)]
struct Events {
    finished: usize,
}
impl Events {
    fn finish(&mut self, _: usize) {
        self.finished += 1;
    }
}
struct Controller {
    dma: Dma,
    op: usize,
    context_bytes: usize,
    interrupter: usize,
    events: Events,
    running: bool,
    pending: VecDeque<Trb>,
    commands: Vec<Trb>,
    dispatched: Vec<Trb>,
    command_error: bool,
}
impl Controller {
    fn command(&mut self, trb: Trb, _: &mut Input) -> Result<Trb, RequestError> {
        self.commands.push(trb);
        if self.command_error {
            return Err(RequestError::Transport);
        }
        assert!(matches!(trb.kind(), 14 | 15));
        // Simulate successful command completion updating the HC-owned state.
        unsafe {
            ((self.dma.virtual_at(0x5000 + self.context_bytes)) as *mut u32).write(3);
        }
        Ok(Trb::default())
    }
    fn event(&mut self) -> Option<Trb> {
        self.pending.pop_front()
    }
    fn dispatch(&mut self, event: Trb, _: &mut Input) {
        self.dispatched.push(event);
    }
}
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/control_recovery.rs"]
mod recovery;

fn fixture(state: u32, context_bytes: usize) -> (Box<[u32]>, Box<[u32; 512]>, Controller) {
    let mut dma = vec![0u32; 0x7000 / 4].into_boxed_slice();
    dma[(0x5000 + context_bytes) / 4] = state;
    let op = Box::new([0u32; 512]);
    let c = Controller {
        dma: Dma {
            virtual_address: dma.as_mut_ptr() as usize,
        },
        op: op.as_ptr() as usize,
        context_bytes,
        interrupter: 0,
        events: Events::default(),
        running: true,
        pending: VecDeque::new(),
        commands: Vec::new(),
        dispatched: Vec::new(),
        command_error: false,
    };
    (dma, op, c)
}

#[test]
fn running_control_pipe_is_stopped_and_events_drained_without_stopping_hid() {
    for context_bytes in [32, 64] {
        let (_dma, _op, mut c) = fixture(1, context_bytes);
        let hid = Trb {
            control: 32 << 10 | 3 << 24 | 3 << 16,
            ..Trb::default()
        };
        c.pending.push_back(hid);
        c.stop_control_after_timeout(1, 2, &mut Input).unwrap();
        assert_eq!(c.commands.len(), 1);
        let command = c.commands[0];
        assert_eq!(
            (command.kind(), command.slot(), command.endpoint()),
            (15, 1, 1)
        );
        assert_eq!(c.dispatched, [hid]);
        assert_eq!(c.events.finished, 1);
        assert!(c.running);
    }
}

#[test]
fn halted_pipe_is_reset_and_already_stopped_pipe_needs_no_command() {
    for state in [0, 2, 3] {
        let (_dma, _op, mut c) = fixture(state, 32);
        c.stop_control_after_timeout(1, 2, &mut Input).unwrap();
        assert_eq!(c.commands.len(), usize::from(state == 2));
        if state == 2 {
            assert_eq!(c.commands[0].kind(), 14);
        }
        assert!(c.running);
    }
}

#[test]
fn failed_stop_does_not_claim_ownership_or_continue_cleanup() {
    let (_dma, _op, mut c) = fixture(1, 32);
    c.command_error = true;
    assert_eq!(
        c.stop_control_after_timeout(1, 2, &mut Input),
        Err(RequestError::Transport)
    );
    assert_eq!(c.events.finished, 0);
    assert!(c.dispatched.is_empty());
}

#[test]
fn error_state_requires_controller_fallback_not_unsafe_ring_reuse() {
    let (_dma, _op, mut c) = fixture(4, 32);
    assert_eq!(
        c.stop_control_after_timeout(1, 2, &mut Input),
        Err(RequestError::Protocol)
    );
    assert!(c.commands.is_empty());
    assert_eq!(c.events.finished, 0);
}
