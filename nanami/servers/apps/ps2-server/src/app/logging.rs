use libnanami::{self, RequestError};

pub fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

pub fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}
