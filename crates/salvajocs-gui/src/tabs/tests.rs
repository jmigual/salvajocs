use std::time::Duration;

use super::*;

#[test]
fn sizes_use_binary_units() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1023), "1023 B");
    assert_eq!(format_size(1024), "1.0 KiB");
    assert_eq!(format_size(1536 * 1024), "1.5 MiB");
    assert_eq!(format_size(u64::MAX), "16777216.0 TiB");
}

#[test]
fn times_read_as_utc_dates() {
    let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
    assert_eq!(format_time(at(0)), "1970-01-01 00:00 UTC");
    assert_eq!(format_time(at(1_709_210_096)), "2024-02-29 12:34 UTC");
    assert_eq!(format_time(at(1_798_761_540)), "2026-12-31 23:59 UTC");
}
