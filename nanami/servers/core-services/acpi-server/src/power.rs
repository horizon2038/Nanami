use acpi::{
    address::{GenericAddress, MappedGas},
    aml::{namespace::AmlName, object::Object, Interpreter},
    registers::FixedRegisters,
    sdt::fadt::Fadt,
    AcpiError, Handler,
};
use alloc::{sync::Arc, vec, vec::Vec};
use core::{cell::Cell, str::FromStr};

const SLP_TYP: u64 = 7 << 10;
const SLP_EN: u64 = 1 << 13;
const GBL_RLS: u64 = 1 << 2;

pub struct Power<H: Handler> {
    host: H,
    interpreter: Arc<Interpreter<H>>,
    registers: Arc<FixedRegisters<H>>,
    pm1a_status: MappedGas<H>,
    pm1b_status: Option<MappedGas<H>>,
    sleep_types: [u8; 2],
    smi_port: u16,
    acpi_enable: u8,
    acpi_disable: u8,
    enabled_mode: Cell<bool>,
}

pub fn sleep_types(value: &Object) -> Result<[u8; 2], AcpiError> {
    let Object::Package(values) = value else {
        return Err(AcpiError::HostUnimplemented);
    };
    if values.len() < 2 {
        return Err(AcpiError::HostUnimplemented);
    }
    let a = values[0].as_integer().map_err(AcpiError::Aml)?;
    let b = values[1].as_integer().map_err(AcpiError::Aml)?;
    if a > 7 || b > 7 {
        return Err(AcpiError::HostUnimplemented);
    }
    Ok([a as u8, b as u8])
}

pub fn sleep_control(old: u64, sleep_type: u8) -> u64 {
    // GBL_RLS is write-only and must not be replayed by this read-modify-write.
    (old & !(SLP_TYP | SLP_EN | GBL_RLS)) | (u64::from(sleep_type) << 10)
}

impl<H: Handler> Power<H> {
    pub fn new(
        host: H,
        interpreter: impl Into<Arc<Interpreter<H>>>,
        registers: Arc<FixedRegisters<H>>,
        fadt: &Fadt,
    ) -> Result<Self, AcpiError> {
        let interpreter = interpreter.into();
        let value = interpreter
            .evaluate(AmlName::from_str("\\_S5").unwrap(), Vec::new())
            .map_err(AcpiError::Aml)?;
        let sleep_types = sleep_types(&value)?;
        let smi_port = fadt
            .smi_cmd_port
            .try_into()
            .map_err(|_| AcpiError::InvalidGenericAddress)?;
        let status_register = |mut gas: GenericAddress| {
            // PM1_EVT contains two independent 16-bit registers: STS and EN.
            // WAK_STS must be acknowledged with a word access to STS only.
            gas.bit_width = 16;
            gas.access_size = 2;
            unsafe { MappedGas::map_gas(gas, &host) }
        };
        let pm1a_status = status_register(fadt.pm1a_event_block()?)?;
        let pm1b_status = fadt.pm1b_event_block()?.map(status_register).transpose()?;
        Ok(Self {
            host,
            interpreter,
            registers,
            pm1a_status,
            pm1b_status,
            sleep_types,
            smi_port,
            acpi_enable: fadt.acpi_enable,
            acpi_disable: fadt.acpi_disable,
            enabled_mode: Cell::new(false),
        })
    }

    pub fn sleep_types(&self) -> [u8; 2] {
        self.sleep_types
    }

    pub fn prepare(&self) -> Result<(), AcpiError> {
        // Do not enter ACPI mode at boot: this service does not yet own thermal
        // management or SCI events. Switch only during an explicit shutdown.
        if self.registers.pm1_control_registers.pm1a.read()? & 1 == 0 {
            if self.smi_port == 0 || self.acpi_enable == 0 || self.acpi_disable == 0 {
                return Err(AcpiError::HostUnimplemented);
            }
            self.enabled_mode.set(true);
            self.host.write_io_u8(self.smi_port, self.acpi_enable);
            let start = self.host.nanos_since_boot();
            while self.registers.pm1_control_registers.pm1a.read()? & 1 == 0 {
                if self.host.nanos_since_boot().saturating_sub(start) >= 3_000_000_000 {
                    return Err(AcpiError::Timeout);
                }
                self.host.sleep(1);
            }
        }
        self.method("\\_PTS")?;
        Ok(())
    }

    pub fn restore_mode(&self) -> Result<(), AcpiError> {
        if !self.enabled_mode.get() {
            return Ok(());
        }
        self.host.write_io_u8(self.smi_port, self.acpi_disable);
        let start = self.host.nanos_since_boot();
        while self.registers.pm1_control_registers.pm1a.read()? & 1 != 0 {
            if self.host.nanos_since_boot().saturating_sub(start) >= 3_000_000_000 {
                return Err(AcpiError::Timeout);
            }
            self.host.sleep(1);
        }
        self.enabled_mode.set(false);
        Ok(())
    }

    fn method(&self, name: &str) -> Result<(), AcpiError> {
        self.interpreter
            .evaluate_if_present(
                AmlName::from_str(name).unwrap(),
                vec![Object::Integer(5).wrap()],
            )
            .map(|_| ())
            .map_err(AcpiError::Aml)
    }

    pub fn enter_s5(&self) -> Result<(), AcpiError> {
        self.method("\\_GTS")?;
        // Clear only WAK_STS (write-one-to-clear), without touching PM1_EN.
        for register in core::iter::once(&self.pm1a_status).chain(self.pm1b_status.iter()) {
            register.write(1 << 15)?;
        }
        let control = &self.registers.pm1_control_registers;
        let a = sleep_control(control.pm1a.read()?, self.sleep_types[0]);
        let b = control
            .pm1b
            .as_ref()
            .map(|reg| reg.read())
            .transpose()?
            .map(|old| sleep_control(old, self.sleep_types[1]));
        // PM1a and PM1b may have different SLP_TYP values. Program both types
        // before setting either SLP_EN; do not use set_sleep_typ(a) for both.
        control.pm1a.write(a)?;
        if let (Some(reg), Some(value)) = (&control.pm1b, b) {
            reg.write(value)?;
        }
        control.pm1a.write(a | SLP_EN)?;
        if let (Some(reg), Some(value)) = (&control.pm1b, b) {
            reg.write(value | SLP_EN)?;
        }
        self.host.sleep(1000);
        Err(AcpiError::Timeout)
    }
}
