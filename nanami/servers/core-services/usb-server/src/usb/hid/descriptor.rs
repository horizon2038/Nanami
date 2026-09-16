//! Bounded HID short-item parser (HID 1.11 section 6.2.2).
//! Input offsets are independent of Output/Feature reports and of Report ID.
//! Unknown usages consume bits but never become mouse events.
use super::report::{Field, MouseReport, Report};
use alloc::vec::Vec;

#[derive(Clone, Copy, Default)]
struct Global {
    page: u32,
    minimum: i64,
    maximum: u32,
    maximum_width: usize,
    size: u32,
    count: u32,
    id: u8,
}

#[derive(Default)]
struct Local {
    usages: Vec<(u32, u32)>,
    minimum: Option<u32>,
}

impl Local {
    fn at(&self, mut index: u32) -> Option<u32> {
        for &(first, last) in &self.usages {
            let count = last - first + 1;
            if index < count {
                return Some(first + index);
            }
            index -= count;
        }
        // Variable items repeat the last usage when count exceeds the usage list.
        self.usages.last().map(|&(_, last)| last)
    }
    fn clear(&mut self) {
        self.usages.clear();
        self.minimum = None;
    }
}

fn signed(value: u32, bytes: usize) -> i64 {
    if bytes == 0 {
        return 0;
    }
    ((value << (32 - bytes * 8)) as i32 >> (32 - bytes * 8)) as i64
}

pub(super) fn parse(data: &[u8], packet_size: usize) -> Result<MouseReport, ()> {
    if data.is_empty() || data.len() > 4096 || !(3..=1024).contains(&packet_size) {
        return Err(());
    }
    let mut global = Global::default();
    let mut local = Local::default();
    let mut globals = Vec::new();
    let mut collections = Vec::new();
    let mut mouse = false;
    let mut layout = MouseReport {
        numbered: false,
        reports: Vec::new(),
    };
    let mut offset = 0;
    while offset < data.len() {
        let prefix = data[offset];
        offset += 1;
        // Long/reserved items and delimiters are not silently interpreted.
        if prefix == 0xfe {
            return Err(());
        }
        let size = [0, 1, 2, 4][(prefix & 3) as usize];
        let bytes = data.get(offset..offset + size).ok_or(())?;
        offset += size;
        let value = bytes
            .iter()
            .enumerate()
            .fold(0u32, |v, (i, b)| v | (*b as u32) << (i * 8));
        let tag = prefix >> 4;
        match (prefix >> 2) & 3 {
            0 => {
                if local.minimum.is_some() {
                    return Err(());
                }
                match tag {
                    8 => input(&mut layout, global, &local, mouse, value, packet_size)?,
                    9 | 11 => {} // Output and Feature do not advance Input offsets.
                    10 => {
                        collections.push(mouse);
                        if value == 1 {
                            mouse = local.at(0) == Some(0x0001_0002);
                        }
                    }
                    12 => mouse = collections.pop().ok_or(())?,
                    _ => return Err(()),
                }
                local.clear();
            }
            1 => match tag {
                0 if value <= 0xffff => global.page = value,
                1 => global.minimum = signed(value, size),
                2 => {
                    global.maximum = value;
                    global.maximum_width = size;
                }
                3..=6 => {} // Physical ranges and units do not change raw relative counts.
                7 => global.size = value,
                8 if (1..=255).contains(&value) => {
                    global.id = value as u8;
                    layout.numbered = true;
                }
                9 => global.count = value,
                10 if size == 0 => globals.push(global),
                11 if size == 0 => global = globals.pop().ok_or(())?,
                _ => return Err(()),
            },
            2 => {
                let usage = if size == 4 {
                    value
                } else {
                    (global.page << 16) | value
                };
                match tag {
                    0 => local.usages.push((usage, usage)),
                    1 if local.minimum.is_none() => local.minimum = Some(usage),
                    2 => {
                        let first = local.minimum.take().ok_or(())?;
                        if first > usage || first >> 16 != usage >> 16 {
                            return Err(());
                        }
                        local.usages.push((first, usage));
                    }
                    3..=5 | 7..=9 => {} // Designator/string indices.
                    _ => return Err(()),
                }
            }
            _ => return Err(()),
        }
    }
    if !collections.is_empty() || !globals.is_empty() || local.minimum.is_some() {
        return Err(());
    }
    let mut present = [false; 6];
    for report in &layout.reports {
        if layout.numbered && report.id == 0 {
            return Err(());
        }
        if report.bits.div_ceil(8) + usize::from(layout.numbered) > packet_size {
            return Err(());
        }
        for (index, field) in report.fields.iter().enumerate() {
            if field.is_some() {
                // Shared button usages across report IDs need separate state/aggregation.
                if index < 3 && present[index] {
                    return Err(());
                }
                present[index] = true;
            }
        }
    }
    // Never lose basic pointer operation just to enable a wheel-only layout.
    if !present[0] || !present[3] || !present[4] {
        return Err(());
    }
    layout
        .reports
        .retain(|report| report.fields.iter().any(Option::is_some));
    Ok(layout)
}

fn input(
    layout: &mut MouseReport,
    global: Global,
    local: &Local,
    mouse: bool,
    flags: u32,
    packet_size: usize,
) -> Result<(), ()> {
    let bits = global.size.checked_mul(global.count).ok_or(())? as usize;
    if bits == 0 || bits > packet_size * 8 {
        return Err(());
    }
    let index = match layout
        .reports
        .iter()
        .position(|report| report.id == global.id)
    {
        Some(index) => index,
        None => {
            layout.reports.push(Report {
                id: global.id,
                bits: 0,
                fields: [None; 6],
            });
            layout.reports.len() - 1
        }
    };
    let report = &mut layout.reports[index];
    let start = report.bits;
    report.bits = start.checked_add(bits).ok_or(())?;
    if report.bits > packet_size * 8 {
        return Err(());
    }
    if !mouse || flags & 1 != 0 {
        return Ok(());
    } // Constant/padding/non-mouse.
    for element in 0..global.count {
        let field = match local.at(element) {
            Some(0x0009_0001..=0x0009_0003) => (local.at(element).unwrap() - 0x0009_0001) as usize,
            Some(0x0001_0030) => 3,
            Some(0x0001_0031) => 4,
            Some(0x0001_0038) => 5,
            _ => continue,
        };
        // Only scalar Variable inputs. Absolute pointers and relative buttons
        // cannot be fed to the relative-mouse API without another translation.
        if flags & 2 == 0
            || flags & 0x100 != 0
            || (flags & 4 != 0) != (field >= 3)
            || !(1..=32).contains(&global.size)
            || report.fields[field].is_some()
        {
            return Err(());
        }
        let minimum = global.minimum;
        let maximum = if minimum < 0 {
            signed(global.maximum, global.maximum_width)
        } else {
            global.maximum as i64
        };
        let low = if minimum < 0 {
            -(1i64 << (global.size - 1))
        } else {
            0
        };
        let high = (1i64 << (global.size - u32::from(minimum < 0))) - 1;
        if minimum < low
            || maximum > high
            || minimum > maximum
            || (field < 3 && (minimum != 0 || maximum != 1))
        {
            return Err(());
        }
        report.fields[field] = Some(Field {
            offset: start + element as usize * global.size as usize,
            width: global.size,
            minimum,
            maximum,
        });
    }
    Ok(())
}
