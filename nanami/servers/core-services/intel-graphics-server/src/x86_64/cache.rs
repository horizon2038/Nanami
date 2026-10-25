use core::arch::asm;
use nanami_runtime::Word;

pub fn flush(address: Word, bytes: Word) {
    // Gen9 scanout buffers are ordinary DMA RAM. Clean WB CPU lines before
    // exposing them to display hardware, without creating WB/WC aliases.
    let end = address + bytes;
    let mut line = address & !63;
    while line < end {
        unsafe {
            asm!("clflush [{line}]", line = in(reg) line, options(nostack, preserves_flags));
        }
        line += 64;
    }
    unsafe {
        asm!("mfence", options(nostack, preserves_flags));
    }
}
