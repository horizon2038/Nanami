use super::*;
use crate::usb::mass_storage;
use alloc::vec;

pub(super) fn probe(
    controllers: &mut [Controller],
    input: &mut Input,
    timer: Word,
) -> Result<Option<Root>, RequestError> {
    let mut root = None;
    for (index, controller) in controllers.iter_mut().enumerate() {
        for disk in controller.storage_disks(input)? {
            if let Some(partition) = probe_disk(controller, disk, input, timer)? {
                libnanami::println!(
                    "[usb-server] root controller={} slot={} lun={} LBA={} sectors={}",
                    index,
                    disk.slot,
                    disk.lun,
                    partition.first_lba,
                    partition.sector_count
                );
                if root
                    .replace(Root {
                        controller: index,
                        disk,
                        partition,
                    })
                    .is_some()
                {
                    libnanami::print!(
                        "[usb-server] multiple Nanami USB roots; refusing selection\n"
                    );
                    return Err(RequestError::Protocol);
                }
            }
        }
    }
    Ok(root)
}

fn probe_disk(
    controller: &mut Controller,
    disk: Disk,
    input: &mut Input,
    timer: Word,
) -> Result<Option<nanami_gpt::Partition>, RequestError> {
    let mut inquiry = [0; 36];
    let (ok, bytes) = controller.scsi(disk, &[0x12, 0, 0, 0, 36, 0], &mut inquiry, true, input)?;
    if !ok || bytes < 5 || inquiry[0] != 0 {
        return Ok(None);
    }
    let mut ready = false;
    for _ in 0..30 {
        if controller.scsi(disk, &[0; 6], &mut [], true, input)?.0 {
            ready = true;
            break;
        }
        let mut sense = [0; 18];
        let (ok, bytes) = controller.scsi(disk, &[3, 0, 0, 0, 18, 0], &mut sense, true, input)?;
        if !ok || bytes < 14 {
            return Ok(None);
        }
        let (key, asc) = match sense[0] & 0x7f {
            0x70 | 0x71 => (sense[2] & 15, sense[12]),
            0x72 | 0x73 => (sense[1] & 15, sense[2]),
            _ => return Err(RequestError::Protocol),
        };
        // Empty card-reader LUNs and unsupported devices are not root disks.
        if asc == 0x3a || !matches!(key, 2 | 6) {
            return Ok(None);
        }
        crate::delay(timer, 100)?;
    }
    if !ready {
        return Err(RequestError::Transport);
    }
    let mut capacity = [0; 8];
    exact(
        controller,
        disk,
        &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &mut capacity,
        input,
    )?;
    let last = u32::from_be_bytes(capacity[..4].try_into().unwrap());
    let mut sector_bytes = u32::from_be_bytes(capacity[4..].try_into().unwrap());
    let sectors = if last == u32::MAX {
        let mut capacity = [0; 32];
        let mut cdb = [0; 16];
        cdb[0] = 0x9e;
        cdb[1] = 0x10;
        cdb[13] = 32;
        exact(controller, disk, &cdb, &mut capacity, input)?;
        sector_bytes = u32::from_be_bytes(capacity[8..12].try_into().unwrap());
        u64::from_be_bytes(capacity[..8].try_into().unwrap())
            .checked_add(1)
            .ok_or(RequestError::Protocol)?
    } else {
        last as u64 + 1
    };
    // The common GPT/block API currently describes 512-byte logical sectors.
    // Reject 4Kn instead of interpreting its LBAs as byte offsets incorrectly.
    if sector_bytes != 512 || sectors < 3 {
        libnanami::println!(
            "[usb-server] unsupported medium slot={} lun={} sector-bytes={}",
            disk.slot,
            disk.lun,
            sector_bytes
        );
        return Ok(None);
    }
    let mut sector = [0; 512];
    read(controller, disk, 1, &mut sector, input)?;
    let header = match nanami_gpt::parse_primary_header(&sector, sectors) {
        Ok(header) => header,
        Err(_) => return Ok(None),
    };
    let bytes = header
        .partition_entry_bytes()
        .map_err(|_| RequestError::Protocol)?;
    let mut entries = vec![0; bytes.div_ceil(512) * 512];
    read(
        controller,
        disk,
        header.partition_entry_lba,
        &mut entries,
        input,
    )?;
    match nanami_gpt::find_nanami_root(header, &entries) {
        Ok(partition) => Ok(Some(partition)),
        Err(nanami_gpt::Error::Ambiguous) => Err(RequestError::Protocol),
        Err(_) => Ok(None),
    }
}

fn exact(
    controller: &mut Controller,
    disk: Disk,
    cdb: &[u8],
    data: &mut [u8],
    input: &mut Input,
) -> Result<(), RequestError> {
    let (ok, bytes) = controller.scsi(disk, cdb, data, true, input)?;
    if ok && bytes == data.len() {
        Ok(())
    } else {
        Err(RequestError::Protocol)
    }
}

fn read(
    controller: &mut Controller,
    disk: Disk,
    lba: u64,
    data: &mut [u8],
    input: &mut Input,
) -> Result<(), RequestError> {
    let (cdb, len) = mass_storage::read_write(lba, (data.len() / 512) as u32, false)
        .map_err(|_| RequestError::InvalidArgument)?;
    exact(controller, disk, &cdb[..len], data, input)
}
