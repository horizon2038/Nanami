use super::{RequestError, Word};
use std::{cell::RefCell, collections::VecDeque};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Healthy,
    Absent,
    InputBusy,
    NoConfig,
    NoAck,
    NoMouseId,
    KeyboardFlood,
}

pub struct Hardware {
    pub mode: Mode,
    pub reads: usize,
    pub writes: usize,
    pub sleeps: usize,
    pub clock_reads: usize,
    pub elapsed_ms: Word,
    pub hz: Word,
    pub clock_origin: Word,
    pub fail_timer: bool,
    pub created_notification: Option<Word>,
    output: VecDeque<(Word, Word)>,
    mouse_write: bool,
    waiting_ack: bool,
}

thread_local! {
    pub static HW: RefCell<Hardware> = RefCell::new(Hardware {
        mode: Mode::Healthy, reads: 0, writes: 0, sleeps: 0, clock_reads: 0,
        elapsed_ms: 0, hz: 1_000_000_000, clock_origin: 0, fail_timer: false,
        created_notification: None, output: VecDeque::new(),
        mouse_write: false, waiting_ack: false,
    });
}

pub mod io {
    use super::*;
    pub fn io_read(_: Word, port: Word, _: Word) -> Result<Word, RequestError> {
        HW.with_borrow_mut(|hw| {
            hw.reads += 1;
            assert!(
                hw.reads < 2000,
                "PS/2 is busy-polling, starving lower-priority services"
            );
            if hw.mode == Mode::Absent {
                return Ok(0xff);
            }
            let keyboard = hw.mode == Mode::KeyboardFlood && hw.waiting_ack;
            match port {
                0x64 => Ok(if hw.mode == Mode::InputBusy {
                    2
                } else if keyboard {
                    1
                } else {
                    hw.output.front().map_or(0, |entry| entry.0)
                }),
                0x60 if keyboard => Ok(0x1e),
                0x60 => Ok(hw.output.pop_front().expect("no data available").1),
                _ => panic!("unexpected read"),
            }
        })
    }
    pub fn io_write(_: Word, port: Word, _: Word, value: Word) -> Result<(), RequestError> {
        HW.with_borrow_mut(|hw| {
            hw.writes += 1;
            if port == 0x64 {
                if value == 0x20 && hw.mode != Mode::NoConfig {
                    hw.output.push_back((1, 0));
                }
                hw.mouse_write = value == 0xd4;
            } else if std::mem::take(&mut hw.mouse_write) {
                hw.waiting_ack = true;
                if !matches!(hw.mode, Mode::NoAck | Mode::KeyboardFlood) {
                    hw.output.push_back((0x21, 0xfa));
                    if value == 0xf2 && hw.mode != Mode::NoMouseId {
                        hw.output.push_back((0x21, 0));
                    }
                }
            }
        });
        Ok(())
    }
}

pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod registry {
    use super::*;
    pub fn connect_boot_service(name: &str, slot: Word) -> Result<(), RequestError> {
        assert_eq!(name, "timer-service");
        assert_eq!(slot, 26);
        Ok(())
    }
}
pub fn request_notification_port_create(slot: Word, badge: Word) -> Result<(), RequestError> {
    assert_eq!(slot, 27);
    assert_eq!(badge, timer::TIMER_NOTIFICATION_IDENTIFIER_BIT);
    HW.with_borrow_mut(|hw| hw.created_notification = Some(slot));
    Ok(())
}
pub mod timer {
    use super::*;
    pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word = 1 << (Word::BITS - 1);
    pub fn timer_service_monotonic_ticks(port: Word) -> Result<(Word, Word), RequestError> {
        assert_eq!(port, 26);
        HW.with_borrow_mut(|hw| {
            hw.clock_reads += 1;
            if hw.fail_timer {
                return Err(RequestError::Transport);
            }
            Ok((
                hw.clock_origin.wrapping_add(hw.elapsed_ms * hw.hz / 1000),
                hw.hz,
            ))
        })
    }
    pub fn timer_service_sleep_on_notification_milliseconds(
        port: Word,
        ms: Word,
        slot: Word,
    ) -> Result<(), RequestError> {
        assert_eq!(port, 26);
        assert_eq!(slot, 27, "IRQ notifications must not complete the sleep");
        HW.with_borrow_mut(|hw| {
            hw.sleeps += 1;
            hw.elapsed_ms += ms.div_ceil(1000 / hw.hz.min(1000)) * (1000 / hw.hz.min(1000));
        });
        Ok(())
    }
}
