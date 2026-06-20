use a9n_abi::CapabilityDescriptor;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::{
    INPUT_EVENT_QUEUE_CAPACITY, INPUT_EVENT_QUEUE_HEADER_WORDS, INPUT_EVENT_QUEUE_MAGIC,
    INPUT_SERVICE_REQUEST_ATTACH_DRIVER, INPUT_SERVICE_REQUEST_ATTACH_DRIVER_SHARED,
    INPUT_SERVICE_REQUEST_PUBLISH_EVENT, INPUT_SERVICE_REQUEST_READ_EVENT,
    INPUT_SERVICE_REQUEST_SUBSCRIBE, INPUT_SERVICE_REQUEST_SUBSCRIBE_SHARED,
};

pub fn input_service_subscribe(
    input_service_port: CapabilityDescriptor,
    event_mask: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_SUBSCRIBE,
        event_mask,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn input_service_subscribe_shared(
    input_service_port: CapabilityDescriptor,
    event_mask: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, queue_vaddr, queue_bytes) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_SUBSCRIBE_SHARED,
        event_mask,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((queue_vaddr, queue_bytes))
}

pub fn input_service_read_event(
    input_service_port: CapabilityDescriptor,
) -> Result<(bool, Word), RequestError> {
    let (status, has_event, packed_event) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_READ_EVENT,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((has_event != 0, packed_event))
}

pub fn input_service_attach_driver(
    input_service_port: CapabilityDescriptor,
    driver_kind: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_ATTACH_DRIVER,
        driver_kind,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn input_service_attach_driver_shared(
    input_service_port: CapabilityDescriptor,
    driver_kind: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, queue_vaddr, queue_bytes) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_ATTACH_DRIVER_SHARED,
        driver_kind,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((queue_vaddr, queue_bytes))
}

pub fn input_service_publish_event(
    input_service_port: CapabilityDescriptor,
    event_kind: Word,
    code: Word,
    value0: Word,
    value1: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        input_service_port,
        INPUT_SERVICE_REQUEST_PUBLISH_EVENT,
        event_kind,
        code,
        value0,
        value1,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn pack_input_event(
    event_kind: Word,
    code: Word,
    value0: i16,
    value1: i16,
    flags: Word,
) -> Word {
    let v0 = value0 as u16 as Word;
    let v1 = value1 as u16 as Word;
    (event_kind & 0xff)
        | ((code & 0xffff) << 8)
        | ((v0 & 0xffff) << 24)
        | ((v1 & 0xffff) << 40)
        | ((flags & 0xff) << 56)
}

pub fn unpack_input_event(packed: Word) -> (Word, Word, i16, i16, Word) {
    let event_kind = packed & 0xff;
    let code = (packed >> 8) & 0xffff;
    let value0 = ((packed >> 24) & 0xffff) as u16 as i16;
    let value1 = ((packed >> 40) & 0xffff) as u16 as i16;
    let flags = (packed >> 56) & 0xff;
    (event_kind, code, value0, value1, flags)
}

pub struct InputEventQueue {
    base: Word,
}

impl InputEventQueue {
    pub const fn new(base: Word) -> Self {
        Self { base }
    }

    pub fn is_valid(&self) -> bool {
        self.base != 0
            && self.read_word(0) == super::constants::INPUT_EVENT_QUEUE_MAGIC
            && self.capacity() != 0
    }

    pub fn pop(&mut self) -> Option<Word> {
        if !self.is_valid() {
            return None;
        }

        // Claim head with CAS so the same shared queue remains safe even if
        // multiple consumers are woken and race in a preemptive scheduler.
        let capacity = INPUT_EVENT_QUEUE_CAPACITY;
        loop {
            let head = (self.read_word(2) as usize) % capacity;
            let tail = (self.read_word(3) as usize) % capacity;
            if head == tail {
                return None;
            }

            let value = self.read_word(INPUT_EVENT_QUEUE_HEADER_WORDS + head);
            let next_head = (head + 1) % capacity;
            if self.compare_exchange_word(2, head as Word, next_head as Word) {
                return Some(value);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        if !self.is_valid() {
            return true;
        }
        let capacity = self.capacity() as usize;
        let head = (self.read_word(2) as usize) % capacity;
        let tail = (self.read_word(3) as usize) % capacity;
        head == tail
    }

    pub fn debug_counters(&self) -> (bool, Word, Word, Word) {
        if !self.is_valid() {
            return (false, 0, 0, 0);
        }
        (
            true,
            self.read_word(2),
            self.read_word(3),
            self.read_word(4),
        )
    }

    pub fn init(&self) {
        if self.base == 0 {
            return;
        }
        self.write_word(0, INPUT_EVENT_QUEUE_MAGIC);
        self.write_word(1, INPUT_EVENT_QUEUE_CAPACITY as Word);
        self.write_word(2, 0);
        self.write_word(3, 0);
        self.write_word(4, 0);
    }

    pub fn push_with_event_kind(&mut self, _event_kind: Word, packed: Word) {
        if !self.is_valid() {
            return;
        }

        // Single producer only. Do not rewrite already-published entries:
        // that is not safe once multiple consumers may read the same queue.
        let capacity = self.capacity() as usize;
        let head = (self.read_word(2) as usize) % capacity;
        let tail = (self.read_word(3) as usize) % capacity;
        let next_tail = (tail + 1) % capacity;
        if next_tail == head {
            self.write_word(4, self.read_word(4).wrapping_add(1));
            return;
        }
        self.write_word(INPUT_EVENT_QUEUE_HEADER_WORDS + tail, packed);
        self.write_word(3, next_tail as Word);
    }

    fn capacity(&self) -> Word {
        let capacity = self.read_word(1);
        if capacity == 0 || capacity as usize > INPUT_EVENT_QUEUE_CAPACITY {
            0
        } else {
            capacity
        }
    }

    fn read_word(&self, index: usize) -> Word {
        unsafe {
            let ptr = (self.base as usize + word_offset(index) as usize) as *const AtomicUsize;
            (*ptr).load(Ordering::SeqCst) as Word
        }
    }

    fn write_word(&self, index: usize, value: Word) {
        unsafe {
            let ptr = (self.base as usize + word_offset(index) as usize) as *const AtomicUsize;
            (*ptr).store(value as usize, Ordering::SeqCst);
        }
    }

    fn compare_exchange_word(&self, index: usize, current: Word, next: Word) -> bool {
        unsafe {
            let ptr = (self.base as usize + word_offset(index) as usize) as *const AtomicUsize;
            (*ptr)
                .compare_exchange(
                    current as usize,
                    next as usize,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        }
    }
}

const fn word_offset(index: usize) -> Word {
    (index * core::mem::size_of::<Word>()) as Word
}
