use eframe::egui::Ui;
use salvajocs_core::backup::{GameBackup, Stored};

use super::{BackupTab, Cleaned, lock};
use crate::state::chain;
use crate::tabs::count;
use crate::theme::{self, icons};

/// What a backup did for one game.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// A new snapshot was stored.
    Saved,
    /// Nothing changed since this machine's last snapshot.
    Unchanged,
    /// Nothing changed since this machine's last snapshot, but the newest one, from the host it
    /// names, differs.
    UnchangedNewerElsewhere(String),
    /// The saves equal the newest snapshot, from the host it names.
    SameAsNewest(String),
    /// The game could not be backed up; holds why.
    Failed(String),
}

/// One game's line in the results.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct GameResult {
    pub(super) title: String,
    pub(super) outcome: Outcome,
}

pub(super) fn summarize(backup: &GameBackup) -> GameResult {
    let outcome = match &backup.result {
        Ok(Stored::New(_)) => Outcome::Saved,
        Ok(Stored::Unchanged {
            newer_elsewhere: None,
        }) => Outcome::Unchanged,
        Ok(Stored::Unchanged {
            newer_elsewhere: Some(newest),
        }) => Outcome::UnchangedNewerElsewhere(newest.host.clone()),
        Ok(Stored::SameAsNewest(newest)) => Outcome::SameAsNewest(newest.host.clone()),
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
            let text = match outcome {
                Outcome::Saved => format!("{title}: backed up"),
                Outcome::Unchanged => format!("{title}: up to date"),
                Outcome::UnchangedNewerElsewhere(host) => format!(
                    "{title}: up to date with this PC's last backup; the newest backup, from \
                     {host}, differs"
                ),
                Outcome::SameAsNewest(host) => {
                    format!("{title}: same as the newest backup, from {host}")
                }
                Outcome::Failed(error) => {
                    theme::status(
                        ui,
                        icons::X_CIRCLE,
                        error_color,
                        format!("{title}: failed: {error}"),
                    );
                    continue;
                }
            };
            theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
        }
    }
}
