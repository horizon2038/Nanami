use nanami_runtime::RequestError;

pub fn log_request_error(prefix: &str, err: RequestError) {
    nanami_runtime::println!("{}{}", prefix, err);
}

pub fn log_error(prefix: &str, err: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

pub fn busy_delay() {
    let mut i = 0usize;
    while i < 500_000 {
        core::hint::spin_loop();
        i += 1;
    }
}
