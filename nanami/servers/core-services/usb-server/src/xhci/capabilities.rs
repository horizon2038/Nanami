//! Checked extended-capability traversal, independent of MMIO/BIOS handoff.

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub value: Option<u32>,
    pub reason: &'static str,
}

pub struct Capabilities {
    next: usize,
    bytes: usize,
}

#[derive(Debug)]
pub struct Capability {
    pub offset: usize,
    pub header: u32,
    end: usize,
}

impl Capabilities {
    pub fn new(hcc: u32, bytes: usize) -> Self {
        Self {
            next: ((hcc >> 16) as usize) * 4,
            bytes,
        }
    }

    pub fn next(
        &mut self,
        read: &mut impl FnMut(usize) -> u32,
    ) -> Result<Option<Capability>, Error> {
        let offset = self.next;
        if offset == 0 {
            return Ok(None);
        }
        if offset > self.bytes.saturating_sub(4) || self.bytes < 4 {
            return Err(Error {
                offset,
                value: None,
                reason: "extended capability header outside BAR",
            });
        }
        let header = read(offset);
        let delta = ((header >> 8) & 0xff) as usize * 4;
        let next = if delta == 0 {
            0
        } else {
            offset
                .checked_add(delta)
                .filter(|&next| next <= self.bytes - 4)
                .ok_or(Error {
                    offset,
                    value: Some(header),
                    reason: "next extended capability outside BAR",
                })?
        };
        // Nonzero links move strictly forward; BAR bounds bound the walk.
        // The next header also bounds this capability's body (including PSI).
        self.next = next;
        Ok(Some(Capability {
            offset,
            header,
            end: if next == 0 { self.bytes } else { next },
        }))
    }
}

impl Capability {
    pub fn require(&self, bytes: usize) -> Result<(), Error> {
        if bytes > self.end - self.offset {
            return Err(Error {
                offset: self.offset,
                value: Some(self.header),
                reason: "extended capability body truncated or overlaps next header",
            });
        }
        Ok(())
    }

    pub fn protocol(
        &self,
        read: &mut impl FnMut(usize) -> u32,
        claimed: &mut [bool],
    ) -> Result<SupportedProtocol, Error> {
        self.require(16)?;
        let name = read(self.offset + 4);
        let compatible = read(self.offset + 8);
        let first = (compatible & 0xff) as usize;
        let count = ((compatible >> 8) & 0xff) as usize;
        let psi_count = (compatible >> 28) as usize;
        self.require(16 + psi_count * 4)?;
        let error = |reason| Error {
            offset: self.offset + 8,
            value: Some(compatible),
            reason,
        };
        // QEMU p3=0 retains an empty USB3 entry. It claims no ports.
        let ports = if count == 0 {
            0..0
        } else {
            first..first + count
        };
        if count != 0 && (first == 0 || ports.end > claimed.len()) {
            return Err(error("protocol port range outside MaxPorts"));
        }
        if claimed[ports.clone()].iter().any(|&used| used) {
            return Err(error("overlapping protocol port ranges"));
        }
        claimed[ports.clone()].fill(true);
        let slot_type = read(self.offset + 12) as u8 & 31;
        let mut psi = [0u32; 15];
        for (index, value) in psi[..psi_count].iter_mut().enumerate() {
            *value = read(self.offset + 16 + index * 4);
        }
        Ok(SupportedProtocol {
            name,
            major: (self.header >> 24) as u8,
            minor: (self.header >> 16) as u8,
            ports,
            slot_type,
            psi,
            psi_count,
        })
    }
}

pub struct SupportedProtocol {
    pub name: u32,
    pub major: u8,
    pub minor: u8,
    pub ports: core::ops::Range<usize>,
    pub slot_type: u8,
    pub psi: [u32; 15],
    pub psi_count: usize,
}
