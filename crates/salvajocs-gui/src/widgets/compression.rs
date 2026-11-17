//! The compression field: a dropdown of presets and a custom level, validated by
//! `Compression::from_field` (ADR-0009).

use eframe::egui::{ComboBox, Ui};
use salvajocs_core::storage::{Compression, StorageError, ZSTD_DEFAULT_LEVEL};

use super::{field_hint, field_status, labelled_edit};
use crate::theme::{self, icons};

/// Why the text of the custom level is not a compression setting.
#[derive(Debug, thiserror::Error)]
pub enum FieldError {
    /// The text is not an integer.
    #[error("the level must be a whole number")]
    NotANumber,
    /// The integer is outside the levels zstd accepts.
    #[error(transparent)]
    OutOfRange(#[from] StorageError),
}

impl FieldError {
    /// The error as the form shows it: a sentence that says which levels are valid.
    pub fn message(&self) -> String {
        const RANGE: &str = "Use -7 to -1, or 1 to 22 (0 is Off).";
        match self {
            Self::NotANumber => format!("Enter a whole number. {RANGE}"),
            Self::OutOfRange(StorageError::InvalidCompression(level)) => {
                format!("Level {level} is out of range. {RANGE}")
            }
            Self::OutOfRange(other) => other.to_string(),
        }
    }
}

/// Reads the custom level text: `0` is off, otherwise a zstd level.
pub fn parse(text: &str) -> Result<Compression, FieldError> {
    let level = text.trim().parse().map_err(|_| FieldError::NotANumber)?;
    Ok(Compression::from_field(Some(level))?)
}

/// An entry of the dropdown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Choice {
    Off,
    Fast,
    Default,
    Smaller,
    Smallest,
    Custom,
}

const CHOICES: [Choice; 6] = [
    Choice::Off,
    Choice::Fast,
    Choice::Default,
    Choice::Smaller,
    Choice::Smallest,
    Choice::Custom,
];

impl Choice {
    fn label(self) -> String {
        match self {
            Self::Off => "Off".to_string(),
            Self::Fast => "Fast (level 1)".to_string(),
            Self::Default => format!("Default (level {ZSTD_DEFAULT_LEVEL})"),
            Self::Smaller => "Smaller (level 9)".to_string(),
            Self::Smallest => "Smallest (level 19)".to_string(),
            Self::Custom => "Custom level".to_string(),
        }
    }

    /// The setting of a preset; `None` for [`Choice::Custom`], which has its own text.
    fn preset(self) -> Option<Compression> {
        match self {
            Self::Off => Some(Compression::Off),
            Self::Fast => Some(Compression::Level(1)),
            Self::Default => Some(Compression::Default),
            Self::Smaller => Some(Compression::Level(9)),
            Self::Smallest => Some(Compression::Level(19)),
            Self::Custom => None,
        }
    }
}

/// `Level(3)` is what a repository reports after `Default` was stored, so they count as one.
fn canonical(compression: Compression) -> Compression {
    match compression {
        Compression::Level(ZSTD_DEFAULT_LEVEL) => Compression::Default,
        other => other,
    }
}

/// The state of one compression field.
pub struct CompressionField {
    choice: Choice,
    custom: String,
}

impl CompressionField {
    /// A field showing `compression`: the preset it equals, else "Custom level" with its number.
    pub fn new(compression: Compression) -> Self {
        let compression = canonical(compression);
        match CHOICES.iter().find(|c| c.preset() == Some(compression)) {
            Some(&choice) => Self {
                choice,
                custom: String::new(),
            },
            None => Self {
                choice: Choice::Custom,
                custom: match compression {
                    Compression::Level(level) => level.to_string(),
                    _ => String::new(),
                },
            },
        }
    }

    /// The setting the field holds, or why the custom level is not one.
    pub fn value(&self) -> Result<Compression, FieldError> {
        match self.choice.preset() {
            Some(preset) => Ok(preset),
            None => parse(&self.custom),
        }
    }

