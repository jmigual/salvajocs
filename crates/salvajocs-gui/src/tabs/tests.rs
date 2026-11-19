use super::*;

#[test]
fn sizes_use_binary_units() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1023), "1023 B");
    assert_eq!(format_size(1024), "1.0 KiB");
    assert_eq!(format_size(1536 * 1024), "1.5 MiB");
    assert_eq!(format_size(u64::MAX), "16777216.0 TiB");
}
