use super::{connector, ddc, edid, gen9::Registers, gfx::display::*, vbt, RequestError, Word};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
};

type Reply = Result<(Word, Word, Word), RequestError>;
#[derive(Default)]
struct Rpc {
    replies: VecDeque<Reply>,
    calls: Vec<[Word; 7]>,
}
thread_local! { static RPC: RefCell<Rpc> = RefCell::new(Rpc::default()); }
pub(super) fn call_port(call: [Word; 7]) -> Reply {
    RPC.with(|rpc| {
        let mut rpc = rpc.borrow_mut();
        rpc.calls.push(call);
        rpc.replies.pop_front().expect("Unexpected IPC call")
    })
}
fn responses(replies: impl IntoIterator<Item = Reply>) {
    RPC.with(|rpc| {
        *rpc.borrow_mut() = Rpc {
            replies: replies.into_iter().collect(),
            calls: Vec::new(),
        }
    });
}
fn output() -> connector::Connector {
    connector::Connector {
        route: connector::Route {
            port: 1,
            kind: CONNECTOR_HDMI,
            pipe: 0,
        },
        edid: edid::Edid::parse(&fixture()).unwrap(),
        width: 1920,
        height: 1080,
    }
}

#[test]
fn client_reads_real_connector_metadata_edid_and_detailed_timings_without_allocation() {
    use super::{display_client as client, gfx::*};
    let output = output();
    let (a, b) = output.info().words();
    let mut replies = vec![Ok((0, a, b))];
    for offset in (0..256).step_by(16) {
        let (a, b) = output.read_edid(offset).unwrap();
        replies.push(Ok((0, a, b)));
    }
    for part in 0..2 {
        let (a, b) = output.monitor_mode(0, part).unwrap();
        replies.push(Ok((0, a, b)));
    }
    replies.push(Ok((0, MODE_END, 0)));
    replies.push(Ok((0, 0, 0)));
    responses(replies);
    let mut buffer = [0xa5; 272];
    assert_eq!(client::read_edid(20, 0, &mut buffer).unwrap(), 256);
    assert_eq!(&buffer[..256], &fixture());
    assert_eq!(&buffer[256..], &[0xa5; 16]);
    assert_eq!(
        client::monitor_mode(20, 0, 0).unwrap(),
        Some(output.edid.timings[0])
    );
    assert_eq!(client::monitor_mode(20, 0, 2).unwrap(), None);
    assert_eq!(client::connector_info(20, 1).unwrap(), None);
    RPC.with(|rpc| {
        let rpc = rpc.borrow();
        assert!(rpc.replies.is_empty());
        assert_eq!(
            rpc.calls[0],
            [20, DISPLAY_SERVICE_REQUEST_CONNECTOR_INFO, 0, 0, 0, 0, 2]
        );
        for (index, offset) in (0..256).step_by(16).enumerate() {
            assert_eq!(
                rpc.calls[index + 1],
                [20, DISPLAY_SERVICE_REQUEST_READ_EDID, 0, offset, 0, 0, 3]
            );
        }
        assert_eq!(
            rpc.calls[17],
            [20, DISPLAY_SERVICE_REQUEST_MONITOR_MODE, 0, 0, 0, 0, 4]
        );
        assert_eq!(
            rpc.calls[18],
            [20, DISPLAY_SERVICE_REQUEST_MONITOR_MODE, 0, 0, 1, 0, 4]
        );
        assert_eq!(
            rpc.calls[19],
            [20, DISPLAY_SERVICE_REQUEST_MONITOR_MODE, 0, 2, 0, 0, 4]
        );
        assert_eq!(
            rpc.calls[20],
            [20, DISPLAY_SERVICE_REQUEST_CONNECTOR_INFO, 1, 0, 0, 0, 2]
        );
    });
}

