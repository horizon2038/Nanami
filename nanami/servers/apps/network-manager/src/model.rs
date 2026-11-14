use alloc::{format, string::String};
use nanami_client::net::*;
use nanami_ui::{Field, Key};
pub struct Model {
    pub mode: usize,
    pub fields: [Field; 4],
    pub focus: usize,
    pub revision: usize,
    pub message: String,
    pub active: String,
    pub error: bool,
    pub edited: bool,
}
pub fn ip_text(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}
impl Model {
    pub fn new(settings: Ipv4Settings, revision: usize) -> Self {
        Self {
            mode: settings.mode,
            fields: [
                settings.address,
                settings.netmask,
                settings.gateway,
                settings.dns,
            ]
            .map(|ip| Field::new(&ip_text(ip))),
            focus: 0,
            revision,
            message: String::new(),
            active: String::new(),
            error: false,
            edited: false,
        }
    }
    pub fn settings(&self) -> Result<Ipv4Settings, &'static str> {
        let mut addresses = [[0; 4]; 4];
        for (address, field) in addresses.iter_mut().zip(&self.fields) {
            *address =
                parse_ipv4(&field.text).ok_or("Enter four decimal octets separated by dots")?;
        }
        let settings = Ipv4Settings {
            mode: self.mode,
            address: addresses[0],
            netmask: addresses[1],
            gateway: addresses[2],
            dns: addresses[3],
        };
        settings.validate_apply()?;
        Ok(settings)
    }
    pub fn key(&mut self, key: Key) -> bool {
        if self.mode != IPV4_MODE_MANUAL {
            return false;
        }
        if key == Key::Tab {
            self.focus = (self.focus + 1) % self.fields.len();
            self.fields[self.focus].selected = true;
            return true;
        }
        if matches!(key, Key::Character(c) if !c.is_ascii_digit() && c != '.') {
            return false;
        }
        let before = self.fields[self.focus].text.clone();
        self.fields[self.focus].key(key);
        self.edited |= before != self.fields[self.focus].text;
        true
    }
}
