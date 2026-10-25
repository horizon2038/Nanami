use nanami_client::pci::*;
use nanami_runtime::{RequestError, Word};
use pci_ids_rs::{Class, FromId, Subclass, Vendor};

/// Resolve the database once per enumerated device, not on config/data requests
/// or repeatedly for each text chunk. Strings refer to the server's static DB.
pub struct Names {
    fields: [&'static str; 5],
}

impl Names {
    pub fn identify(device: Device) -> Self {
        let vendor = Vendor::from_id(device.vendor_id);
        let product = pci_ids_rs::Device::from_vid_pid(device.vendor_id, device.device_id);
        let class = Class::from_id((device.class >> 16) as u8);
        let subclass =
            Subclass::from_cid_sid((device.class >> 16) as u8, (device.class >> 8) as u8);
        let interface = subclass.and_then(|subclass| {
            subclass
                .prog_ifs()
                .find(|interface| interface.id() == device.class as u8)
        });
        Self {
            fields: [
                vendor.map_or("", Vendor::name),
                product.map_or("", pci_ids_rs::Device::name),
                class.map_or("", Class::name),
                subclass.map_or("", Subclass::name),
                interface.map_or("", pci_ids_rs::ProgIf::name),
            ],
        }
    }

    pub fn get(&self, field: Word) -> Result<&'static str, RequestError> {
        self.fields
            .get(field)
            .copied()
            .ok_or(RequestError::InvalidArgument)
    }

    pub fn chunk(&self, field: Word, offset: Word) -> Result<(Word, Word), RequestError> {
        let name = self.get(field)?.as_bytes();
        let rest = name.get(offset..).ok_or(RequestError::InvalidArgument)?;
        let mut chunk = [0u8; PCI_NAME_CHUNK_BYTES];
        let count = rest.len().min(chunk.len());
        chunk[..count].copy_from_slice(&rest[..count]);
        Ok((name.len(), u64::from_le_bytes(chunk) as Word))
    }
}
