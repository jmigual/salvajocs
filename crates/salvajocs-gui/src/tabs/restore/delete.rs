use std::mem;

use eframe::egui::{self, RichText};
use salvajocs_core::retention::{self, DeleteError};
use salvajocs_core::storage::SnapshotInfo;

use super::RestoreTab;
use super::pick::snapshot_label;
use crate::job::Job;
use crate::state::lock_storage;
use crate::tabs::count;
use crate::theme;

/// Backups of one game waiting in the delete confirm dialog.
pub(super) struct Deletion {
    pub(super) title: String,
    pub(super) snapshots: Vec<SnapshotInfo>,
    /// Whether these are all of the game's backups.
    pub(super) all: bool,
    /// Whether Cancel still has to take the focus, once when the dialog opens.
    pub(super) focus: bool,
}

impl RestoreTab {
    /// The confirm dialog of a delete; Delete starts it in the listed repository.
    pub(super) fn delete_ui(&mut self, ctx: &egui::Context) {
        let backing_up = self.backing_up;
        let Some(deletion) = &mut self.confirm_delete else {
            return;
        };
        let (mut cancel, mut delete) = (false, false);
        let ((), dismissed) = theme::modal_dismissible(ctx, "restore_delete", 560.0, |ui| {
            let backups = count(deletion.snapshots.len(), "backup");
            ui.heading(format!("Delete {backups} of {}?", deletion.title));
            for snapshot in &deletion.snapshots {
                ui.label(snapshot_label(snapshot));
            }
            ui.label(
                "This removes them from the repository for every computer. It cannot be undone.",
            );
            if deletion.all {
                ui.weak("Your file choices for this game are kept.");
            }
            let error = ui.visuals().error_fg_color;
            theme::row_end(ui, |ui| {
                let button = ui.button("Cancel");
                // `row_end` first runs this in a sizing pass, whose widgets are not the real ones.
                if !ui.is_sizing_pass() && mem::take(&mut deletion.focus) {
                    button.request_focus();
                }
                cancel |= button.clicked();
                if backing_up {
                    ui.weak("Waiting for the running backup to end.");
                }
                let button = egui::Button::new(RichText::new("Delete").color(error).strong());
                delete |= ui.add_enabled(!backing_up, button).clicked();
            });
        });
        if delete {
            self.start_delete(ctx);
        } else if cancel || dismissed {
            self.confirm_delete = None;
        }
    }

    /// Deletes the backups of the confirm dialog in a job holding the storage lock.
    fn start_delete(&mut self, ctx: &egui::Context) {
        let (Some(deletion), Some(storage)) = (self.confirm_delete.take(), &self.repository) else {
            return;
        };
        let storage = storage.clone();
        let ids: Vec<String> = deletion.snapshots.into_iter().map(|s| s.id).collect();
        self.error = None;
        self.removed = None;
        self.deleting = Some(Job::spawn(ctx, move || {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            Ok(retention::delete(&mut *lock_storage(&storage)?, &ids)?)
        }));
    }
}

/// What a failed delete job tells the user, with how many backups it removed.
pub(super) fn describe_delete_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<DeleteError>() {
        Some(DeleteError::Stopped(stopped)) if stopped.forgotten > 0 => format!(
            "Removed {}, then the delete failed: {}",
            count(stopped.forgotten, "backup"),
            stopped.source
        ),
        Some(DeleteError::Stopped(stopped)) => format!("Nothing was removed: {}", stopped.source),
        Some(DeleteError::NotAGameBackup(_)) => {
            "The list was out of date, so nothing was removed".to_owned()
        }
        None => format!("Nothing was removed: {err:#}"),
        Some(_) => format!("The delete failed: {err:#}"),
    }
}
