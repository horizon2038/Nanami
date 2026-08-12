use super::*;

pub(crate) fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

pub(crate) fn fail_device(io_desc: Word, io_base: Word) {
    if let Ok(mut s) = read_device_status(io_desc, io_base) {
        s |= VIRTIO_STATUS_FAILED;
        let _ = write_device_status(io_desc, io_base, s);
    }
}
