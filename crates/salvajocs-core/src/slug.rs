//! Stable per-game identifiers for snapshot paths and tags (ADR-0010).

/// Longest name part of a slug, before the hash.
const MAX_NAME_LEN: usize = 40;

/// The slug of `title`: its ASCII alphanumeric runs, lowercased and joined by `-`, cut to 40
/// characters without a trailing `-`, then `-` and the FNV-1a 32 hash of the exact title as 8
/// hex digits. A title without ASCII alphanumerics gives the hash alone.
///
/// Snapshots are found by their slug, so its output must never change.
pub fn slug(title: &str) -> String {
    let name = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|run| !run.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase();
    // Byte slicing cannot split a character: the name is ASCII.
    let name = name[..name.len().min(MAX_NAME_LEN)].trim_end_matches('-');
    let hash = fnv1a32(title.as_bytes());
    if name.is_empty() {
        format!("{hash:08x}")
    } else {
        format!("{name}-{hash:08x}")
    }
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}
