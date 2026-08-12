use crate::nanami_core::memory::MemoryManager;
use crate::nanami_core::vm_space::VmSpace;
use nun::{CapabilityDescriptor, CapabilityError, FramebufferInfo, InitInfo};

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;
const FB_MAP_BASE_VA: usize = 0x6000_0000;

pub struct DisplayDriver {
    info: FramebufferInfo,
}

impl DisplayDriver {
    pub fn from_init_info(init_info: &InitInfo) -> Option<Self> {
        let mut data = [0usize; 13];
        data.copy_from_slice(&init_info.arch_info[1..14]);
        Some(Self {
            info: FramebufferInfo::deserialize(&data),
        })
    }

    pub fn map(
        &mut self,
        _init_info: &InitInfo,
        memory: &mut MemoryManager,
        address_space: CapabilityDescriptor,
        vm_space: &mut VmSpace,
    ) -> Result<(), CapabilityError> {
        let fb_addr = self.info.address;
        if fb_addr == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let bytes_per_pixel = (self.info.bits_per_pixel as usize).saturating_div(8);
        if bytes_per_pixel == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let stride_raw = self.info.stride as usize;
        let stride_bytes = if stride_raw >= self.info.width as usize * bytes_per_pixel {
            stride_raw
        } else {
            stride_raw.saturating_mul(bytes_per_pixel)
        };

        let total_bytes = stride_bytes.saturating_mul(self.info.height as usize);
        if total_bytes == 0 {
            return Err(CapabilityError::InvalidArgument);
        }

        let fb_base = fb_addr & !(PAGE_SIZE - 1);
        let offset = fb_addr - fb_base;
        let total_span = offset + total_bytes;
        let page_count = (total_span + PAGE_SIZE - 1) / PAGE_SIZE;

        match memory.allocate_physical_at(fb_base, page_count * PAGE_SIZE, true) {
            Ok(_) | Err(CapabilityError::InvalidArgument) => {}
            Err(e) => return Err(e),
        }
        let (base_frame_index, skip_pages, converted_pages) = memory
            .ensure_alpha_frames_for_range_from_initial_generic(
                fb_base,
                page_count * PAGE_SIZE,
                true,
            )?;
        if converted_pages != page_count {
            return Err(CapabilityError::InvalidArgument);
        }

        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_frame_index + skip_pages + i;
            let frame = memory
                .physical_frame_descriptor_from_index(frame_index)
                .ok_or(CapabilityError::InvalidArgument)?;
            if i == 0 {
                match nun::arch::frame::get_address(frame) {
                    Ok(pa) => {
                        crate::info!("get_address ok pa={:#018x} frame={:#018x}", pa, frame);
                    }
                    Err(e) => {
                        crate::info!("get_address err={:?} frame={:#018x}", e, frame);
                    }
                }
            }
            let va = FB_MAP_BASE_VA + i * PAGE_SIZE;
            if let Err(e) = memory.map_frame(address_space, frame, va, vm_space) {
                crate::info!(
                    "page={:>6} frame={:#018x} va={:#018x} err={:?}",
                    i,
                    frame,
                    va,
                    e
                );
                return Err(e);
            }
            i += 1;
        }

        self.info.address = FB_MAP_BASE_VA + offset;
        crate::info!(
            "framebuffer mapped addr={:#018x} pages={:>6}",
            self.info.address,
            page_count
        );

        Ok(())
    }

    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        let mut y = 0;
        while y < self.info.height {
            let mut x = 0;
            while x < self.info.width {
                self.put_pixel(x, y, r, g, b);
                x += 1;
            }
            y += 1;
        }
    }

    pub fn draw_test_pattern(&mut self) {
        let mut y = 0;
        while y < self.info.height {
            let mut x = 0;
            while x < self.info.width {
                let r = ((x * 255) / self.info.width.max(1)) as u8;
                let g = ((y * 255) / self.info.height.max(1)) as u8;
                let b = 0x80;
                self.put_pixel(x, y, r, g, b);
                x += 1;
            }
            y += 1;
        }
    }

    fn put_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) {
        if x >= self.info.width || y >= self.info.height {
            return;
        }
        let bpp = self.info.bits_per_pixel as usize;
        if bpp != 32 {
            return;
        }

        let pixel = pack_color(&self.info, r, g, b);
        let bytes_per_pixel = bpp / 8;
        let stride_raw = self.info.stride as usize;
        let stride_bytes = if stride_raw >= self.info.width as usize * bytes_per_pixel {
            stride_raw
        } else {
            stride_raw.saturating_mul(bytes_per_pixel)
        };
        let offset = y as usize * stride_bytes + x as usize * bytes_per_pixel;

        unsafe {
            let p = (self.info.address as *mut u8).add(offset) as *mut u32;
            p.write_volatile(pixel);
        }
    }
}

fn pack_color(info: &FramebufferInfo, r: u8, g: u8, b: u8) -> u32 {
    let mut value = 0u32;
    value |= (scale_channel(r, info.red.size) as u32) << info.red.position;
    value |= (scale_channel(g, info.green.size) as u32) << info.green.position;
    value |= (scale_channel(b, info.blue.size) as u32) << info.blue.position;
    value
}

fn scale_channel(value: u8, size: u8) -> u8 {
    if size == 0 {
        return 0;
    }
    let max_dst = (1u32 << size) - 1;
    ((value as u32 * max_dst) / 255u32) as u8
}
