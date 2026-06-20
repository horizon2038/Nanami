use libnanami::Word;

use crate::constants::{
    MAX_KEY_EVENTS_PER_BATCH, MAX_MOUSE_BUTTON_EVENTS_PER_BATCH, PS2_MOUSE_ACK, PS2_MOUSE_RESEND,
};

#[derive(Clone, Copy)]
pub struct ButtonEvent {
    pub code: Word,
    pub pressed: Word,
}

impl ButtonEvent {
    pub const EMPTY: Self = Self {
        code: 0,
        pressed: 0,
    };
}

pub struct MouseBatch {
    pub dx: i32,
    pub dy: i32,
    pub buttons: [ButtonEvent; MAX_MOUSE_BUTTON_EVENTS_PER_BATCH],
    pub button_count: usize,
}

impl MouseBatch {
    pub const fn new() -> Self {
        Self {
            dx: 0,
            dy: 0,
            buttons: [ButtonEvent::EMPTY; MAX_MOUSE_BUTTON_EVENTS_PER_BATCH],
            button_count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dx == 0 && self.dy == 0 && self.button_count == 0
    }

    pub fn clear(&mut self) {
        self.dx = 0;
        self.dy = 0;
        self.button_count = 0;
    }

    pub fn push_button(&mut self, code: Word, pressed: Word) {
        if self.button_count >= MAX_MOUSE_BUTTON_EVENTS_PER_BATCH {
            return;
        }
        self.buttons[self.button_count] = ButtonEvent { code, pressed };
        self.button_count += 1;
    }
}

pub struct KeyboardDecoder {
    e0_prefix: bool,
    ctrl_down: bool,
    alt_down: bool,
    key_down: [bool; 512],
}

impl KeyboardDecoder {
    pub const fn new() -> Self {
        Self {
            e0_prefix: false,
            ctrl_down: false,
            alt_down: false,
            key_down: [false; 512],
        }
    }

    pub fn push_byte(
        &mut self,
        data: u8,
        events: &mut [(Word, Word); MAX_KEY_EVENTS_PER_BATCH],
        event_count: &mut usize,
    ) {
        if data == 0xE0 {
            self.e0_prefix = true;
            return;
        }

        let released = (data & 0x80) != 0;
        let mut code = (data & 0x7f) as Word;
        if self.e0_prefix {
            code |= 0x100;
            self.e0_prefix = false;
        }

        let value = if released { 0 } else { 1 };
        let is_ctrl = code == 0x1d || code == 0x11d;
        let is_alt = code == 0x38 || code == 0x138;
        let is_g = code == 0x22 || code == 0x122;

        if value != 0 && is_g && self.ctrl_down && self.alt_down {
            push_key_event(events, event_count, code, 1);
            push_key_event(events, event_count, code, 0);
            push_key_event(events, event_count, 0x1d, 0);
            push_key_event(events, event_count, 0x38, 0);
            self.ctrl_down = false;
            self.alt_down = false;
            return;
        }

        if value != 0 && !is_ctrl && !is_alt && self.ctrl_down && self.alt_down {
            push_key_event(events, event_count, 0x1d, 0);
            push_key_event(events, event_count, 0x38, 0);
            self.ctrl_down = false;
            self.alt_down = false;
        }

        if is_ctrl {
            self.ctrl_down = value != 0;
        }
        if is_alt {
            self.alt_down = value != 0;
        }

        let key_index = (code & 0x1ff) as usize;
        if value != 0 {
            if self.key_down[key_index] {
                return;
            }
            self.key_down[key_index] = true;
        } else {
            self.key_down[key_index] = false;
        }

        push_key_event(events, event_count, code, value);
    }
}

pub struct MouseDecoder {
    packet: [u8; 3],
    packet_index: usize,
    last_buttons: u8,
}

impl MouseDecoder {
    pub const fn new() -> Self {
        Self {
            packet: [0; 3],
            packet_index: 0,
            last_buttons: 0,
        }
    }

    pub fn push_byte(&mut self, data: u8, batch: &mut MouseBatch, packet_counter: &mut usize) {
        if self.packet_index == 0 && (data == PS2_MOUSE_ACK || data == PS2_MOUSE_RESEND) {
            return;
        }

        if self.packet_index == 0 && (data & 0x08) == 0 {
            return;
        }

        self.packet[self.packet_index] = data;
        self.packet_index += 1;

        if self.packet_index < 3 {
            return;
        }

        self.packet_index = 0;
        let b0 = self.packet[0];
        if (b0 & 0x08) == 0 {
            return;
        }

        let dx = self.packet[1] as i8 as i16 as i32;
        let dy_raw = self.packet[2] as i8 as i16 as i32;
        batch.dx = batch.dx.saturating_add(dx);
        batch.dy = batch.dy.saturating_add(-dy_raw);

        let buttons = b0 & 0x07;
        let changed = buttons ^ self.last_buttons;
        if changed != 0 {
            collect_mouse_button(batch, changed, buttons, 0x01, 1);
            collect_mouse_button(batch, changed, buttons, 0x02, 2);
            collect_mouse_button(batch, changed, buttons, 0x04, 3);
            self.last_buttons = buttons;
        }

        *packet_counter = packet_counter.wrapping_add(1);
    }
}

pub struct Ps2Server {
    pub io_desc: Word,
    pub input_queue_vaddr: Word,
    pub input_notification: Word,
    pub input_sequence: Word,

    pub keyboard: KeyboardDecoder,
    pub mouse: MouseDecoder,

    pub key_events: [(Word, Word); MAX_KEY_EVENTS_PER_BATCH],
    pub key_event_count: usize,

    pub mouse_batch: MouseBatch,

    pub irq1_count: usize,
    pub irq12_count: usize,
    pub key_count: usize,
    pub mouse_packet_count: usize,
    pub published_count: usize,
    pub drain_budget_hits: usize,
}

impl Ps2Server {
    pub const fn new(io_desc: Word, input_queue_vaddr: Word, input_notification: Word) -> Self {
        Self {
            io_desc,
            input_queue_vaddr,
            input_notification,
            input_sequence: 0,
            keyboard: KeyboardDecoder::new(),
            mouse: MouseDecoder::new(),
            key_events: [(0, 0); MAX_KEY_EVENTS_PER_BATCH],
            key_event_count: 0,
            mouse_batch: MouseBatch::new(),
            irq1_count: 0,
            irq12_count: 0,
            key_count: 0,
            mouse_packet_count: 0,
            published_count: 0,
            drain_budget_hits: 0,
        }
    }

    pub fn has_pending_events(&self) -> bool {
        self.key_event_count > 0 || !self.mouse_batch.is_empty()
    }
}

pub enum DrainState {
    Empty,
    ReachedBudget,
}

fn push_key_event(
    events: &mut [(Word, Word); MAX_KEY_EVENTS_PER_BATCH],
    event_count: &mut usize,
    code: Word,
    value: Word,
) {
    if *event_count >= MAX_KEY_EVENTS_PER_BATCH {
        return;
    }
    events[*event_count] = (code, value);
    *event_count += 1;
}

fn collect_mouse_button(batch: &mut MouseBatch, changed: u8, buttons: u8, bit: u8, code: Word) {
    if (changed & bit) == 0 {
        return;
    }
    let pressed = if (buttons & bit) != 0 { 1 } else { 0 };
    batch.push_button(code, pressed);
}
