#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64.rs"]
mod imp;
#[cfg(all(target_arch = "x86_64", not(feature = "hpet")))]
#[path = "arch/x86_64.rs"]
mod imp;
#[cfg(all(target_arch = "x86_64", feature = "hpet"))]
#[path = "arch/x86_64_hpet.rs"]
mod imp;

pub use imp::*;

pub fn counter_source(
    timer: &PreparedTimer,
    kind: nanami_runtime::Word,
) -> Option<(nanami_runtime::Word, nanami_runtime::Word)> {
    #[cfg(all(target_arch = "x86_64", feature = "hpet"))]
    if kind == nanami_client::timer::TIMER_COUNTER_SOURCE_HPET64 {
        return timer.counter_source();
    }
    let _ = (timer, kind);
    None
}
