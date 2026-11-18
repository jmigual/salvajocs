use super::*;

// These normalize_title tests cover ludusavi's `can_normalize_title` cases (src/scan/title.rs).

#[test]
fn normalize_title_lowercases() {
    assert_eq!(normalize_title("foo bar"), "foo bar");
    assert_eq!(normalize_title("Foo Bar"), "foo bar");
}

#[test]
fn normalize_title_drops_punctuated_editions() {
    assert_eq!(normalize_title("Foo Bar: Any Arbitrary Edition"), "foo bar");
    assert_eq!(
        normalize_title("Foo Bar - Any Arbitrary Edition"),
        "foo bar"
    );
    assert_eq!(normalize_title("Foo Bar™ Any Arbitrary Edition"), "foo bar");
    assert_eq!(
        normalize_title("Foo Bar® - Any Arbitrary Edition"),
        "foo bar"
    );
}

#[test]
fn normalize_title_drops_game_of_the_year_edition() {
    assert_eq!(
        normalize_title("Foo Bar Game of the Year Edition"),
        "foo bar"
    );
}

#[test]
fn normalize_title_drops_one_word_editions() {
    assert_eq!(normalize_title("Foo Bar Special Edition"), "foo bar");
}

#[test]
fn normalize_title_drops_year_suffix() {
    assert_eq!(normalize_title("Foo Bar (2000)"), "foo bar");
}

#[test]
fn normalize_title_replaces_symbols() {
    assert_eq!(normalize_title("Foo:Bar"), "foo bar");
    assert_eq!(normalize_title("Foo: Bar"), "foo bar");
    assert_eq!(normalize_title("Fo'o Bar"), "foo bar");
    assert_eq!(normalize_title("Foo \"Bar\""), "foo bar");
}

#[test]
fn normalize_title_collapses_spaces() {
    assert_eq!(normalize_title("  Foo  Bar  "), "foo bar");
}

/// salvajocs' own cases, beyond ludusavi's.
#[test]
fn normalize_title_salvajocs_cases() {
    assert_eq!(
        normalize_title("Foo Bar Very Special Edition"),
        "foo bar very"
    );
    assert_eq!(normalize_title("Foo Bar (Remake)"), "foo bar (remake)");
    assert_eq!(normalize_title("Foo’s “Bar”"), "foos bar");
    assert_eq!(normalize_title("\tFoo Bar\n"), "foo bar");
}
