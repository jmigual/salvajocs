//! One module per screen.

use crate::theme::icons;

/// `count` and `noun`, with an `s` unless `count` is one: `1 file`, `2 files`.
pub(crate) fn count(count: usize, noun: &str) -> String {
    let s = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{s}")
}

pub mod backup;
pub mod custom_games;
pub mod restore;
pub mod settings;

/// Which tab is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Backup,
    Restore,
    CustomGames,
    Settings,
}

impl Tab {
    /// Every tab, in the order of the tab strip.
    pub const ALL: [Tab; 4] = [Tab::Backup, Tab::Restore, Tab::CustomGames, Tab::Settings];

    /// The label on the tab strip.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Backup => "Backup",
            Tab::Restore => "Restore",
            Tab::CustomGames => "Custom games",
            Tab::Settings => "Settings",
        }
    }

    /// The icon before the label on the tab strip.
    pub fn icon(self) -> &'static str {
        match self {
            Tab::Backup => icons::FLOPPY_DISK,
            Tab::Restore => icons::CLOCK_COUNTER_CLOCKWISE,
            Tab::CustomGames => icons::GAME_CONTROLLER,
            Tab::Settings => icons::GEAR_SIX,
        }
    }
}
