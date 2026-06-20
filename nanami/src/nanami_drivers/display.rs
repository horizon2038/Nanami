use crate::nanami_core::memory::MemoryManager;
use crate::nanami_core::vm_space::VmSpace;
use crate::nanami_utils::descriptor::{make_child_slot_descriptor, make_root_slot_descriptor};
use nun::{CapabilityDescriptor, CapabilityError, FramebufferInfo, InitInfo};

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;
const FB_MAP_BASE_VA: usize = 0x6000_0000;
const GENERIC_NODE_RADIX: usize = 7;

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
        init_info: &InitInfo,
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

        let (generic_idx, generic_start, _) =
            find_framebuffer_generic_range(init_info, fb_addr, total_bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
        let generic_desc = make_generic_descriptor(memory.root_radix(), generic_idx);
        let generic_meta = init_info.generic_list[generic_idx];

        let skip_pages = (fb_base.saturating_sub(generic_start)) / PAGE_SIZE;
        let total_frames = skip_pages.saturating_add(page_count);
        match memory.allocate_physical_at(fb_base, page_count * PAGE_SIZE, true) {
            Ok(_) | Err(CapabilityError::InvalidArgument) => {}
            Err(e) => return Err(e),
        }
        let base_frame_index = memory
            .physical_page_index_from_address(generic_start)
            .ok_or(CapabilityError::InvalidArgument)?;

        crate::info!(
            "convert type={:>2} src_idx={:>3} src_desc={:#018x} src_is_device={} src_radix={:>2} count={:>6} dst_node={:#018x} dst_slot={:>7}",
            nun::CapabilityType::Frame as usize,
            generic_idx,
            generic_desc,
            generic_meta.is_device,
            generic_meta.size_radix,
            total_frames,
            memory.frame_node_descriptor(),
            base_frame_index
        );
        memory.ensure_alpha_frames_from_generic(generic_desc, base_frame_index, total_frames)?;

        crate::info!(
            "fb_generic={:>3} skip_pages={:>6} frame_count={:>6} base_slot={:>7}",
            generic_idx,
            skip_pages,
            total_frames,
            base_frame_index + skip_pages
        );

        let mut i = 0usize;
        while i < page_count {
            let frame_index = base_frame_index + skip_pages + i;
            let frame = memory
                .physical_frame_descriptor_from_index(frame_index)
                .ok_or(CapabilityError::InvalidArgument)?;
            if i == 0 {
                match nun::arch::frame::get_address(frame) {
                    Ok(pa) => {
                        crate::info!(
                            "get_address ok pa={:#018x} frame={:#018x}",
                            pa,
                            frame
                        );
                    }
                    Err(e) => {
                        crate::info!(
                            "get_address err={:?} frame={:#018x}",
                            e,
                            frame
                        );
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

fn find_framebuffer_generic_range(
    init_info: &InitInfo,
    fb_addr: usize,
    fb_size: usize,
) -> Option<(usize, usize, usize)> {
    let count = init_info.generic_list_count as usize;
    let mut best: Option<(usize, usize, usize, usize)> = None;

    for pass_device_only in [true, false] {
        let mut i = 0;
        while i < count {
            let g = init_info.generic_list[i];
            if pass_device_only && !g.is_device {
                i += 1;
                continue;
            }

            let start = g.address as usize;
            let size = 1usize << g.size_radix;
            let end = start.saturating_add(size);

            if fb_addr >= start && fb_addr.saturating_add(fb_size) <= end {
                match best {
                    None => best = Some((i, start, end, size)),
                    Some((_, _, _, best_size)) if size < best_size => {
                        best = Some((i, start, end, size))
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        if best.is_some() {
            break;
        }
    }

    best.map(|(idx, start, end, _)| (idx, start, end))
}

fn make_generic_descriptor(root_radix: usize, generic_index: usize) -> CapabilityDescriptor {
    let generic_node = make_root_slot_descriptor(root_radix, nun::InitSlotOffset::GenericNode as usize);
    make_child_slot_descriptor(generic_node, GENERIC_NODE_RADIX, generic_index)
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
