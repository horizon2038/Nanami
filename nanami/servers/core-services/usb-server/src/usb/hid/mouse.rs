use super::{Event, MouseReport};

pub struct Mouse {
    buttons: u8,
    layout: Option<MouseReport>,
}

impl Mouse {
    pub const fn new() -> Self {
        Self {
            buttons: 0,
            layout: None,
        }
    }

    // Set only after SET_PROTOCOL(Report) succeeds, before submitting interrupt IN.
    pub fn set_report_protocol(&mut self, layout: MouseReport) {
        self.layout = Some(layout);
    }

    pub fn report(&mut self, data: &[u8], mut emit: impl FnMut(Event)) {
        let values = if let Some(layout) = &self.layout {
            let Some(values) = layout.decode(data) else {
                return;
            };
            values
        } else {
            if data.len() < 3 {
                return;
            }
            // Bytes after the boot mouse's X/Y are device-specific, not a wheel.
            [
                Some((data[0] & 1) as i16),
                Some(((data[0] >> 1) & 1) as i16),
                Some(((data[0] >> 2) & 1) as i16),
                Some(data[1] as i8 as i16),
                Some(data[2] as i8 as i16),
                None,
            ]
        };
        for (bit, value) in values[..3].iter().enumerate() {
            if let Some(value) = value {
                let pressed = *value != 0;
                if pressed != (self.buttons & (1 << bit) != 0) {
                    self.buttons ^= 1 << bit;
                    emit(Event {
                        kind: 2,
                        code: bit + 1,
                        x: pressed as i16,
                        y: 0,
                    });
                }
            }
        }
        let x = values[3].unwrap_or(0);
        let y = values[4].unwrap_or(0);
        if x != 0 || y != 0 {
            emit(Event {
                kind: 3,
                code: 0,
                x,
                y,
            });
        }
        let wheel = values[5].unwrap_or(0);
        if wheel != 0 {
            // Positive HID Wheel means up, as does the existing input-service API.
            emit(Event {
                kind: 4,
                code: 0,
                x: wheel,
                y: 0,
            });
        }
    }

    pub fn release(&mut self, mut emit: impl FnMut(Event)) {
        // A synthetic boot packet is not valid for a numbered report layout.
        for bit in 0..3 {
            if self.buttons & (1 << bit) != 0 {
                emit(Event {
                    kind: 2,
                    code: bit + 1,
                    x: 0,
                    y: 0,
                });
            }
        }
        self.buttons = 0;
    }
}
