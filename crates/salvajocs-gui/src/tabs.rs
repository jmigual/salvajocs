//! One module per screen. Backup and Restore are empty until their steps land.

pub mod custom_games;
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
}