#[test]
fn client_rejects_short_buffers_and_malformed_wire_records_before_further_ipc() {
    use super::display_client as client;
    let (a, b) = output().info().words();
    responses([Ok((0, a, b))]);
    let mut buffer = [0xa5; 255];
    assert_eq!(
        client::read_edid(20, 0, &mut buffer),
        Err(RequestError::InvalidArgument)
    );
    assert_eq!(buffer, [0xa5; 255]);
    for malformed in [127, 257, (1usize << 32) | b, 0] {
        responses([Ok((0, a, malformed))]);
        assert_eq!(client::connector_info(20, 0), Err(RequestError::Protocol));
    }
    responses([Ok((0, MODE_END, 1))]);
    assert_eq!(client::monitor_mode(20, 0, 0), Err(RequestError::Protocol));
    let [a, h, v] = output().edid.timings[0].words();
    responses([Ok((0, a, h)), Ok((0, v, 1))]);
    assert_eq!(client::monitor_mode(20, 0, 0), Err(RequestError::Protocol));
    responses([Err(RequestError::Transport)]);
    assert_eq!(client::connector_info(20, 0), Err(RequestError::Transport));
    responses([Ok((7, 0, 0))]);
    assert_eq!(client::connector_info(20, 0), Err(RequestError::Status(7)));
}

