use nanami_runtime::{self, RequestError};

pub fn log_request_error(prefix: &str, err: RequestError) {
    nanami_runtime::println!("{}{}", prefix, err);
}

pub fn log_error(prefix: &str, err: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, err);
    err.into()
}
