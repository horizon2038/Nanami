use libnanami::Word;

#[derive(Clone, Copy)]
pub enum InputEvent {
    Key { code: Word, pressed: bool },
    MouseMove { dx: i32, dy: i32 },
    MouseButton { code: Word, pressed: bool },
    MouseWheel { delta: i32 },
    Unknown,
}

pub fn decode_input_event(packed: Word) -> InputEvent {
    let (kind, code, value0, value1, _) = nanami_services::input::unpack_input_event(packed);
    match kind {
        nanami_services::input::INPUT_EVENT_KIND_KEY => InputEvent::Key {
            code,
            pressed: value0 != 0,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE => InputEvent::MouseMove {
            dx: value0 as i32,
            dy: value1 as i32,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON => InputEvent::MouseButton {
            code,
            pressed: value0 != 0,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL => InputEvent::MouseWheel {
            delta: value0 as i32,
        },
        _ => InputEvent::Unknown,
    }
}
