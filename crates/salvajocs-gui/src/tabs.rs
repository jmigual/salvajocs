//! One module per screen.

use crate::theme::icons;

/// `count` and `noun`, with an `s` unless `count` is one: `1 file`, `2 files`.
pub(crate) fn count(count: usize, noun: &str) -> String {
    let s = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{s}")
}

/// A byte count with a binary unit, such as `1.5 MiB`.
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub mod backup;
pub mod custom_games;
pub mod restore;
pub mod settings;
mod tree;

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

#[cfg(test)]
mod tests;
