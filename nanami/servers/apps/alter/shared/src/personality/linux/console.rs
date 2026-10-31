//! Linux VT/mediumraw and PS/2 compatibility for an explicitly granted window.
use super::*;
use crate::common::console::{keymap, mediumraw, Console};

const VT_OPENQRY: Word = 0x5600;
const VT_GETMODE: Word = 0x5601;
const VT_SETMODE: Word = 0x5602;
const VT_GETSTATE: Word = 0x5603;
const VT_RELDISP: Word = 0x5605;
const VT_ACTIVATE: Word = 0x5606;
const VT_WAITACTIVE: Word = 0x5607;
const KDGETLED: Word = 0x4b31;
const KDSETLED: Word = 0x4b32;
const KDGKBTYPE: Word = 0x4b33;
const KDSETMODE: Word = 0x4b3a;
const KDGETMODE: Word = 0x4b3b;
const KDGKBMODE: Word = 0x4b44;
const KDSKBMODE: Word = 0x4b45;
const KDGKBENT: Word = 0x4b46;
const KDGKBLED: Word = 0x4b64;
const KDSKBLED: Word = 0x4b65;
const K_XLATE: u32 = 1;
const K_MEDIUMRAW: u32 = 2;

pub(super) fn device(file: LinuxFile) -> Option<VirtualNode> {
    if !matches!(
        file.kind,
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse
    ) {
        return None;
    }
    match VirtualNode::from_id(file.resource & 0xffff_ffff)? {
        node @ (VirtualNode::DevConsole | VirtualNode::DevPs2Mouse) => Some(node),
        _ => None,
    }
}
fn state(runtime: &mut Runtime, file: LinuxFile) -> Result<&mut Console, i32> {
    let index = (file.resource >> 32).checked_sub(1).ok_or(ENODEV)?;
    runtime
        .graphics
        .get_mut(index)
        .filter(|session| session.active)
        .map(|session| &mut session.console)
        .ok_or(ENODEV)
}
pub(super) fn open(runtime: &mut Runtime, file: LinuxFile) -> Result<(), i32> {
    if device(file) == Some(VirtualNode::DevPs2Mouse) {
        // Window creation can publish a clamped pointer position before X has
        // initialized its pointer. Like opening mousedev, start from new input
        // rather than replaying pre-open movement against X's reset origin.
        pump_input_events(runtime);
        let index = (file.resource >> 32).checked_sub(1).ok_or(ENODEV)?;
        let session = runtime
            .graphics
            .get_mut(index)
            .filter(|session| session.active)
            .ok_or(ENODEV)?;
        if session.console.mouse.announce() {
            session.mouse_head = session.mouse_tail;
            session.mouse_position_valid = false;
        }
    }
    Ok(())
}
pub(super) fn ready(runtime: &Runtime, file: LinuxFile) -> bool {
    let id = file.resource >> 32;
    if device(file) == Some(VirtualNode::DevPs2Mouse) {
        runtime
            .graphics
            .get(id.wrapping_sub(1))
            .is_some_and(|session| session.active && session.console.mouse.ready())
    } else {
        keyboard_event_ready(runtime, id)
    }
}
pub(super) fn writable(runtime: &Runtime, file: LinuxFile) -> bool {
    device(file) == Some(VirtualNode::DevPs2Mouse)
        && runtime
            .graphics
            .get((file.resource >> 32).wrapping_sub(1))
            .is_some_and(|session| session.active && session.console.mouse.writable())
}
pub(super) fn read(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    output: Word,
    len: Word,
) -> Result<Word, i32> {
    if len == 0 {
        return Ok(0);
    }
    if output == 0 {
        return Err(EFAULT);
    }
    let len = len.min(runtime.posix_shm_size);
    let id = file.resource >> 32;
    pump_input_events(runtime);
    let mut written = 0;
    while written < len {
        let byte = if device(file) == Some(VirtualNode::DevConsole) {
            if state(runtime, file)?.keyboard_mode != K_MEDIUMRAW {
                return Err(EAGAIN);
            }
            let Some(event) = pop_keyboard_event(runtime, id) else {
                break;
            };
            let (kind, code, value) = unpack_linux_input_event(event);
            if kind != LINUX_EV_KEY {
                continue;
            }
            let Some(byte) = mediumraw(code, value) else {
                continue;
            };
            byte
        } else {
            let Some(byte) = state(runtime, file)?.mouse.pop() else {
                break;
            };
            byte
        };
        unsafe {
            *((runtime.posix_shm + written) as *mut u8) = byte;
        }
        written += 1;
    }
    if written == 0 {
        return Err(EAGAIN);
    }
    write_target_memory(runtime, pid, output, written)?;
    runtime.readiness_changes |= crate::state::readiness::READY_INPUT;
    Ok(written)
}
pub(super) fn write(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    input: Word,
    len: Word,
) -> Result<Word, i32> {
    if device(file) != Some(VirtualNode::DevPs2Mouse) {
        return Err(EOPNOTSUPP);
    }
    if len == 0 {
        return Ok(0);
    }
    let len = len.min(runtime.posix_shm_size);
    read_target_memory(runtime, pid, input, len)?;
    let mut accepted = 0;
    while accepted < len {
        let byte = unsafe { *((runtime.posix_shm + accepted) as *const u8) };
        if !state(runtime, file)?.mouse.command(byte) {
            break;
        }
        accepted += 1;
    }
    if accepted == 0 {
        return Err(EAGAIN);
    }
    runtime.readiness_changes |= crate::state::readiness::READY_INPUT;
    Ok(accepted)
}
fn reply(runtime: &Runtime, pid: Word, output: Word, bytes: &[u8]) -> Result<Word, i32> {
    if output == 0 {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), runtime.posix_shm as *mut u8, bytes.len());
    }
    write_target_memory(runtime, pid, output, bytes.len())?;
    Ok(0)
}
pub(super) fn ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    if device(file) != Some(VirtualNode::DevConsole) {
        return Err(ENOTTY);
    }
    match request {
        VT_OPENQRY => reply(runtime, pid, argument, &1u32.to_le_bytes()),
        VT_GETMODE => {
            let mode = state(runtime, file)?.vt_mode;
            reply(runtime, pid, argument, &mode)
        }
        VT_SETMODE => {
            // This window has one VT, never switches the host.
            read_target_memory(runtime, pid, argument, 8)?;
            let mut mode = [0; 8];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    runtime.posix_shm as *const u8,
                    mode.as_mut_ptr(),
                    8,
                );
            }
            if mode[0] > 1 {
                return Err(EINVAL);
            }
            state(runtime, file)?.vt_mode = mode;
            Ok(0)
        }
        VT_GETSTATE => reply(runtime, pid, argument, &[1, 0, 0, 0, 2, 0]),
        VT_RELDISP if argument <= 2 => Ok(0),
        VT_ACTIVATE | VT_WAITACTIVE if argument == 1 => Ok(0),
        KDGETMODE | KDGKBMODE => {
            let value = if request == KDGETMODE {
                state(runtime, file)?.display_mode
            } else {
                state(runtime, file)?.keyboard_mode
            };
            reply(runtime, pid, argument, &value.to_le_bytes())
        }
        KDSETMODE => {
            if argument > 1 {
                return Err(EINVAL);
            }
            state(runtime, file)?.display_mode = argument as u32;
            Ok(0)
        }
        KDSKBMODE => {
            if argument != K_XLATE as Word && argument != K_MEDIUMRAW as Word {
                return Err(EINVAL);
            }
            state(runtime, file)?.keyboard_mode = argument as u32;
            Ok(0)
        }
        KDGKBTYPE => reply(runtime, pid, argument, &[2]), // KB_101
        KDGKBENT => {
            read_target_memory(runtime, pid, argument, 4)?;
            let table = unsafe { *(runtime.posix_shm as *const u8) };
            let key = unsafe { *((runtime.posix_shm + 1) as *const u8) };
            let value = keymap(table, key).to_le_bytes();
            reply(runtime, pid, argument, &[table, key, value[0], value[1]])
        }
        KDGETLED | KDGKBLED => {
            let leds = state(runtime, file)?.leds;
            reply(runtime, pid, argument, &[leds])
        }
        KDSETLED | KDSKBLED => {
            state(runtime, file)?.leds = argument as u8 & 7;
            Ok(0)
        }
        LINUX_TCGETS => {
            let termios = state(runtime, file)?.termios;
            if let Some(termios) = termios {
                return reply(runtime, pid, argument, &termios);
            }
            write_linux_termios(runtime.posix_shm, false, false);
            write_target_memory(runtime, pid, argument, LINUX_TERMIOS_BYTES)?;
            Ok(0)
        }
        LINUX_TCSETS | LINUX_TCSETSW | LINUX_TCSETSF => {
            read_target_memory(runtime, pid, argument, 36)?;
            let mut termios = [0; 36];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    runtime.posix_shm as *const u8,
                    termios.as_mut_ptr(),
                    36,
                );
            }
            state(runtime, file)?.termios = Some(termios);
            Ok(0)
        }
        LINUX_TIOCGWINSZ => reply(runtime, pid, argument, &[24, 0, 80, 0, 0, 0, 0, 0]),
        _ => Err(ENOTTY),
    }
}