    /// The field's setting when it is valid and differs from `stored`.
    pub fn change_from(&self, stored: Compression) -> Option<Compression> {
        let new = self.value().ok()?;
        (canonical(new) != canonical(stored)).then_some(new)
    }

    /// Draws the field as grid rows: the dropdown, the custom level when chosen with its error,
    /// then `hint`.
    pub fn ui(&mut self, ui: &mut Ui, label: &str, hint: &str) {
        let name = ui.label(label);
        ComboBox::from_id_salt(label)
            // Taller than the default 200 px, so the six entries need no scrolling.
            .height(300.0)
            .selected_text(self.choice.label())
            .show_ui(ui, |ui| {
                theme::pill_list(ui);
                for choice in CHOICES {
                    ui.selectable_value(&mut self.choice, choice, choice.label());
                }
            })
            .response
            .labelled_by(name.id);
        ui.end_row();
        if self.choice == Choice::Custom {
            labelled_edit(ui, "Level number", &mut self.custom, false);
            ui.end_row();
            if let Err(err) = parse(&self.custom) {
                ui.label("");
                let color = ui.visuals().error_fg_color;
                field_status(ui, icons::X_CIRCLE, color, err.message());
                ui.end_row();
            }
        }
        ui.label("");
        field_hint(ui, hint);
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_off_and_levels_pass() {
        assert!(matches!(parse(" 0 "), Ok(Compression::Off)));
        assert!(matches!(parse("19"), Ok(Compression::Level(19))));
        assert!(matches!(parse("-7"), Ok(Compression::Level(-7))));
    }

    #[test]
    fn out_of_range_empty_and_non_numbers_are_rejected() {
        for text in ["23", "-8", "100"] {
            assert!(matches!(
                parse(text),
                Err(FieldError::OutOfRange(StorageError::InvalidCompression(_)))
            ));
        }
        for text in ["fast", ""] {
            assert!(matches!(parse(text), Err(FieldError::NotANumber)));
        }
    }

    #[test]
    fn the_error_message_names_the_level_and_the_valid_range() {
        let out_of_range = parse("99").unwrap_err().message();
        assert!(
            out_of_range.starts_with("Level 99 is out of range."),
            "{out_of_range}"
        );
        let not_a_number = parse("fast").unwrap_err().message();
        for message in [out_of_range, not_a_number] {
            assert!(
                message.contains("-7 to -1, or 1 to 22 (0 is Off)"),
                "{message}"
            );
        }
    }

    #[test]
    fn default_and_its_stored_level_select_the_same_entry() {
        let default = CompressionField::new(Compression::Default);
        let stored = CompressionField::new(Compression::Level(ZSTD_DEFAULT_LEVEL));
        assert_eq!(default.choice, Choice::Default);
        assert_eq!(stored.choice, Choice::Default);
        assert!(stored.change_from(Compression::Default).is_none());
    }

    #[test]
    fn a_level_without_a_preset_selects_the_custom_entry_with_its_number() {
        for level in [5, 22, -3] {
            let field = CompressionField::new(Compression::Level(level));
            assert_eq!(field.choice, Choice::Custom);
            assert_eq!(field.custom, level.to_string());
            assert!(matches!(field.value(), Ok(Compression::Level(l)) if l == level));
        }
    }

    #[test]
    fn presets_round_trip_and_an_invalid_custom_level_has_no_value() {
        for compression in [
            Compression::Off,
            Compression::Level(1),
            Compression::Default,
            Compression::Level(9),
            Compression::Level(19),
        ] {
            let field = CompressionField::new(compression);
            assert_ne!(field.choice, Choice::Custom);
            assert_eq!(field.value().unwrap(), compression);
        }
        let mut field = CompressionField::new(Compression::Level(5));
        field.custom = "23".to_string();
        assert!(field.value().is_err());
        assert!(field.change_from(Compression::Off).is_none());
    }
}
