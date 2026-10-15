extern crate self as nanami_runtime;
pub use std::println;

#[allow(dead_code)]
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/registers.rs"]
pub mod registers;
mod xhci {
    pub use crate::registers;
}

#[path = "../../../nanami/servers/core-services/usb-server/src/init_trace.rs"]
mod init_trace;
