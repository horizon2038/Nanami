use crate::{host::Host, power::Power};
use acpi::{
    aml::Interpreter,
    platform::PciConfigRegions,
    registers::FixedRegisters,
    rsdp::Rsdp,
    sdt::{facs::Facs, fadt::Fadt, SdtHeader, Signature},
    AcpiError, AcpiTable, AcpiTables, AmlTable, Handler, PhysicalMapping,
};
use alloc::sync::Arc;
use core::{mem::size_of, slice};

/// Unlike AcpiTables' lenient discovery, power management requires valid
/// checksums and bounded lengths before it can write firmware-selected ports.
fn table(
    host: &Host,
    address: usize,
    signature: Signature,
) -> Result<PhysicalMapping<Host, SdtHeader>, AcpiError> {
    if address == 0 {
        return Err(AcpiError::SdtInvalidSignature(signature));
    }
    let header = unsafe { host.map_physical_region::<SdtHeader>(address, size_of::<SdtHeader>()) };
    let length = header.length as usize;
    if !(size_of::<SdtHeader>()..=1024 * 1024).contains(&length) {
        return Err(AcpiError::SdtInvalidSignature(signature));
    }
    let mapping = unsafe { host.map_physical_region::<SdtHeader>(address, length) };
    unsafe {
        mapping.validate(signature)?;
    }
    Ok(mapping)
}

pub struct Firmware {
    pub interpreter: Arc<Interpreter<Host>>,
    host: Host,
    registers: Arc<FixedRegisters<Host>>,
    fadt: PhysicalMapping<Host, Fadt>,
}

impl Firmware {
    pub fn power(&self) -> Result<Power<Host>, AcpiError> {
        Power::new(
            self.host.clone(),
            self.interpreter.clone(),
            self.registers.clone(),
            &self.fadt,
        )
    }
}

pub fn load(host: Host, rsdp_address: usize) -> Result<Firmware, AcpiError> {
    if rsdp_address == 0 {
        return Err(AcpiError::NoValidRsdp);
    }
    let rsdp = unsafe { host.map_physical_region::<Rsdp>(rsdp_address, size_of::<Rsdp>()) };
    if rsdp.revision > 0 && !(36..=4096).contains(&{ rsdp.length }) {
        return Err(AcpiError::RsdpInvalidChecksum);
    }
    let rsdp = unsafe {
        host.map_physical_region::<Rsdp>(
            rsdp_address,
            if rsdp.revision > 0 {
                rsdp.length as usize
            } else {
                size_of::<Rsdp>()
            },
        )
    };
    rsdp.validate()?;
    // Validate the original 20-byte checksum independently of the extended one.
    let bytes = unsafe { slice::from_raw_parts(rsdp.virtual_start.as_ptr().cast::<u8>(), 20) };
    if bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
        return Err(AcpiError::RsdpInvalidChecksum);
    }
    let (revision, root, signature, entry_size) = if rsdp.revision > 0 && rsdp.xsdt_address != 0 {
        (
            rsdp.revision,
            rsdp.xsdt_address as usize,
            Signature::XSDT,
            8,
        )
    } else {
        (0, rsdp.rsdt_address as usize, Signature::RSDT, 4)
    };
    let root_table = table(&host, root, signature)?;
    if (root_table.length as usize - size_of::<SdtHeader>()) % entry_size != 0 {
        return Err(AcpiError::SdtInvalidSignature(signature));
    }
    let tables = unsafe { AcpiTables::from_rsdt(host.clone(), revision, root)? };
    let fadt = tables
        .find_table::<Fadt>()
        .ok_or(AcpiError::TableNotFound(Signature::FADT))?;
    fadt.validate()?;
    if fadt.header.length < 116
        || { fadt.flags }.system_is_hw_reduced_acpi()
        || fadt.pm1_control_length != 2
        || fadt.pm1_event_length != 4
    {
        return Err(AcpiError::HostUnimplemented);
    }
    validate_register(fadt.pm1a_control_block()?, 16)?;
    validate_register(fadt.pm1a_event_block()?, 32)?;
    if let Some(reg) = fadt.pm1b_control_block()? {
        validate_register(reg, 16)?;
    }
    if let Some(reg) = fadt.pm1b_event_block()? {
        validate_register(reg, 32)?;
    }
    if let Some(mcfg) = tables.find_table::<acpi::sdt::mcfg::Mcfg>() {
        mcfg.validate()?;
        host.set_pci(PciConfigRegions::new(&tables)?);
    }
    let registers = Arc::new(FixedRegisters::new(&fadt, host.clone())?);
    let facs = match fadt.facs_address() {
        Ok(address) => {
            let mapping = unsafe { host.map_physical_region::<Facs>(address, size_of::<Facs>()) };
            if mapping.signature != Signature::FACS || mapping.length < 64 {
                return Err(AcpiError::InvalidFacsAddress);
            }
            Some(mapping)
        }
        Err(_) => None,
    };
    let dsdt = tables.dsdt()?;
    let interpreter = Interpreter::new(host.clone(), dsdt.revision, registers.clone(), facs);
    load_aml(&host, &interpreter, dsdt, Signature::DSDT)?;
    for ssdt in tables.ssdts() {
        load_aml(&host, &interpreter, ssdt, Signature::SSDT)?;
    }
    Ok(Firmware {
        host,
        interpreter: Arc::new(interpreter),
        registers,
        fadt,
    })
}

fn validate_register(gas: acpi::address::GenericAddress, bits: u8) -> Result<(), AcpiError> {
    use acpi::address::AddressSpace;
    let bytes = u64::from(bits / 8);
    if gas.address == 0 || gas.bit_offset != 0 || gas.bit_width != bits {
        return Err(AcpiError::InvalidGenericAddress);
    }
    match gas.address_space {
        AddressSpace::SystemIo if gas.address <= 0x10000 - bytes => Ok(()),
        AddressSpace::SystemMemory if gas.address % bytes == 0 => Ok(()),
        _ => Err(AcpiError::InvalidGenericAddress),
    }
}

fn load_aml(
    host: &Host,
    interpreter: &Interpreter<Host>,
    aml: AmlTable,
    signature: Signature,
) -> Result<(), AcpiError> {
    let mapping = table(host, aml.phys_address, signature)?;
    let bytes = unsafe {
        slice::from_raw_parts(
            mapping
                .virtual_start
                .as_ptr()
                .cast::<u8>()
                .add(size_of::<SdtHeader>()),
            mapping.region_length - size_of::<SdtHeader>(),
        )
    };
    interpreter.load_table(bytes).map_err(AcpiError::Aml)
}
