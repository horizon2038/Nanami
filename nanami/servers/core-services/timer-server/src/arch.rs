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
