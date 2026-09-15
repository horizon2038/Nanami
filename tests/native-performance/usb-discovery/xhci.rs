use super::{input::Input, RequestError};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Disk {
    pub slot: u8,
    pub lun: u8,
}

#[derive(Default)]
pub struct Controller {
    disks: Vec<Disk>,
    changed: bool,
    pub failed: bool,
    pub add_during_probe: bool,
    pub remove_during_probe: bool,
    pub scans: usize,
    pub commands: usize,
}
impl Controller {
    pub fn attach(&mut self) {
        self.disks.push(Disk {
            slot: self.disks.len() as u8 + 1,
            lun: 0,
        });
        self.changed = true;
    }
    pub fn detach(&mut self) {
        self.disks.clear();
        self.changed = true;
    }
    pub fn take_storage_change(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
    pub fn poll(&mut self, _: &mut Input) {
        if std::mem::take(&mut self.add_during_probe) {
            self.attach();
        }
        if std::mem::take(&mut self.remove_during_probe) {
            self.detach();
        }
    }
    pub fn storage_disks(&mut self, _: &mut Input) -> Result<Vec<Disk>, RequestError> {
        self.scans += 1;
        if self.failed {
            Err(RequestError::Transport)
        } else {
            Ok(self.disks.clone())
        }
    }
    pub fn disk_present(&self, disk: Disk) -> bool {
        !self.failed && self.disks.contains(&disk)
    }
    pub fn scsi(
        &mut self,
        disk: Disk,
        cdb: &[u8],
        data: &mut [u8],
        input: bool,
        _: &mut Input,
    ) -> Result<(bool, usize), RequestError> {
        assert!(input);
        self.commands += 1;
        if !self.disk_present(disk) {
            return Err(RequestError::Transport);
        }
        match cdb[0] {
            0x00 | 0x12 => data.fill(0), // Ready, direct-access block device.
            0x25 => {
                data[..4].copy_from_slice(&4095u32.to_be_bytes());
                data[4..].copy_from_slice(&512u32.to_be_bytes());
            }
            0x28 => {
                let lba = u32::from_be_bytes(cdb[2..6].try_into().unwrap());
                let (header, entries) = gpt();
                data.copy_from_slice(match lba {
                    1 => &header,
                    2 => &entries,
                    _ => panic!("unexpected LBA"),
                });
            }
            _ => panic!("unexpected SCSI command"),
        }
        Ok((true, data.len()))
    }
}

fn gpt() -> ([u8; 512], [u8; 512]) {
    let mut entries = [0; 512];
    entries[..16].copy_from_slice(&nanami_gpt::NANAMI_ROOT_TYPE_GUID);
    entries[32..40].copy_from_slice(&34u64.to_le_bytes());
    entries[40..48].copy_from_slice(&4000u64.to_le_bytes());
    let mut header = [0; 512];
    header[..8].copy_from_slice(b"EFI PART");
    for (offset, value) in [(24, 1u64), (32, 4095), (40, 34), (48, 4062), (72, 2)] {
        header[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (8, 0x10000u32),
        (12, 92),
        (80, 4),
        (84, 128),
        (88, nanami_gpt::crc32(&entries)),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let crc = nanami_gpt::crc32(&header[..92]);
    header[16..20].copy_from_slice(&crc.to_le_bytes());
    (header, entries)
}
