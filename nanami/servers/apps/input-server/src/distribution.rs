use super::*;

const _: () = assert!(MAX_SUBSCRIBERS <= Word::BITS as usize);

pub(super) fn drain_driver_queues(state: &mut InputState) -> usize {
    let mut delivered_total = 0usize;
    let mut notifications = 0;
    let mut i = 0usize;
    while i < MAX_DRIVER_QUEUES {
        if state.driver_queues[i].used && state.driver_queues[i].local_vaddr != 0 {
            let mask = state.driver_queues[i].event_mask;
            let queue = state.driver_queues[i].local_vaddr;
            let mut budget = 0usize;
            while budget < 512 {
                let packed = match pop_shared_event(queue) {
                    Some(value) => value,
                    None => break,
                };
                let event_kind = packed & 0xff;
                if mask & mask_for_event_kind(event_kind) != 0 {
                    let (delivered, pending) = enqueue_event(state, event_kind, packed);
                    delivered_total = delivered_total.wrapping_add(delivered);
                    notifications |= pending;
                    state.published_count = state.published_count.wrapping_add(1);
                }
                budget += 1;
            }
        }
        i += 1;
    }
    // Publish all events before waking consumers. Always notify at the end of this
    // batch, even if a consumer on another core drained the shared queue meanwhile.
    notify_subscribers(state, notifications);
    delivered_total
}

pub(super) fn distribute_event(state: &mut InputState, event_kind: Word, packed: Word) -> usize {
    let (delivered, notifications) = enqueue_event(state, event_kind, packed);
    notify_subscribers(state, notifications);
    delivered
}

fn enqueue_event(state: &mut InputState, event_kind: Word, packed: Word) -> (usize, Word) {
    let required_mask = mask_for_event_kind(event_kind);
    let mut delivered = 0usize;
    let mut notifications = 0;
    let mut i = 0usize;
    while i < MAX_SUBSCRIBERS {
        if state.subscribers[i].used && (state.subscribers[i].event_mask & required_mask) != 0 {
            let should_notify = if state.subscribers[i].shared_queue_local != 0 {
                distribute_shared_event(&mut state.subscribers[i], event_kind, packed)
            } else {
                distribute_local_event(&mut state.subscribers[i], event_kind, packed)
            };
            if should_notify && state.subscribers[i].notification_descriptor != 0 {
                notifications |= 1 << i;
            }
            delivered += 1;
            state.delivered_count = state.delivered_count.wrapping_add(1);
        }
        i += 1;
    }
    (delivered, notifications)
}

fn notify_subscribers(state: &InputState, mut pending: Word) {
    while pending != 0 {
        let index = pending.trailing_zeros() as usize;
        let _ =
            libnanami::ipc::notification_notify(state.subscribers[index].notification_descriptor);
        pending &= pending - 1;
    }
}

fn distribute_local_event(subscriber: &mut Subscriber, event_kind: Word, packed: Word) -> bool {
    let was_empty = subscriber.queue.count == 0;
    subscriber.queue.push_with_event_kind(event_kind, packed);
    event_kind != nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE || was_empty
}

fn distribute_shared_event(subscriber: &mut Subscriber, event_kind: Word, packed: Word) -> bool {
    push_shared_event_with_kind(subscriber.shared_queue_local, event_kind, packed);

    if event_kind != nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE {
        subscriber.shared_mouse_since_notify = 0;
        return true;
    }

    subscriber.shared_mouse_since_notify = subscriber.shared_mouse_since_notify.wrapping_add(1);
    true
}

pub(super) fn mask_for_event_kind(event_kind: Word) -> Word {
    match event_kind {
        nanami_services::input::INPUT_EVENT_KIND_KEY => {
            nanami_services::input::INPUT_SUBSCRIBE_KEYBOARD
        }
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL => {
            nanami_services::input::INPUT_SUBSCRIBE_MOUSE
        }
        _ => 0,
    }
}
