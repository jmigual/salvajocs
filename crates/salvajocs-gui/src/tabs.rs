//! One module per screen.

use std::time::{SystemTime, UNIX_EPOCH};

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

/// A UTC time as `YYYY-MM-DD HH:MM UTC`.
pub(crate) fn format_time(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's `civil_from_days`, for days since 1970-01-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

pub mod backup;
pub mod custom_games;
mod mapping;
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
