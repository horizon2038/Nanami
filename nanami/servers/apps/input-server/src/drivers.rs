use super::*;

pub(super) fn driver_mask(kind: Word) -> Word {
    match kind {
        nanami_services::input::INPUT_DRIVER_KEYBOARD => {
            nanami_services::input::INPUT_SUBSCRIBE_KEYBOARD
        }
        nanami_services::input::INPUT_DRIVER_MOUSE => nanami_services::input::INPUT_SUBSCRIBE_MOUSE,
        _ => 0,
    }
}

pub(super) fn register_driver(state: &mut InputState, pid: Word, kind: Word) -> Option<usize> {
    let mask = driver_mask(kind);
    if pid == 0 || mask == 0 {
        return None;
    }
    let index = state
        .driver_queues
        .iter()
        .position(|driver| driver.used && driver.pid == pid)
        .or_else(|| state.driver_queues.iter().position(|driver| !driver.used))?;
    let driver = &mut state.driver_queues[index];
    driver.used = true;
    driver.pid = pid;
    driver.event_mask |= mask;
    Some(index)
}

pub(super) fn is_authorized_event_from_pid(pid: Word, kind: Word, state: &InputState) -> bool {
    let mask = mask_for_event_kind(kind);
    state
        .driver_queues
        .iter()
        .any(|driver| driver.used && driver.pid == pid && driver.event_mask & mask != 0)
}
