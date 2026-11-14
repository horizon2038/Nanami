//! Two independent, checksummed records. Saving one never overwrites the
//! other, so an interrupted/short write retains the previous configuration.
use nanami_client::net::Ipv4Settings;
pub const RECORD_BYTES: usize = 64;
pub const FILE_BYTES: usize = 2 * RECORD_BYTES;
const MAGIC: &[u8; 8] = b"NNETCFG1";
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ *byte as u64).wrapping_mul(0x100000001b3)
    })
}
pub fn encode(settings: Ipv4Settings, revision: usize) -> [u8; RECORD_BYTES] {
    let mut record = [0; RECORD_BYTES];
    record[..8].copy_from_slice(MAGIC);
    record[8..16].copy_from_slice(&(revision as u64).to_le_bytes());
    record[16] = settings.mode as u8;
    for (offset, address) in [
        (24, settings.address),
        (28, settings.netmask),
        (32, settings.gateway),
        (36, settings.dns),
    ] {
        record[offset..offset + 4].copy_from_slice(&address);
    }
    let hash = checksum(&record[..40]);
    record[40..48].copy_from_slice(&hash.to_le_bytes());
    record
}
pub fn decode(record: &[u8]) -> Option<(Ipv4Settings, usize)> {
    if record.len() != RECORD_BYTES
        || &record[..8] != MAGIC
        || record[17..24].iter().chain(&record[48..]).any(|b| *b != 0)
    {
        return None;
    }
    if checksum(&record[..40]) != u64::from_le_bytes(record[40..48].try_into().ok()?) {
        return None;
    }
    let revision = usize::try_from(u64::from_le_bytes(record[8..16].try_into().ok()?)).ok()?;
    if revision == 0 {
        return None;
    }
    let settings = Ipv4Settings {
        mode: record[16] as usize,
        address: record[24..28].try_into().ok()?,
        netmask: record[28..32].try_into().ok()?,
        gateway: record[32..36].try_into().ok()?,
        dns: record[36..40].try_into().ok()?,
    };
    settings.validate_draft().ok()?;
    Some((settings, revision))
}
pub fn latest(bytes: &[u8]) -> Option<(Ipv4Settings, usize)> {
    if bytes.len() > FILE_BYTES {
        return None;
    }
    bytes
        .chunks_exact(RECORD_BYTES)
        .filter_map(decode)
        .max_by_key(|(_, revision)| *revision)
}
