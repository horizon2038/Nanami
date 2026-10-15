use alloc::{vec, vec::Vec};
use core::convert::Infallible;
use embedded_graphics::{
    pixelcolor::{Rgb888, RgbColor},
    prelude::*,
};

pub const MAX_EDGE: usize = 1024;
pub const MAX_PIXELS: usize = MAX_EDGE * MAX_EDGE;
pub const MAX_FILE: usize = MAX_PIXELS * 4 + 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}
impl Image {
    pub fn new(width: usize, height: usize, color: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return Err("Canvas size must be 1..1024 pixels on each side");
        }
        Ok(Self {
            width,
            height,
            pixels: vec![color & 0xffffff; width * height],
        })
    }
    pub fn get(&self, x: usize, y: usize) -> u32 {
        self.pixels[y * self.width + x]
    }
    pub fn set(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.pixels[y as usize * self.width + x as usize] = color & 0xffffff;
        }
    }
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 54 || &data[..2] != b"BM" {
            return Err("Not a BMP file");
        }
        let u16_at = |at| u16::from_le_bytes([data[at], data[at + 1]]);
        let u32_at = |at| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let offset = u32_at(10) as usize;
        let dib = u32_at(14) as usize;
        let width = u32_at(18) as i32;
        let height = u32_at(22) as i32;
        let depth = u16_at(28);
        if dib < 40
            || dib > data.len() - 14
            || offset < 14 + dib
            || u16_at(26) != 1
            || !matches!(depth, 24 | 32)
            || u32_at(30) != 0
        {
            return Err("Use an uncompressed 24-bit or 32-bit BMP");
        }
        if width <= 0 || height == 0 || height == i32::MIN {
            return Err("Invalid BMP dimensions");
        }
        let mut image = Self::new(width as usize, height.unsigned_abs() as usize, 0xffffff)?;
        let stride = (image.width * (depth as usize / 8)).div_ceil(4) * 4;
        let end = offset
            .checked_add(stride * image.height)
            .ok_or("Invalid BMP size")?;
        if end > data.len() || (u32_at(2) as usize) < end || (u32_at(2) as usize) > data.len() {
            return Err("Truncated BMP data");
        }
        for y in 0..image.height {
            let source_y = if height > 0 { image.height - 1 - y } else { y };
            for x in 0..image.width {
                let at = offset + source_y * stride + x * (depth as usize / 8);
                image.pixels[y * image.width + x] =
                    data[at] as u32 | (data[at + 1] as u32) << 8 | (data[at + 2] as u32) << 16;
            }
        }
        Ok(image)
    }
    pub fn encode(&self) -> Vec<u8> {
        let stride = (self.width * 3).div_ceil(4) * 4;
        let mut bytes = vec![0; 54 + stride * self.height];
        let length = bytes.len() as u32;
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&length.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&(self.width as i32).to_le_bytes());
        bytes[22..26].copy_from_slice(&(self.height as i32).to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        bytes[34..38].copy_from_slice(&((stride * self.height) as u32).to_le_bytes());
        for y in 0..self.height {
            for x in 0..self.width {
                let at = 54 + (self.height - 1 - y) * stride + x * 3;
                let color = self.get(x, y);
                bytes[at] = color as u8;
                bytes[at + 1] = (color >> 8) as u8;
                bytes[at + 2] = (color >> 16) as u8;
            }
        }
        bytes
    }
}
impl OriginDimensions for Image {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}
impl DrawTarget for Image {
    type Color = Rgb888;
    type Error = Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb888>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Infallible> {
        for Pixel(point, c) in pixels {
            self.set(
                point.x,
                point.y,
                (c.r() as u32) << 16 | (c.g() as u32) << 8 | c.b() as u32,
            );
        }
        Ok(())
    }
}
