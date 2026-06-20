use libnanami::{RequestError, Word};

pub fn ack_waited_irqs(
    waited: Word,
    irq1_desc: Word,
    irq12_desc: Word,
) -> Result<(), RequestError> {
    let (ack_irq1, ack_irq12) = classify_irq_notification(waited);

    if ack_irq1 {
        libnanami::ipc::interrupt_ack(irq1_desc)?;
    }
    if ack_irq12 {
        libnanami::ipc::interrupt_ack(irq12_desc)?;
    }
    Ok(())
}

pub fn update_irq_counters(waited: Word, irq1_count: &mut usize, irq12_count: &mut usize) {
    let (hit1, hit12) = classify_irq_notification(waited);

    if hit1 {
        *irq1_count = irq1_count.wrapping_add(1);
    }
    if hit12 {
        *irq12_count = irq12_count.wrapping_add(1);
    }
}

fn classify_irq_notification(waited: Word) -> (bool, bool) {
    let mut irq1 = waited == 1;
    let mut irq12 = waited == 12;

    if 1 < (core::mem::size_of::<Word>() * 8) && (waited & (1usize << 1)) != 0 {
        irq1 = true;
    }
    if 12 < (core::mem::size_of::<Word>() * 8) && (waited & (1usize << 12)) != 0 {
        irq12 = true;
    }

    if !irq1 && !irq12 {
        return (true, true);
    }

    (irq1, irq12)
}
