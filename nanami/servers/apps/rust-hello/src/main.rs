#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::debug::write_string("[user-app/rust] panic\n");
    loop {}
}

fn log_request_error(prefix: &str, err: nanami_runtime::RequestError) {
    nanami_runtime::debug::write_string(prefix);
    match err {
        nanami_runtime::RequestError::InvalidArgument => nanami_runtime::debug::write_string("invalid-arg\n"),
        nanami_runtime::RequestError::Unsupported => nanami_runtime::debug::write_string("unsupported\n"),
        nanami_runtime::RequestError::Transport => nanami_runtime::debug::write_string("transport\n"),
        nanami_runtime::RequestError::Protocol => nanami_runtime::debug::write_string("protocol\n"),
        nanami_runtime::RequestError::Status(code) => {
            nanami_runtime::debug::write_string("status=");
            nanami_runtime::print!("{:#x}", code);
            nanami_runtime::debug::write_string("\n");
        }
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::debug::write_string("[user-app/rust] hello from Rust user process\n");
    match nanami_runtime::ping(0xfeed_beef) {
        Ok(echo) => {
            if echo == 0xfeed_beef {
                nanami_runtime::debug::write_string("[user-app/rust] ping-pong ok\n");
            } else {
                nanami_runtime::debug::write_string("[user-app/rust] ping-pong mismatch\n");
            }
        }
        Err(e) => log_request_error("[user-app/rust] ping-pong failed: ", e),
    }
    Ok(())
}

nanami_runtime::nanami_entry!(nanami_main);
