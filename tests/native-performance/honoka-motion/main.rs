//! Pixel and damage regressions using the real compositor, with IPC/font stubs.
#![allow(dead_code)]
extern crate alloc;
extern crate self as a9n_abi;
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
pub type CapabilityDescriptor = Word;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
pub const OS_RESPONSE_OK: Word = 0;
#[derive(Debug)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
    Status(Word),
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
pub struct NanamiError;
impl NanamiError {
    pub const INVALID_ARGUMENT: Self = Self;
}
#[path = "../../../nanami/servers/apps/honoka/src/app/background.rs"]
mod background;
#[path = "../../../nanami/servers/apps/honoka/src/app/compositor.rs"]
mod compositor;
#[path = "../../../nanami/servers/apps/honoka/src/app/constants.rs"]
mod constants;
#[path = "../../../nanami/servers/apps/honoka/src/app/framebuffer.rs"]
mod framebuffer;
#[path = "../../../nanami/servers/apps/honoka/src/app/input.rs"]
mod input_events;
#[path = "../../../nanami/servers/apps/honoka/src/app/motion_damage.rs"]
mod motion_damage;
mod input_api {
    pub mod constants {
        include!("../../../nanami/servers/sdk/rust/nanami-services/src/input/constants.rs");
    }
    pub mod service {
        include!("../../../nanami/servers/sdk/rust/nanami-services/src/input/service.rs");
    }
    pub use constants::*;
    pub use service::*;
}
#[path = "../../../nanami/servers/sdk/rust/nanami-services/src/gfx/honoka.rs"]
pub mod honoka_api;
pub mod input {
    pub use super::input_api::*;
    pub use super::input_events::InputEvent;
}
thread_local! { static PRESENTED: RefCell<Vec<framebuffer::Rect>> = const { RefCell::new(Vec::new()) }; }
mod api;
pub use api::*;
mod tests;
