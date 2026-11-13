use crate::Word;

pub const ACPI_SERVICE: &str = "acpi-service";

pub const POWER_REQUEST_SHUTDOWN: Word = 0xa001;
/// Firmware PCI INTx lookup: arg0=BDF, arg1=pin (1..4), reply0=GSI.
/// Read-only; failure means the caller must use a supported polling path.
pub const ACPI_REQUEST_PCI_INTX_ROUTE: Word = 0xa002;
