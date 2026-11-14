//! Production ARP/IP/TCP-send and HTTP reactor code with IPC/device I/O mocked.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;

use std::cell::RefCell;
use std::collections::VecDeque;
pub use std::print;
pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const OS_RESPONSE_FATAL: Word = 5;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Status(Word),
    InvalidArgument,
    Transport,
    Protocol,
}
pub struct NanamiError(pub Word);
pub type NanamiResult = Result<(), NanamiError>;
pub mod debug {
    pub fn print_char(c: char) {
        print!("{c}");
    }
}

#[derive(Default)]
struct Fake {
    shm: Word,
    send_error: Option<RequestError>,
    sent: Vec<(Word, Vec<u8>, Word)>,
    incoming: VecDeque<(Word, Vec<u8>)>,
    sleeps: Vec<Word>,
    polls: usize,
    waits: usize,
    recv_calls: usize,
    send_limit: Option<usize>,
    logs: usize,
    segment_fail_after: Option<usize>,
    segments: Vec<(u32, u8, usize)>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }

pub mod ipc {
    use super::*;
    #[derive(Default)]
    pub struct ServiceRequest {
        pub identifier: Word,
        pub arg0: Word,
        pub arg1: Word,
        pub arg2: Word,
        pub arg3: Word,
    }
    pub fn notification_wait(_: Word) -> Result<Word, RequestError> {
        FAKE.with(|fake| fake.borrow_mut().waits += 1);
        Ok(1)
    }
}

pub mod net {
    use super::*;
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../nanami/servers/uapi/src/net.rs"
    ));

    pub fn net_device_recv(_: Word, offset: Word, _: Word) -> Result<Word, RequestError> {
        backend::receive(offset, 1, false)
    }
    pub fn net_device_recv_batch(_: Word, offset: Word, slots: Word) -> Result<Word, RequestError> {
        backend::receive(offset, slots, true)
    }

    pub fn net_service_tcp_send_on_connection(
        _: Word,
        id: Word,
        offset: Word,
        len: Word,
        flags: Word,
    ) -> Result<Word, RequestError> {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            if let Some(error) = fake.send_error {
                return Err(error);
            }
            let accepted = fake.send_limit.unwrap_or(len).min(len);
            let data = if accepted == 0 {
                Vec::new()
            } else {
                unsafe {
                    std::slice::from_raw_parts((fake.shm + offset) as *const u8, accepted).to_vec()
                }
            };
            fake.sent
                .push((id, data, if accepted == len { flags } else { flags & !1 }));
            Ok(accepted)
        })
    }
    pub fn net_service_tcp_recv_ex(
        _: Word,
        _: Word,
        offset: Word,
        max: Word,
    ) -> Result<(Word, Word), RequestError> {
        net_service_tcp_recv_on_connection(0, 0, 0, offset, max)
    }
    pub fn net_service_tcp_recv_on_connection(
        _: Word,
        connection: Word,
        _: Word,
        offset: Word,
        max: Word,
    ) -> Result<(Word, Word), RequestError> {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.recv_calls += 1;
            let index = if connection == 0 {
                0
            } else {
                let Some(index) = fake.incoming.iter().position(|(id, _)| *id == connection) else {
                    return Ok((0, 0));
                };
                index
            };
            let Some((id, data)) = fake.incoming.remove(index) else {
                return Ok((0, 0));
            };
            assert!(data.len() <= max);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    (fake.shm + offset) as *mut u8,
                    data.len(),
                );
            }
            Ok((data.len(), id))
        })
    }
    pub fn net_service_control(
        _: Word,
        command: Word,
        _: Word,
        _: Word,
    ) -> Result<(), RequestError> {
        assert_eq!(command, NET_SERVICE_CONTROL_POLL);
        FAKE.with(|fake| fake.borrow_mut().polls += 1);
        Ok(())
    }
}

mod backend;
mod backend_connect;
mod http;
mod neighbors;
mod tcp;
mod virtio;
mod virtio_tx;
