#![no_std]
//! The user-space ABI shared by Alpha, servers and clients. This crate only
//! describes protocols: it does not invoke capabilities, allocate or start a
//! process. Runtime mechanisms and client policy live in separate crates.

pub type Word = usize;
mod error;
pub use error::{NanamiError, NanamiResult, RequestError};
pub mod alpha;
pub mod alter;
pub mod block;
pub mod device;
pub mod exec;
pub mod gfx;
pub mod gpu;
pub mod input;
pub mod intel_graphics;
pub mod ipc;
pub mod negoto;
pub mod net;
pub mod os;
pub mod pci;
pub mod posix;
pub mod power;
pub mod registry;
pub mod rtc;
pub mod service;
pub mod terminal;
pub mod timer;
pub mod usb;
pub mod vfs;
