//! The compression field: one integer, validated by `Compression::from_field` (ADR-0009).

use eframe::egui::Ui;
use salvajocs_core::storage::{Compression, StorageError};

use super::labelled_edit;

/// Why the text of a compression field is not a compression setting.
#[derive(Debug, thiserror::Error)]
pub enum FieldError {
    /// The text is not an integer.
    #[error("compression must be a whole number")]
    NotANumber,
    /// The integer is outside the levels zstd accepts.
    #[error(transparent)]
    OutOfRange(#[from] StorageError),
}

/// Reads the field text: empty is the default level, `0` is off, otherwise a zstd level.
pub fn parse(text: &str) -> Result<Compression, FieldError> {
    let text = text.trim();
    let field = if text.is_empty() {
        None
    } else {
        Some(text.parse().map_err(|_| FieldError::NotANumber)?)
    };
    Ok(Compression::from_field(field)?)
}

/// The field text that [`parse`] reads back as `compression`.
pub fn format(compression: Compression) -> String {
    match compression {
        Compression::Default => String::new(),
        Compression::Off => "0".to_string(),
        Compression::Level(level) => level.to_string(),
    }
}

/// Draws the field as a grid row (label, field, hint) and the error below it when the text is
/// not valid. Returns the parsed setting, or the error that is shown.
pub fn ui(ui: &mut Ui, label: &str, text: &mut String) -> Result<Compression, FieldError> {
    labelled_edit(ui, label, text, false);
    ui.weak("Empty for the default. 0 turns it off; 1 to 22, or -7 to -1 for faster.");
    ui.end_row();
    let parsed = parse(text);
    if let Err(err) = &parsed {
        ui.label("");
        ui.colored_label(ui.visuals().error_fg_color, err.to_string());
        ui.end_row();
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default_zero_is_off_and_levels_pass() {
        assert!(matches!(parse(""), Ok(Compression::Default)));
        assert!(matches!(parse(" 0 "), Ok(Compression::Off)));
        assert!(matches!(parse("19"), Ok(Compression::Level(19))));
        assert!(matches!(parse("-7"), Ok(Compression::Level(-7))));
    }

    #[test]
    fn out_of_range_and_non_numbers_are_rejected() {
        for text in ["23", "-8", "100"] {
            assert!(matches!(
                parse(text),
                Err(FieldError::OutOfRange(StorageError::InvalidCompression(_)))
            ));
        }
        assert!(matches!(parse("fast"), Err(FieldError::NotANumber)));
    }

    #[test]
    fn format_round_trips_through_parse() {
        for compression in [
            Compression::Default,
            Compression::Off,
            Compression::Level(-3),
        ] {
            let text = format(compression);
            assert_eq!(
                format(parse(&text).expect("a formatted setting parses")),
                text
            );
        }
    }
}