fn checksum(block: &mut [u8]) {
    block[127] = 0;
    block[127] = 0u8.wrapping_sub(block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}
fn fixture() -> [u8; 256] {
    let mut bytes = [0u8; 256];
    bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    bytes[18] = 1;
    bytes[19] = 4;
    bytes[24] = 2;
    bytes[126] = 1;
    bytes[54..72].copy_from_slice(&[
        2, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0, 0, 0, 0, 0, 0, 0x1e,
    ]);
    bytes[128] = 2;
    bytes[129] = 3;
    bytes[130] = 4;
    bytes[132..150].copy_from_slice(&[
        1, 0x1d, 0, 0x72, 0x51, 0xd0, 0x1e, 0x20, 0x6e, 0x28, 0x55, 0, 0, 0, 0, 0, 0, 0x1e,
    ]);
    checksum(&mut bytes[..128]);
    checksum(&mut bytes[128..]);
    bytes
}

#[test]
fn edid_detailed_timings_and_preferred_mode_round_trip_the_public_wire_format() {
    let edid = edid::Edid::parse(&fixture()).unwrap();
    assert_eq!(edid.len, 256);
    assert!(!edid.truncated);
    assert_eq!(edid.timing_count, 2);
    let preferred = edid.timings[0];
    assert_eq!(
        (preferred.hactive, preferred.vactive, preferred.clock_khz),
        (1920, 1080, 148500)
    );
    assert_eq!(
        (preferred.hsync_start, preferred.hsync_end, preferred.htotal),
        (2008, 2052, 2200)
    );
    assert_eq!(
        (preferred.vsync_start, preferred.vsync_end, preferred.vtotal),
        (1084, 1089, 1125)
    );
    assert_eq!(
        preferred.flags,
        MODE_PREFERRED | MODE_HSYNC_POSITIVE | MODE_VSYNC_POSITIVE
    );
    assert_eq!(Timing::from_words(preferred.words()), Some(preferred));
    assert_eq!(
        (edid.timings[1].hactive, edid.timings[1].vactive),
        (1280, 720)
    );
}

#[test]
fn malformed_edid_does_not_fabricate_monitor_modes() {
    let mut bytes = fixture();
    bytes[0] = 1;
    checksum(&mut bytes[..128]);
    assert!(edid::Edid::parse(&bytes).is_err());
    let mut bytes = fixture();
    bytes[22] ^= 1;
    assert!(edid::Edid::parse(&bytes).is_err());
    let mut bytes = fixture();
    bytes[130] = 3;
    checksum(&mut bytes[128..]);
    assert!(edid::Edid::parse(&bytes).is_err());
    let mut bytes = fixture();
    bytes[71] |= 0x80;
    checksum(&mut bytes[..128]);
    let edid = edid::Edid::parse(&bytes[..128]).unwrap();
    assert_eq!(edid.timing_count, 0);
    assert!(edid.truncated);
    for len in [0, 1, 127, 129, 255] {
        assert!(edid::Edid::parse(&fixture()[..len]).is_err());
    }
}

#[test]
fn duplicate_cta_timings_are_deduplicated_and_missing_extensions_are_reported() {
    let mut bytes = fixture();
    bytes[126] = 3;
    let dtd = bytes[54..72].to_vec();
    bytes[132..150].copy_from_slice(&dtd);
    checksum(&mut bytes[..128]);
    checksum(&mut bytes[128..]);
    let edid = edid::Edid::parse(&bytes).unwrap();
    assert!(edid.truncated);
    assert_eq!(edid.timing_count, 1);
    assert_ne!(edid.timings[0].flags & MODE_PREFERRED, 0);
}

#[test]
fn active_hdmi_dp_and_edp_routes_are_read_from_the_real_transcoder_registers() {
    let mut regs = super::registers();
    regs.registers.insert(0x60400, (1 << 31) | (1 << 28));
    regs.registers.insert(0x64100, 1 << 31);
    assert_eq!(
        connector::active_route(&regs, 0).unwrap(),
        connector::Route {
            port: 1,
            kind: CONNECTOR_HDMI,
            pipe: 0
        }
    );
    regs.registers
        .insert(0x60400, (1 << 31) | (1 << 28) | (2 << 24));
    assert_eq!(
        connector::active_route(&regs, 0).unwrap().kind,
        CONNECTOR_DP
    );
    regs.registers.insert(0x60400, 0);
    regs.registers.insert(0x64000, 1 << 31);
    regs.registers
        .insert(0x6f400, (1 << 31) | (2 << 24) | (5 << 12));
    assert_eq!(
        connector::active_route(&regs, 1).unwrap(),
        connector::Route {
            port: 0,
            kind: CONNECTOR_EDP,
            pipe: 1
        }
    );
    assert!(connector::active_route(&regs, 0).is_err());
    regs.registers
        .insert(0x61400, (1 << 31) | (1 << 28) | (2 << 24));
    assert!(connector::active_route(&regs, 1).is_err()); // One pipe cloned to two outputs.
}

#[test]
fn pch_family_and_vbt_override_select_the_correct_ddc_pins() {
    use connector::Pch::*;
    assert_eq!(connector::Pch::from_id(0x8086, 0xa148), Some(Sunrise));
    assert_eq!(connector::Pch::from_id(0x8086, 0xa2c8), Some(Sunrise));
    assert_eq!(connector::Pch::from_id(0x8086, 0xa305), Some(Cannon));
    assert_eq!(connector::Pch::from_id(0x8086, 0x1234), None);
    assert_eq!(connector::Pch::from_id(0x1234, 0xa305), None);
    assert_eq!(Sunrise.pin(1, 0).unwrap(), 5);
    assert_eq!(Cannon.pin(1, 0).unwrap(), 1);
    assert_eq!(Sunrise.pin(3, 0).unwrap(), 6);
    assert_eq!(Cannon.pin(3, 0).unwrap(), 4);
    assert_eq!(Cannon.pin(1, 2).unwrap(), 2);
    assert!(Sunrise.pin(4, 0).is_err());
    assert!(Cannon.pin(4, 0).is_err());
}

fn vbt_fixture() -> Vec<u8> {
    let mut bytes = vec![0u8; 48 + 22 + 3 + 5 + 33];
    let size = bytes.len() as u16;
    bytes[..4].copy_from_slice(b"$VBT");
    bytes[22..24].copy_from_slice(&48u16.to_le_bytes());
    bytes[24..26].copy_from_slice(&size.to_le_bytes());
    bytes[28..32].copy_from_slice(&48u32.to_le_bytes());
    bytes[48..64].copy_from_slice(b"BIOS_DATA_BLOCK ");
    bytes[64..66].copy_from_slice(&196u16.to_le_bytes());
    bytes[66..68].copy_from_slice(&22u16.to_le_bytes());
    bytes[68..70].copy_from_slice(&(size - 48).to_le_bytes());
    bytes[70] = 2;
    bytes[71..73].copy_from_slice(&38u16.to_le_bytes());
    bytes[77] = 33;
    let child = 78;
    bytes[child + 2] = 0x60;
    bytes[child + 16] = 1;
    bytes[child + 19] = 5;
    bytes[child + 25] = 0x10;
    bytes[26] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    bytes
}

#[test]
fn vbt_wiring_uses_validated_variable_length_bdb_records() {
    let bytes = vbt_fixture();
    assert_eq!(
        vbt::wiring(&bytes, 1).unwrap(),
        Some(vbt::Wiring {
            ddc_pin: 5,
            aux: Some(1)
        })
    );
    assert_eq!(vbt::wiring(&bytes, 2).unwrap(), None);
    for len in 0..bytes.len() {
        assert!(vbt::wiring(&bytes[..len], 1).is_err());
    }
    let mut broken = bytes.clone();
    broken[77] = 0;
    broken[26] = 0;
    broken[26] = 0u8.wrapping_sub(broken.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    assert!(vbt::wiring(&broken, 1).is_err());
}

#[test]
fn vbt_merges_complementary_hdmi_dp_wiring_but_rejects_conflicting_board_routes() {
    let mut bytes = vbt_fixture();
    let mut dp = bytes[78..111].to_vec();
    bytes[78 + 25] = 0;
    dp[16] = 7; // DP B and HDMI B refer to the same DDI.
    dp[19] = 0;
    bytes.extend_from_slice(&dp);
    let size = bytes.len() as u16;
    bytes[24..26].copy_from_slice(&size.to_le_bytes());
    bytes[68..70].copy_from_slice(&(size - 48).to_le_bytes());
    bytes[71..73].copy_from_slice(&71u16.to_le_bytes());
    bytes[26] = 0;
    bytes[26] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    assert_eq!(
        vbt::wiring(&bytes, 1).unwrap(),
        Some(vbt::Wiring {
            ddc_pin: 5,
            aux: Some(1)
        })
    );
    bytes[111 + 19] = 6;
    bytes[26] = 0;
    bytes[26] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
    assert!(vbt::wiring(&bytes, 1).is_err());
}

#[test]
fn opregion_two_zero_and_two_one_have_different_rvda_address_semantics() {
    let mut region = vec![0u8; 8192];
    region[..16].copy_from_slice(b"IntelGraphicsMem");
    region[16..20].copy_from_slice(&8u32.to_le_bytes());
    region[23] = 2;
    region[88] = 4;
    region[0x3ba..0x3c2].copy_from_slice(&0x9000_0000u64.to_le_bytes());
    region[0x3c2..0x3c6].copy_from_slice(&256u32.to_le_bytes());
    assert_eq!(
        vbt::extended_vbt(&region, 0x8000_0000).unwrap(),
        Some((0x9000_0000, 256))
    );
    region[22] = 1;
    region[0x3ba..0x3c2].copy_from_slice(&8192u64.to_le_bytes());
    assert_eq!(
        vbt::extended_vbt(&region, 0x8000_0000).unwrap(),
        Some((0x8000_2000, 256))
    );
    region[0x3ba..0x3c2].copy_from_slice(&1024u64.to_le_bytes());
    assert!(vbt::extended_vbt(&region, 0x8000_0000).is_err());
    assert!(vbt::embedded_vbt(&region).is_ok()); // Malformed RVDA doesn't hide an embedded VBT.
}

#[derive(Default)]
struct Clock {
    ticks: usize,
}
impl ddc::Delay for Clock {
    fn millisecond(&mut self) -> Result<(), RequestError> {
        self.ticks += 1;
        if self.ticks > 300 {
            return Err(RequestError::Transport);
        }
        Ok(())
    }
}
struct LinkState {
    regs: BTreeMap<Word, u32>,
    edid: [u8; 256],
    pointer: usize,
    gmbus_status: u32,
    gmbus_remaining: usize,
    writes: Vec<(Word, u32)>,
    packets: Vec<Vec<u8>>,
    nak: bool,
    stuck: bool,
    defer: usize,
    partial: usize,
    write_count_reply: bool,
}
impl Default for LinkState {
    fn default() -> Self {
        Self {
            regs: [
                (0xc5100, 0x104),
                (0xc5110, 2),
                (0xc5120, 0x1234),
                (0xc2020, 0x100),
            ]
            .into_iter()
            .collect(),
            edid: fixture(),
            pointer: 0,
            gmbus_status: 0,
            gmbus_remaining: 0,
            writes: Vec::new(),
            packets: Vec::new(),
            nak: false,
            stuck: false,
            defer: 0,
            partial: 16,
            write_count_reply: false,
        }
    }
}
#[derive(Default)]
struct Link(RefCell<LinkState>);
impl Registers for Link {
    fn read(&self, offset: Word) -> u32 {
        let mut state = self.0.borrow_mut();
        if offset == 0xc5108 {
            if state.gmbus_status != 0 && state.nak {
                return 1 << 10;
            }
            return state.gmbus_status;
        }
        if offset == 0xc510c && state.gmbus_remaining != 0 {
            let value = u32::from_le_bytes(
                state.edid[state.pointer..state.pointer + 4]
                    .try_into()
                    .unwrap(),
            );
            state.pointer += 4;
            state.gmbus_remaining -= 4;
            if state.gmbus_remaining == 0 {
                state.gmbus_status = (1 << 14) | (1 << 9);
            }
            return value;
        }
        state.regs.get(&offset).copied().unwrap_or(0)
    }
    fn write(&mut self, offset: Word, value: u32) {
        let state = self.0.get_mut();
        state.writes.push((offset, value));
        state.regs.insert(offset, value);
        if offset == 0xc5104 {
            if value & (1 << 30) != 0 && value & (4 << 25) != 0 {
                state.gmbus_status = 0;
            } else if value & (1 << 30) != 0 {
                state.pointer = ((value >> 8) & 0xff) as usize;
                state.gmbus_remaining = ((value >> 16) & 511) as usize;
                state.gmbus_status = if state.stuck {
                    1 << 9
                } else {
                    (1 << 11) | (1 << 9)
                };
            }
        }
        if (0x64010..=0x64410).contains(&offset) && offset & 0xff == 0x10 {
            if value & (1 << 31) == 0 {
                state
                    .regs
                    .insert(offset, value & !((1 << 30) | (1 << 28) | (1 << 25)));
                return;
            }
            let size = ((value >> 20) & 31) as usize;
            let mut packet = Vec::new();
            for index in 0..5 {
                packet.extend_from_slice(
                    &state
                        .regs
                        .get(&(offset + 4 + index * 4))
                        .copied()
                        .unwrap_or(0)
                        .to_be_bytes(),
                );
            }
            packet.truncate(size);
            state.packets.push(packet.clone());
            if state.stuck {
                return;
            }
            let mut reply = vec![0];
            if state.defer != 0 {
                state.defer -= 1;
                reply[0] = 0x80;
            } else if state.nak {
                reply[0] = 0x40;
            } else if packet.len() == 5 {
                state.pointer = packet[4] as usize;
                if state.write_count_reply {
                    reply.push(1);
                }
            } else if packet.len() == 4 {
                let count = (packet[3] as usize + 1).min(state.partial);
                reply.extend_from_slice(&state.edid[state.pointer..state.pointer + count]);
                state.pointer += count;
            }
            state
                .regs
                .insert(offset, (1 << 30) | ((reply.len() as u32) << 20));
            for (index, part) in reply.chunks(4).enumerate() {
                let mut bytes = [0; 4];
                bytes[..part.len()].copy_from_slice(part);
                state
                    .regs
                    .insert(offset + 4 + index * 4, u32::from_be_bytes(bytes));
            }
        }
    }
    fn pte(&self, _: Word) -> u64 {
        panic!("DDC must not access GGTT");
    }
    fn write_pte(&mut self, _: Word, _: u64) {
        panic!("DDC must not access GGTT");
    }
}

#[test]
fn gmbus_reads_two_blocks_and_restores_pin_index_irq_mask_and_clock_gate() {
    let mut link = Link::default();
    let before = link.0.borrow().regs.clone();
    let edid = ddc::gmbus(&mut link, &mut Clock::default(), 5).unwrap();
    assert_eq!(edid.bytes, fixture());
    assert_eq!(edid.timing_count, 2);
    let state = link.0.borrow();
    for offset in [0xc5100, 0xc5110, 0xc5120, 0xc2020] {
        assert_eq!(state.regs.get(&offset), before.get(&offset));
    }
    assert_eq!(
        state
            .writes
            .iter()
            .filter(|(offset, value)| *offset == 0xc5104 && *value & (4 << 25) != 0)
            .count(),
        2
    );
}

#[test]
fn gmbus_does_not_touch_an_in_use_engine_and_nak_restores_the_idle_engine() {
    let mut link = Link::default();
    link.0.get_mut().gmbus_status = (1 << 9) | (1 << 15);
    assert!(ddc::gmbus(&mut link, &mut Clock::default(), 5).is_err());
    assert!(link.0.borrow().writes.is_empty());
    let mut link = Link::default();
    link.0.get_mut().nak = true;
    // The NAK branch must go idle before clearing the error.
    // This model reports no ACTIVE with NAK, as hardware does after address NAK.
    assert!(ddc::gmbus(&mut link, &mut Clock::default(), 5).is_err());
    assert_eq!(link.0.borrow().regs[&0xc5100], 0x104);
    assert_eq!(link.0.borrow().regs[&0xc2020], 0x100);
}

#[test]
fn aux_i2c_handles_defer_short_reads_and_write_count_replies_then_releases_mot() {
    let mut link = Link::default();
    link.0.get_mut().defer = 2;
    link.0.get_mut().partial = 7;
    link.0.get_mut().write_count_reply = true;
    let mut clock = Clock::default();
    let edid = ddc::aux(&mut link, &mut clock, 1).unwrap();
    assert_eq!(edid.bytes, fixture());
    assert!(clock.ticks >= 2);
    let state = link.0.borrow();
    assert_eq!(state.packets.last().unwrap(), &[0x10, 0, 0x50]);
    assert!(state
        .writes
        .iter()
        .filter(|(offset, _)| *offset == 0x64110)
        .all(|(_, value)| *value & (1 << 29) == 0));
    assert_eq!(state.regs[&0x64110], 0);
}

#[test]
fn aux_timeout_never_restores_data_or_restarts_a_still_busy_transaction() {
    let mut link = Link::default();
    link.0.get_mut().stuck = true;
    let mut clock = Clock::default();
    assert!(ddc::aux(&mut link, &mut clock, 1).is_err());
    let state = link.0.borrow();
    assert_eq!(state.packets.len(), 1);
    assert_eq!(clock.ticks, 5);
    assert_ne!(state.regs[&0x64110] & (1 << 31), 0);
    assert_eq!(state.writes.last().unwrap().0, 0x64110);
}

#[test]
fn aux_busy_at_entry_is_not_touched_and_nak_restores_idle_channel_state() {
    let mut link = Link::default();
    link.0.get_mut().regs.insert(0x64110, 1 << 31);
    assert!(ddc::aux(&mut link, &mut Clock::default(), 1).is_err());
    assert!(link.0.borrow().writes.is_empty());
    let mut link = Link::default();
    link.0.get_mut().nak = true;
    link.0.get_mut().regs.insert(0x64114, 0x1234);
    assert!(ddc::aux(&mut link, &mut Clock::default(), 1).is_err());
    let state = link.0.borrow();
    assert_eq!(state.regs[&0x64110], 0);
    assert_eq!(state.regs[&0x64114], 0x1234);
    assert_eq!(state.packets.last().unwrap(), &[0x10, 0, 0x50]);
}

#[test]
fn aux_repeated_defer_and_empty_ack_do_not_hang_or_fabricate_edid() {
    let mut link = Link::default();
    link.0.get_mut().defer = usize::MAX;
    let mut clock = Clock::default();
    assert!(ddc::aux(&mut link, &mut clock, 1).is_err());
    assert_eq!(clock.ticks, 12); // Seven attempts for data and for MOT cleanup.
    assert_eq!(link.0.borrow().packets.len(), 14);
    let mut link = Link::default();
    link.0.get_mut().partial = 0;
    assert!(ddc::aux(&mut link, &mut Clock::default(), 1).is_err());
    assert_eq!(link.0.borrow().packets.len(), 3); // Offset, empty read, STOP.
}

#[test]
fn gmbus_timeout_is_bounded_and_does_not_restore_configuration_while_active() {
    let mut link = Link::default();
    link.0.get_mut().stuck = true;
    let mut clock = Clock::default();
    assert!(ddc::gmbus(&mut link, &mut clock, 5).is_err());
    assert_eq!(clock.ticks, 100);
    let state = link.0.borrow();
    assert_ne!(state.gmbus_status & (1 << 9), 0);
    assert_eq!(state.regs[&0xc5100], 5); // No restore over an active owned engine.
    assert_eq!(state.regs[&0xc5110], 0);
    assert_eq!(state.regs[&0xc2020], 0x80000100);
    assert!(state
        .writes
        .iter()
        .all(|(reg, _)| matches!(reg, 0xc5100 | 0xc5104 | 0xc5110 | 0xc5120 | 0xc2020)));
}

#[test]
fn corrupt_extension_retains_a_valid_base_and_reports_truncation() {
    let mut link = Link::default();
    link.0.get_mut().edid[150] ^= 1;
    let edid = ddc::aux(&mut link, &mut Clock::default(), 1).unwrap();
    assert_eq!(edid.len, 128);
    assert!(edid.truncated);
    assert_eq!(edid.timing_count, 1);
}

#[test]
fn metadata_packets_report_actual_edid_length_and_validate_query_offsets() {
    let connector = connector::Connector {
        route: connector::Route {
            port: 1,
            kind: CONNECTOR_HDMI,
            pipe: 0,
        },
        edid: edid::Edid::parse(&fixture()).unwrap(),
        width: 1920,
        height: 1080,
    };
    let info = connector.info();
    let (a, b) = info.words();
    assert_eq!(ConnectorInfo::from_words(a, b), Some(info));
    assert_eq!(info.flags, CONNECTOR_ACTIVE | CONNECTOR_EDID_VALID);
    assert!(connector.read_edid(256).is_err());
    assert!(connector.monitor_mode(0, 2).is_err());
    assert_eq!(connector.monitor_mode(2, 0).unwrap(), (MODE_END, 0));
    let (clock, h) = connector.monitor_mode(0, 0).unwrap();
    let (v, reserved) = connector.monitor_mode(0, 1).unwrap();
    assert_eq!(reserved, 0);
    assert_eq!(
        Timing::from_words([clock, h, v]),
        Some(connector.edid.timings[0])
    );
}
