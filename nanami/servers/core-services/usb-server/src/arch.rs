//! Platform boundary: resource assignment belongs to Driver Manager. Mapping
//! and coherent DMA are currently supported only by the x86_64 adapter.

pub struct ControllerResource {
    pub mmio: usize,
    pub bytes: usize,
    pub irq: Option<usize>,
}

#[cfg(not(target_arch = "x86_64"))]
pub fn prepare(
    _: nanami_services::device::UsbControllerResource,
) -> Result<ControllerResource, libnanami::RequestError> {
    Err(libnanami::RequestError::Unsupported)
}

#[cfg(target_arch = "x86_64")]
pub fn prepare(
    resource: nanami_services::device::UsbControllerResource,
) -> Result<ControllerResource, libnanami::RequestError> {
    let (_, mmio) = libnanami::request_mmio(resource.physical, resource.bytes)?;
    Ok(ControllerResource {
        mmio,
        bytes: resource.bytes,
        irq: resource.irq,
    })
}
