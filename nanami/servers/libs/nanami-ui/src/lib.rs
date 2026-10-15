#![no_std]
//! Small native-app drawing/input helpers, using Honoka's existing Hack font.
extern crate alloc;

pub mod drawing;
#[cfg(target_os = "none")]
pub mod files;
pub mod input;
#[cfg(target_os = "none")]
pub mod window;

pub use drawing::{Canvas, Font, Rect};
pub use input::{Field, Key, Keyboard};

pub const PAPER: u32 = 0xfbfaf7;
pub const CHROME: u32 = 0xefede7;
pub const RULE: u32 = 0xd8d4ca;
pub const INK: u32 = 0x302e2b;
pub const MUTED: u32 = 0x77736c;
pub const ACCENT: u32 = 0x8b683a;
pub const SELECTED: u32 = 0xe6d6b9;
pub const ERROR: u32 = 0x9d342b;
pub const CELL: usize = 9;

pub trait Files {
    fn read(&mut self, path: &str) -> Result<alloc::vec::Vec<u8>, alloc::string::String>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), alloc::string::String>;
    fn exists(&mut self, path: &str) -> Result<bool, alloc::string::String>;
}

pub fn normalize_path(input: &str) -> Result<alloc::string::String, &'static str> {
    use alloc::{format, vec::Vec};
    if input.is_empty() || input.len() > 255 || input.contains('\0') || input.ends_with('/') {
        return Err("Enter a file path, including its name");
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => {
                parts.push(part);
            }
        }
    }
    let result = format!("/{}", parts.join("/"));
    if parts.is_empty() || result.len() > 255 {
        return Err("Invalid file path");
    }
    Ok(result)
}
