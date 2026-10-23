use nanami_runtime::{RequestError, Word};

/// Deassert the device's level source before unmasking its kernel IRQ port.
pub fn acknowledge(io: Word, base: Word, interrupt: Word) -> Result<(), RequestError> {
    complete(io, base, Some(interrupt))
}

/// Polling still retires device ISR state, but must not ACK an unowned IRQ.
pub fn complete(io: Word, base: Word, interrupt: Option<Word>) -> Result<(), RequestError> {
    super::arch::acknowledge_interrupt(io, base)?;
    if let Some(interrupt) = interrupt {
        nanami_runtime::ipc::interrupt_ack(interrupt)?;
    }
    Ok(())
}
