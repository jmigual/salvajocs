use std::collections::BTreeSet;

use eframe::egui::Ui;
use salvajocs_core::backup::{GameBackup, Stored};

use super::{BackupTab, Cleaned, lock};
use crate::state::chain;
use crate::tabs::{count, format_date};
use crate::theme::{self, icons};

/// What a backup did for one game.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// A new snapshot was stored.
    Saved,
    /// A new snapshot was stored because the user chose to back up this PC's saves over those of
    /// another PC, so they are now the newest.
    SavedAsNewest,
    /// The saves equal the newest snapshot, which is this PC's own.
    Unchanged,
    /// The saves equal the newest snapshot, from the host it names.
    SameAsNewest(String),
    /// Another PC has newer saves and this PC has not changed them since it last synced, so
    /// nothing was backed up.
    Behind {
        /// The host of the newest backup.
        host: String,
        /// When the newest backup was made.
        date: String,
    },
    /// This PC and the host it names changed the saves since this PC last synced, so nothing was
    /// backed up.
    ChangedOnBoth(String),
    /// This PC has no record of syncing the game and its saves differ from the newest backup,
    /// from the host it names, so nothing was backed up.
    UnknownBase(String),
    /// The game could not be backed up; holds why.
    Failed(String),
}

/// The severity of a result's mark.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Level {
    Good,
    Warn,
    Error,
}

impl Outcome {
    /// The icon in front of the game's line.
    pub(super) fn icon(&self) -> &'static str {
        match self {
            Self::Saved | Self::SavedAsNewest | Self::Unchanged | Self::SameAsNewest(_) => {
                icons::CHECK_CIRCLE
            }
            Self::Behind { .. } => icons::ARROW_DOWN,
            Self::ChangedOnBoth(_) | Self::UnknownBase(_) => icons::WARNING,
            Self::Failed(_) => icons::X_CIRCLE,
        }
    }

    /// The severity of the icon and the line.
    pub(super) fn level(&self) -> Level {
        match self {
            Self::Saved | Self::SavedAsNewest | Self::Unchanged | Self::SameAsNewest(_) => {
                Level::Good
            }
            Self::Behind { .. } | Self::ChangedOnBoth(_) | Self::UnknownBase(_) => Level::Warn,
            Self::Failed(_) => Level::Error,
        }
    }

    /// The game's line: `title`, then what happened and, when nothing was backed up, why and
    /// what to do.
    pub(super) fn line(&self, title: &str) -> String {
        match self {
            Self::Saved => format!("{title}: backed up"),
            Self::SavedAsNewest => {
                format!("{title}: backed up; this PC's saves are now the newest")
            }
            Self::Unchanged => format!("{title}: up to date"),
            Self::SameAsNewest(host) => {
                format!("{title}: same as the newest backup, from {host}")
            }
            Self::Behind { host, date } => format!(
                "{title}: not backed up: {host} has newer saves, from {date}, and this PC has not \
                 changed them since it last synced. To use them, restore them on the Restore \
                 tab. To keep this PC's saves instead, use Back up anyway under the game."
            ),
            Self::ChangedOnBoth(host) => format!(
                "{title}: not backed up: both this PC and {host} changed the saves since this PC \
                 last synced. Nothing was overwritten. Back up asks again next time."
            ),
            Self::UnknownBase(host) => format!(
                "{title}: not backed up: salvajocs does not know which backup this PC is in sync \
                 with (for example after upgrading), and the saves differ \
                 from the newest backup, from {host}. Nothing was overwritten. Back up asks \
                 again next time."
            ),
            Self::Failed(error) => format!("{title}: failed: {error}"),
        }
    }
}

/// One game's line in the results.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct GameResult {
    pub(super) title: String,
    pub(super) outcome: Outcome,
}

/// The result of `backup`. A game stored while in `back_up_anyway` is one the user chose to back
/// up over another PC's saves.
pub(super) fn summarize(backup: &GameBackup, back_up_anyway: &BTreeSet<String>) -> GameResult {
    let outcome = match &backup.result {
        Ok(Stored::New(_)) if back_up_anyway.contains(&backup.slug) => Outcome::SavedAsNewest,
        Ok(Stored::New(_)) => Outcome::Saved,
        Ok(Stored::UpToDate { elsewhere: None }) => Outcome::Unchanged,
        Ok(Stored::UpToDate {
            elsewhere: Some(newest),
        }) => Outcome::SameAsNewest(newest.host.clone()),
        Ok(Stored::Behind { newest }) => Outcome::Behind {
            host: newest.host.clone(),
            date: format_date(newest.time),
        },
        Ok(Stored::ChangedOnBoth { newest }) => Outcome::ChangedOnBoth(newest.host.clone()),
        Ok(Stored::UnknownBase { newest }) => Outcome::UnknownBase(newest.host.clone()),
        Ok(other) => Outcome::Failed(format!("unexpected backup result: {other:?}")),
        Err(err) => Outcome::Failed(chain(err)),
    };
    GameResult {
        title: backup.title.clone(),
        outcome,
    }
}

impl BackupTab {
    pub(super) fn results_ui(&self, ui: &mut Ui) {
        let progress = lock(&self.progress);
        let error_color = ui.visuals().error_fg_color;
        if let Some(error) = &self.run_error {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("The backup failed: {error}"),
            );
        }
        for error in &self.shared.errors {
            theme::status(ui, icons::X_CIRCLE, error_color, error);
        }
        for failed in &self.saving.failed {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!(
                    "What to back up was not saved for {failed}; it is tried again at the next Back up"
                ),
            );
        }
        let warn = ui.visuals().warn_fg_color;
        for warning in self.shared.warnings.iter().chain(&self.saving.warnings) {
            theme::status(ui, icons::WARNING, warn, warning);
        }
        let accent = theme::palette(ui).accent;
        if self.shared.written > 0 {
            let text = format!(
                "Saved {} to the repository",
                count(self.shared.written, "custom game")
            );
            theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
        }
        if self.shared.rules.is_some() {
            let text = "Saved the Old backups rules";
            theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
        }
        if let Some(Cleaned {
            rules_failed,
            policy_error,
            done,
            error,
        }) = &self.cleanup
        {
            if *rules_failed {
                let text = "The Old backups rules were not saved, so no old backup was removed";
                theme::status(ui, icons::WARNING, warn, text);
            }
            if let Some(error) = policy_error {
                let text = "The cleanup rules could not be read, so no old backup was removed";
                theme::status(ui, icons::WARNING, warn, format!("{text}: {error}"));
            }
            if done.forgotten > 0 {
                let removed = count(done.forgotten, "old backup");
                let text = format!("Removed {removed}");
                theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
            }
            if done.append_only_kept > 0 {
                let kept = count(done.append_only_kept, "old backup");
                let text = format!("The repository is append-only, so {kept} stayed");
                theme::status(ui, icons::WARNING, warn, text);
            }
            if let Some(error) = error {
                let text = format!("Cleanup did not finish: {error}");
                theme::status(ui, icons::X_CIRCLE, error_color, text);
            }
        }
        if progress.done.is_empty() {
            return;
        }
        ui.add_space(16.0);
        ui.heading("Last backup");
        ui.spacing_mut().item_spacing.y = 4.0;
        for GameResult { title, outcome } in &progress.done {
            let text = outcome.line(title);
            match outcome.level() {
                Level::Good => theme::icon_label(ui, outcome.icon(), accent, text),
                Level::Warn => theme::status(ui, outcome.icon(), warn, text),
                Level::Error => theme::status(ui, outcome.icon(), error_color, text),
            }
        }
    }
}
