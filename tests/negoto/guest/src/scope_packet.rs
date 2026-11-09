//! Independent decoder assertions for the two known functional-fixture reports.
pub fn validate(packet: &[u8], mode: u64) {
    assert!(mode <= 1);
    assert_eq!(packet.len(), 8272);
    let word = |index: usize| u64::from_le_bytes(packet[index * 8..index * 8 + 8].try_into().unwrap());
    assert_eq!(word(0), 0x0031_5043_534e_414e);
    assert_eq!((word(1), word(2), word(5), word(6), word(7)), (mode + 1, mode, 1, 513, 1));
    assert!(word(4) >= word(3));
    for number in 0..513 {
        let expected = match number {
            39 if mode == 0 => 7,
            39 => 5,
            57 | 60 | 61 | 512 if mode == 0 => 1,
            157 if mode == 0 => 2,
            _ => 0,
        };
        assert_eq!(word(8 + number * 2), expected, "row={number}");
        let ticks = word(9 + number * 2);
        if mode == 0 || number != 39 {
            assert_eq!(ticks, 0);
        } else {
            assert!(ticks > 0 && ticks <= word(4) - word(3));
        }
    }
}
