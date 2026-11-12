//! Read-only authority bound to an active open description and one delegate.
//! The token's monotonically increasing serial prevents handle-slot reuse.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Grant {
    token: Word,
    peer: Word,
    delegate: Word,
}
impl Grant {
    pub(super) const EMPTY: Self = Self { token: 0, peer: 0, delegate: 0 };
}
const SLOT_BITS: u32 = 6;
const SLOT_MASK: Word = (1 << SLOT_BITS) - 1;

pub(super) fn issue(runtime: &mut Ext2Runtime, handle: usize, owner: Word, delegate: Word) -> Word {
    let Some(buffer) = delegate.checked_sub(1).and_then(|index| runtime.delegated_sessions.get(index))
        .filter(|buffer| buffer.active && buffer.owner_pid == owner && buffer.peer_pid != 0)
    else { return 0 };
    let peer = buffer.peer_pid;
    let Some(file) = runtime.handles.get(handle).filter(|file|
        file.active && file.owner_pid == owner && file.mode & EXT2_S_IFMT == EXT2_S_IFREG)
    else { return 0 };
    let old = file.read_grant;
    if old.token != 0 && old.peer == peer && old.delegate == delegate { return old.token; }
    let serial = runtime.next_read_grant;
    // Leave eight bits for the open reply's type. Exhaustion disables grants;
    // it must never wrap or affect an otherwise valid ordinary open.
    if handle > SLOT_MASK || serial == 0 || serial > Word::MAX >> (SLOT_BITS + 8) { return 0; }
    runtime.next_read_grant = serial + 1;
    let token = (serial << SLOT_BITS) | handle;
    runtime.handles[handle].read_grant = Grant { token, peer, delegate };
    token
}

pub(super) fn resolve(runtime: &Ext2Runtime, peer: Word, token: Word) -> Option<(usize, ClientSession)> {
    if token == 0 { return None; }
    let index = token & SLOT_MASK;
    let file = runtime.handles.get(index)?;
    let grant = file.read_grant;
    if !file.active || grant.token != token || grant.peer != peer || peer == 0 { return None; }
    let buffer = runtime.delegated_sessions.get(grant.delegate.checked_sub(1)?)?;
    if !buffer.active || buffer.owner_pid != file.owner_pid || buffer.peer_pid != peer { return None; }
    Some((index, ClientSession { active: true, pid: peer, shm_local: buffer.shm_local, shm_size: buffer.shm_size }))
}
