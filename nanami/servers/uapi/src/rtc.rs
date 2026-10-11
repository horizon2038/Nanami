use crate::Word;

pub const RTC_SERVICE_REQUEST_READ: Word = 0x7001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub const fn pack_rtc_date(dt: RtcDateTime) -> Word {
    ((dt.year as Word) << 16) | ((dt.month as Word) << 8) | dt.day as Word
}

pub const fn pack_rtc_time(dt: RtcDateTime) -> Word {
    ((dt.hour as Word) << 16) | ((dt.minute as Word) << 8) | dt.second as Word
}

pub const fn unpack_rtc_datetime(date: Word, time: Word) -> RtcDateTime {
    RtcDateTime {
        year: ((date >> 16) & 0xffff) as u16,
        month: ((date >> 8) & 0xff) as u8,
        day: (date & 0xff) as u8,
        hour: ((time >> 16) & 0xff) as u8,
        minute: ((time >> 8) & 0xff) as u8,
        second: (time & 0xff) as u8,
    }
}
