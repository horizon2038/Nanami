use crate::*;
use a9n_types::MessageInfo;
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Mutex, MutexGuard},
};

pub const BOUND: Word = 18;
pub const SERVICE: Word = 20;
static TEST_LOCK: Mutex<()> = Mutex::new(());
#[derive(Default)]
pub struct Fake {
    pub messages: Vec<Word>,
    pub incoming: VecDeque<(MessageInfo, Word)>,
    pub operations: Vec<&'static str>,
    pub replies: Vec<Vec<Word>>,
    pub reply_error: Option<CapabilityError>,
    pub notification: Word,
}
thread_local! { pub static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub fn setup() -> MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE.with(|fake| {
        *fake.borrow_mut() = Fake {
            messages: vec![0; 64],
            ..Fake::default()
        }
    });
    ports::bind_current_thread_notification(BOUND).unwrap();
    guard
}
pub fn incoming(info: MessageInfo, id: Word) {
    FAKE.with(|fake| fake.borrow_mut().incoming.push_back((info, id)));
}
fn receive(info: &mut MessageInfo, id: &mut Word, operation: &'static str) {
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        fake.operations.push(operation);
        (*info, *id) = fake
            .incoming
            .pop_front()
            .expect("kernel receive would block: notification was already consumed by Call");
    });
}
fn reply(info: MessageInfo, operation: &'static str) -> Result<(), CapabilityError> {
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        fake.operations.push(operation);
        if let Some(error) = fake.reply_error.take() {
            return Err(error);
        }
        let payload = fake.messages[4..4 + usize::from(info.message_length())].to_vec();
        fake.replies.push(payload);
        Ok(())
    })
}
pub mod capability_call {
    pub mod ipc_port {
        pub use a9n_types::MessageInfo;
    }
    pub mod process_control_block {
        pub struct ConfigurationInfo;
        impl ConfigurationInfo {
            pub fn new(
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
                _: bool,
            ) -> Self {
                Self
            }
        }
    }
}
pub mod arch {
    use super::*;
    pub mod ipc_buffer {
        use super::*;
        pub struct Buffer;
        pub fn get_ipc_buffer() -> Buffer {
            Buffer
        }
        impl Buffer {
            pub fn get_message(&self, index: usize) -> Word {
                FAKE.with(|fake| fake.borrow().messages[index])
            }
            pub fn configure_message(&self, index: usize, value: Word) {
                FAKE.with(|fake| fake.borrow_mut().messages[index] = value);
            }
        }
    }
    pub mod ipc_port {
        use super::*;
        pub fn receive(
            _: Word,
            info: &mut MessageInfo,
            id: &mut Word,
        ) -> Result<(), CapabilityError> {
            super::super::receive(info, id, "receive");
            Ok(())
        }
        pub fn reply(_: Word, info: MessageInfo) -> Result<(), CapabilityError> {
            super::super::reply(info, "reply")
        }
        pub fn reply_receive(
            _: Word,
            info: &mut MessageInfo,
            id: &mut Word,
        ) -> Result<(), CapabilityError> {
            super::super::reply(*info, "reply_receive")?;
            super::super::receive(info, id, "receive part");
            Ok(())
        }
    }
    pub mod process_control_block {
        use super::*;
        pub fn configure(
            _: Word,
            _: capability_call::process_control_block::ConfigurationInfo,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
            _: Word,
        ) -> Result<(), CapabilityError> {
            Ok(())
        }
    }
    pub mod notification_port {
        use super::*;
        pub fn notify(_: Word) -> Result<(), CapabilityError> {
            Ok(())
        }
        pub fn poll(_: Word) -> Result<Word, CapabilityError> {
            Ok(FAKE.with(|fake| std::mem::take(&mut fake.borrow_mut().notification)))
        }
        pub fn wait(desc: Word) -> Result<Word, CapabilityError> {
            poll(desc)
        }
    }
    pub mod interrupt_port {
        use super::*;
        pub fn ack(_: Word) -> Result<(), CapabilityError> {
            Ok(())
        }
    }
}
