use a9n_abi::CapabilityDescriptor;

use crate::{map_capability_error, RequestError, Word};

pub fn io_read(
    port_descriptor: CapabilityDescriptor,
    address: Word,
    byte_width: Word,
) -> Result<Word, RequestError> {
    let mut data = 0usize;
    a9n_abi::arch::io_port::read(port_descriptor, address, byte_width, &mut data)
        .map_err(map_capability_error)?;
    Ok(data)
}

pub fn io_write(
    port_descriptor: CapabilityDescriptor,
    address: Word,
    byte_width: Word,
    data: Word,
) -> Result<(), RequestError> {
    a9n_abi::arch::io_port::write(port_descriptor, address, byte_width, data)
        .map_err(map_capability_error)
}
