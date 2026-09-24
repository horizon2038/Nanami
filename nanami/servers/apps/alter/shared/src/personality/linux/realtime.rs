use super::{map_request_error, refresh_clock, Runtime, Word, EIO};
use nanami_services::rtc::RtcDateTime;

pub(super) fn realtime(runtime: &mut Runtime) -> Result<(Word, Word), i32> {
    let (epoch, anchor) = if let Some(anchor) = runtime.realtime_anchor {
        refresh_clock(runtime)?;
        anchor
    } else {
        nanami_services::registry::connect_rtc_service(crate::abi::SLOT_RTC_SERVICE)
            .map_err(map_request_error)?;
        let port = libnanami::ipc::process_slot_descriptor(crate::abi::SLOT_RTC_SERVICE);
        let date = nanami_services::rtc::rtc_service_read(port).map_err(map_request_error)?;
        let epoch = unix_seconds(date).ok_or(EIO)?;
        refresh_clock(runtime)?;
        let anchor = (epoch, runtime.monotonic_ticks);
        runtime.realtime_anchor = Some(anchor);
        anchor
    };
    // Cache only the epoch anchor, never the current time. Both clock APIs
    // advance from a fresh timer-service sample; no raw counter is exposed.
    let elapsed = runtime.monotonic_ticks.saturating_sub(anchor);
    let (seconds, nanos) = super::clocks::split_ticks(elapsed, runtime.monotonic_tick_hz);
    Ok((epoch.saturating_add(seconds), nanos))
}

fn unix_seconds(date: RtcDateTime) -> Option<Word> {
    let year = date.year as Word;
    if year < 1970
        || !(1..=12).contains(&date.month)
        || date.hour > 23
        || date.minute > 59
        || date.second > 59
    {
        return None;
    }
    let leap = |year: Word| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if date.day == 0 || date.day as Word > months[date.month as usize - 1] {
        return None;
    }
    let leaps_before = |year: Word| (year - 1) / 4 - (year - 1) / 100 + (year - 1) / 400;
    let days = (year - 1970) * 365 + leaps_before(year) - leaps_before(1970)
        + months[..date.month as usize - 1].iter().sum::<Word>()
        + date.day as Word
        - 1;
    Some(days * 86400 + date.hour as Word * 3600 + date.minute as Word * 60 + date.second as Word)
}
