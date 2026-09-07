#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64.rs"]
mod imp;
#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64.rs"]
mod imp;

pub use imp::*;

pub fn validate_platform(architecture: &str, platform: &str) -> Result<(), libnanami::RequestError> {
    match (architecture, platform) {
        #[cfg(target_arch = "x86_64")]
        ("x86_64", "pc99") => Ok(()),
        #[cfg(target_arch = "aarch64")]
        ("aarch64", "qemu") => Ok(()),
        _ => Err(libnanami::RequestError::Unsupported),
    }
}
