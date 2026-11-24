//! The system clock reads the host's time zone: a hold's `--between`
//! window is the host's local time of day (factorium#286). Its own test
//! binary, so setting `TZ` races no other test.

use factorium_host::{Clock, SystemClock};

#[test]
#[allow(unsafe_code)]
fn the_system_clock_reads_its_offset_from_the_time_zone() {
    // SAFETY: the only test of this binary, before anything reads the time zone.
    unsafe { std::env::set_var("TZ", "Asia/Tokyo") };
    let clock = SystemClock;
    assert_eq!(clock.offset_min(clock.now()), 9 * 60, "Tokyo keeps UTC+9, no daylight saving");
}
