//! Reading `.cargo_vcs_info.json`, which `cargo package` writes into a crates.io package.
//! Shared by `build.rs` and, for its tests, the library.

/// The commit and whether the tree was dirty, from the text of a `.cargo_vcs_info.json`, which
/// looks like `{"git":{"sha1":"<hex>","dirty":true},"path_in_vcs":"..."}` (`dirty` is absent when
/// clean). `None` when there is no commit in it.
pub fn parse(json: &str) -> Option<(String, bool)> {
    let compact: String = json.chars().filter(|c| !c.is_whitespace()).collect();
    let (_, rest) = compact.split_once("\"sha1\":\"")?;
    let (sha, _) = rest.split_once('"')?;
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((sha.to_owned(), compact.contains("\"dirty\":true")))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_a_clean_commit() {
        let json = r#"{"git":{"sha1":"0123456789abcdef0123456789abcdef01234567"},"path_in_vcs":"crates/salvajocs-gui"}"#;
        assert_eq!(
            parse(json),
            Some(("0123456789abcdef0123456789abcdef01234567".to_owned(), false))
        );
    }

    #[test]
    fn reads_a_dirty_commit_with_spaces() {
        let json = "{ \"git\": { \"sha1\": \"abc123\", \"dirty\": true } }";
        assert_eq!(parse(json), Some(("abc123".to_owned(), true)));
    }

    #[test]
    fn rejects_text_without_a_commit() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"git":{"sha1":""}}"#), None);
        assert_eq!(parse(r#"{"git":{"sha1":"not hex"}}"#), None);
    }
}
