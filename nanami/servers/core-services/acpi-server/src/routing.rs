//! Firmware INTx routing. No guessed chipset routes or writes to PCI link _SRS.
use acpi::{
    aml::{
        namespace::{AmlName, NamespaceLevelKind},
        object::{Object, WrappedObject},
        AmlError, Interpreter,
    },
    Handler,
};
use alloc::{sync::Arc, vec, vec::Vec};
use core::str::FromStr;

#[path = "routing/resources.rs"]
mod resources;

#[derive(Debug)]
pub enum Error {
    Aml(AmlError),
    Malformed,
    NoRoute,
    Unsupported,
}
impl From<AmlError> for Error {
    fn from(error: AmlError) -> Self {
        Self::Aml(error)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Aml(error) => write!(formatter, "AML: {:?}", error),
            error => write!(formatter, "{:?}", error),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub gsi: u32,
    pub level: bool,
    pub low: bool,
}
impl Route {
    pub fn supported_irq(self) -> Result<usize, Error> {
        // A9N's current IO-APIC API provides these attributes for GSI16..23.
        // Other routes require an explicit kernel routing API, not MMIO hacks.
        if self.level && self.low && (16..=23).contains(&self.gsi) {
            Ok(self.gsi as usize)
        } else {
            Err(Error::Unsupported)
        }
    }
}

pub struct Routing<H: Handler> {
    interpreter: Arc<Interpreter<H>>,
    devices: Vec<AmlName>,
}

fn child(scope: &AmlName, name: &str) -> Result<AmlName, AmlError> {
    AmlName::from_str(name)?.resolve(scope)
}

impl<H: Handler> Routing<H> {
    pub fn new(interpreter: Arc<Interpreter<H>>) -> Result<Self, Error> {
        // Inform AML that the already-initialized interrupt controller is APIC.
        // This does not enable ACPI mode, run device _INI methods, or enter S5.
        interpreter.evaluate_if_present(
            AmlName::from_str("\\_PIC")?,
            vec![Object::Integer(1).wrap()],
        )?;
        let mut devices = Vec::new();
        interpreter.namespace.lock().traverse(|name, level| {
            if level.kind == NamespaceLevelKind::Device {
                devices.push(name.clone());
            }
            Ok(true)
        })?;
        Ok(Self {
            interpreter,
            devices,
        })
    }

    fn value(&self, scope: &AmlName, name: &str) -> Result<Option<WrappedObject>, Error> {
        Ok(self
            .interpreter
            .evaluate_if_present(child(scope, name)?, vec![])?)
    }
    fn number(&self, scope: &AmlName, name: &str, default: u64) -> Result<u64, Error> {
        self.value(scope, name)?
            .map_or(Ok(default), |value| Ok(value.as_integer()?))
    }

    /// Resolve a function on its ACPI root bus. Non-root buses are rejected:
    /// they need PCI bridge topology/swizzling, never a guessed root _PRT entry.
    pub fn route(&self, bdf: usize, pin: u8) -> Result<Route, Error> {
        if bdf & !0x00ff_1f07 != 0 || !(1..=4).contains(&pin) {
            return Err(Error::Malformed);
        }
        let bus = bdf >> 16;
        let mut root = None;
        for name in &self.devices {
            let is_root = self.value(name, "_HID")?.is_some_and(|id| pci_root(&id))
                || self.value(name, "_CID")?.is_some_and(|id| pci_root(&id));
            if !is_root
                || self.number(name, "_SEG", 0)? != 0
                || self.number(name, "_BBN", 0)? != bus as u64
            {
                continue;
            }
            if root.replace(name).is_some() {
                return Err(Error::Malformed);
            }
        }
        let root = root.ok_or(Error::NoRoute)?;
        let table = self.value(root, "_PRT")?.ok_or(Error::NoRoute)?;
        self.from_prt(
            root,
            &table,
            ((bdf >> 8) & 31) as u16,
            (bdf & 7) as u16,
            pin - 1,
        )
    }

    fn from_prt(
        &self,
        scope: &AmlName,
        table: &Object,
        device: u16,
        function: u16,
        pin: u8,
    ) -> Result<Route, Error> {
        let Object::Package(entries) = table else {
            return Err(Error::Malformed);
        };
        let mut selected = None;
        for entry in entries {
            let Object::Package(fields) = &**entry else {
                return Err(Error::Malformed);
            };
            if fields.len() != 4 {
                return Err(Error::Malformed);
            }
            let address = fields[0].as_integer()?;
            let entry_pin = fields[1].as_integer()?;
            if address >> 32 != 0
                || address >> 16 > 31
                || entry_pin > 3
                || (address as u16 != 0xffff && address as u16 > 7)
            {
                return Err(Error::Malformed);
            }
            if address >> 16 != u64::from(device)
                || entry_pin != u64::from(pin)
                || (address as u16 != 0xffff && address as u16 != function)
            {
                continue;
            }
            if selected.replace(fields).is_some() {
                return Err(Error::Malformed);
            }
        }
        let fields = selected.ok_or(Error::NoRoute)?;
        let index = u32::try_from(fields[3].as_integer()?).map_err(|_| Error::Malformed)?;
        if matches!(*fields[2], Object::Integer(0)) {
            return Ok(Route {
                gsi: index,
                level: true,
                low: true,
            });
        }
        let source = match &*fields[2] {
            Object::String(name) => self
                .interpreter
                .namespace
                .lock()
                .search_for_level(&AmlName::from_str(name)?, scope)?,
            Object::Reference { inner, .. } => {
                // AML may return a reference to the Link Device rather than a string.
                let object = inner.clone().unwrap_reference();
                let mut namespace = self.interpreter.namespace.lock();
                let mut found = None;
                for name in &self.devices {
                    if namespace
                        .get(name.clone())
                        .is_ok_and(|value| core::ptr::eq(&*value, &*object))
                    {
                        if found.replace(name.clone()).is_some() {
                            return Err(Error::Malformed);
                        }
                    }
                }
                found.ok_or(Error::NoRoute)?
            }
            _ => return Err(Error::Malformed),
        };
        let current = self.value(&source, "_CRS")?.ok_or(Error::NoRoute)?;
        let Object::Buffer(bytes) = &*current else {
            return Err(Error::Malformed);
        };
        // SourceIndex selects a resource descriptor, not an IRQ-array element.
        resources::interrupt(bytes, index)
    }
}

fn pci_root(value: &Object) -> bool {
    match value {
        Object::String(id) => matches!(id.as_str(), "PNP0A03" | "PNP0A08"),
        Object::Integer(id) => matches!(*id, 0x030a_d041 | 0x080a_d041),
        Object::Package(ids) => ids.iter().any(|id| pci_root(id)),
        _ => false,
    }
}
