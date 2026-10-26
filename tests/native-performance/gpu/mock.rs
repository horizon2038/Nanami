//! Host software execution of the real provider protocol, not a hardware model.
use crate::{RequestError, Word};
use nanami_uapi::gpu::*;
use std::{
    alloc::{alloc_zeroed, dealloc, Layout as MemoryLayout},
    cell::RefCell,
    collections::BTreeMap,
};
#[path = "../../../nanami/servers/libs/nanami-gpu/src/lib.rs"]
pub mod provider;
use provider::{Backend, Buffer};
struct Memory {
    pointer: Word,
    bytes: Word,
    refs: usize,
}
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            dealloc(
                self.pointer as *mut u8,
                MemoryLayout::from_size_align(self.bytes, 4096).unwrap(),
            );
        }
    }
}
#[derive(Default)]
pub struct Fake {
    pub accelerated: Word,
    pub fills: usize,
    pub copies: usize,
    pub presents: usize,
    pub fail_work: bool,
    pub fail_release: bool,
    pub dead_owners: std::collections::BTreeSet<Word>,
    pub presented: Vec<u8>,
    pub calls: Vec<(Word, [Word; 4], u8)>,
}
thread_local! {
    static MEMORY: RefCell<BTreeMap<Word, Memory>> = RefCell::new(BTreeMap::new());
    static SERVICE: RefCell<provider::Service> = RefCell::new(provider::Service::default());
    pub static FAKE: RefCell<Fake> = RefCell::new(Fake::default());
    static CALLER: std::cell::Cell<Word> = const { std::cell::Cell::new(42) };
}
pub fn reset() {
    SERVICE.with(|service| *service.borrow_mut() = provider::Service::default());
    MEMORY.with(|memory| memory.borrow_mut().clear());
    FAKE.with(|fake| *fake.borrow_mut() = Fake::default());
    CALLER.with(|caller| caller.set(42));
}
pub fn caller(pid: Word) {
    CALLER.with(|caller| caller.set(pid));
}
pub fn shared(_: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    let layout = MemoryLayout::from_size_align(bytes, 4096).unwrap();
    let pointer = unsafe { alloc_zeroed(layout) } as Word;
    assert_ne!(pointer, 0);
    MEMORY.with(|memory| {
        memory.borrow_mut().insert(
            pointer,
            Memory {
                pointer,
                bytes,
                refs: 2,
            },
        );
    });
    Ok((pointer, pointer))
}
pub fn release_if_owned(address: Word, bytes: Word) -> bool {
    MEMORY.with(|memory| {
        let mut memory = memory.borrow_mut();
        let Some(block) = memory.get_mut(&address) else {
            return false;
        };
        assert_eq!(block.bytes, bytes);
        block.refs -= 1;
        if block.refs == 0 {
            memory.remove(&address);
        }
        true
    })
}
impl Backend for Fake {
    fn owner_alive(&mut self, owner: Word) -> Result<bool, RequestError> {
        Ok(!self.dead_owners.contains(&owner))
    }
    fn accelerated(&self) -> Word {
        self.accelerated
    }
    fn allocate(
        &mut self,
        owner: Word,
        layout: Layout,
    ) -> Result<provider::Allocation, RequestError> {
        let (local, peer) = shared(owner, layout.bytes)?;
        Ok(provider::Allocation {
            local,
            peer,
            gpu: 0,
        })
    }
    fn release(&mut self, buffer: Buffer) -> Result<(), RequestError> {
        if self.fail_release {
            return Err(RequestError::Transport);
        }
        assert!(release_if_owned(
            buffer.allocation.local,
            buffer.layout.bytes
        ));
        Ok(())
    }
    fn fill(&mut self, buffer: Buffer, rect: Rect, color: u32) -> Result<(), RequestError> {
        self.fills += 1;
        if self.fail_work {
            return Err(RequestError::Transport);
        }
        provider::cpu_fill(buffer, rect, color);
        Ok(())
    }
    fn copy(
        &mut self,
        source: Buffer,
        target: Buffer,
        rect: Rect,
        x: Word,
        y: Word,
    ) -> Result<(), RequestError> {
        self.copies += 1;
        if self.fail_work {
            return Err(RequestError::Transport);
        }
        provider::cpu_copy(source, target, rect, x, y);
        Ok(())
    }
    fn present(&mut self, source: Buffer, _: Rect) -> Result<(), RequestError> {
        self.presents += 1;
        if self.fail_work {
            return Err(RequestError::Transport);
        }
        self.presented = unsafe {
            std::slice::from_raw_parts(source.allocation.local as *const u8, source.layout.bytes)
        }
        .to_vec();
        Ok(())
    }
}
pub fn call(code: Word, args: [Word; 4], words: u8) -> Result<(Word, Word, Word), RequestError> {
    let expected = match code {
        GET_INFO => 1,
        CREATE | PRESENT => 4,
        DESTROY => 2,
        FILL | COPY => 5,
        _ => panic!("Unknown GPU operation"),
    };
    assert_eq!(words, expected);
    FAKE.with(|fake| fake.borrow_mut().calls.push((code, args, words)));
    let result = SERVICE.with(|service| {
        FAKE.with(|fake| {
            service.borrow_mut().request(
                CALLER.with(|caller| caller.get()),
                code,
                args,
                &mut *fake.borrow_mut(),
            )
        })
    });
    match result {
        Ok((a, b)) => Ok((0, a, b)),
        Err(RequestError::Status(status)) => Ok((status, 0, 0)),
        Err(RequestError::InvalidArgument) => Ok((1, 0, 0)),
        Err(RequestError::Unsupported) => Ok((4, 0, 0)),
        Err(_) => Ok((5, 0, 0)),
    }
}
