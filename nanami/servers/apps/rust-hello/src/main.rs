#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::debug::write_string("[user-app/rust] panic\n");
    loop {}
}

fn log_request_error(prefix: &str, err: libnanami::RequestError) {
    libnanami::debug::write_string(prefix);
    match err {
        libnanami::RequestError::InvalidArgument => libnanami::debug::write_string("invalid-arg\n"),
        libnanami::RequestError::Unsupported => libnanami::debug::write_string("unsupported\n"),
        libnanami::RequestError::Transport => libnanami::debug::write_string("transport\n"),
        libnanami::RequestError::Protocol => libnanami::debug::write_string("protocol\n"),
        libnanami::RequestError::Status(code) => {
            libnanami::debug::write_string("status=");
            libnanami::print!("{:#x}", code);
            libnanami::debug::write_string("\n");
        }
    }
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::debug::write_string("[user-app/rust] hello from Rust user process\n");
    match libnanami::ping(0xfeed_beef) {
        Ok(echo) => {
            if echo == 0xfeed_beef {
                libnanami::debug::write_string("[user-app/rust] ping-pong ok\n");
            } else {
                libnanami::debug::write_string("[user-app/rust] ping-pong mismatch\n");
            }
        }
        Err(e) => log_request_error("[user-app/rust] ping-pong failed: ", e),
    }
    Ok(())
}

libnanami::nanami_entry!(nanami_main);
