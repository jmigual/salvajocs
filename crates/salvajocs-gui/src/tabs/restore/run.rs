use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Ui};
use salvajocs_core::restore::{self, RestorePlan, RestoreReport, RunError, SafetyBackup};

use super::RestoreTab;
use super::batch::remove_stale_restore_dirs;
use super::dirs::{own_dir, remove_dir};
use super::plan::ticked_plan;
use crate::job::Job;
use crate::state::{State, lock_storage};
use crate::theme::{self, icons};

/// What a finished restore shows.
pub(super) struct Restored {
    pub(super) title: String,
    pub(super) safety_backup: String,
    /// One line per write and for the registry, with whether it failed.
    pub(super) lines: Vec<(String, bool)>,
}

impl Restored {
    /// What `run` did for the game `title` with `plan`, as lines to show.
    pub(super) fn new(title: String, plan: &RestorePlan, report: RestoreReport) -> Self {
        let safety_backup = match report.safety_backup {
            SafetyBackup::Created(snapshot) => format!(
                "Safety backup: snapshot {} created, labelled pre-restore",
                snapshot.id.get(..8).unwrap_or(&snapshot.id)
            ),
            SafetyBackup::Unchanged => {
                "Safety backup: the repository already holds the replaced files".to_owned()
            }
            SafetyBackup::NothingExisted => {
                "Safety backup: none needed, nothing existed at the targets".to_owned()
            }
            _ => "Safety backup: unknown result".to_owned(),
        };
        let mut lines: Vec<_> = (plan.writes.iter().zip(report.files))
            .map(|(write, result)| match result {
                Ok(()) => (format!("{}: restored", write.target.display()), false),
                Err(err) => (
                    format!("{}: failed: {:#}", write.target.display(), anyhow!(err)),
                    true,
                ),
            })
            .collect();
        lines.extend(report.registry.map(|result| match result {
            Ok(()) => ("Registry: restored".to_owned(), false),
            Err(err) => (format!("Registry: failed: {:#}", anyhow!(err)), true),
        }));
        Self {
            title,
            safety_backup,
            lines,
        }
    }

    /// Draws the safety backup line and a line per write and for the registry.
    pub(super) fn lines_ui(&self, ui: &mut Ui) {
        ui.label(&self.safety_backup);
        let (accent, error_color) = (theme::palette(ui).accent, ui.visuals().error_fg_color);
        for (line, failed) in &self.lines {
            if *failed {
                theme::status(ui, icons::X_CIRCLE, error_color, line);
            } else {
                theme::icon_label(ui, icons::CHECK_CIRCLE, accent, line);
            }
        }
    }
}

impl RestoreTab {
    /// Downloads the planned snapshot into its cleared work dir, then runs the ticked part of the
    /// plan behind a safety backup staged in its own dir under the cache dir. A failed download
    /// writes nothing, not even the safety backup.
    pub(super) fn start_restore(&mut self, ctx: &egui::Context, state: &State) {
        let Some(preview) = &self.preview else {
            return;
        };
        self.error = None;
        self.restored = None;
        self.batch.clear_result();
        let plan = ticked_plan(
            &preview.plan,
            &preview.tree.keys,
            &preview.keys,
            &preview.selection,
        );
        let fetched = preview.fetched.clone();
        let stage_root = own_dir(&state.env.cache_dir, "restore-stage");
        let cache_dir = state.env.cache_dir.clone();
        let wineserver_running = state.env.wineserver_running;
        self.restoring = Some(Job::spawn(ctx, move || {
            // Each session makes dirs of its own, and an earlier one may have left its.
            remove_stale_restore_dirs(&cache_dir);
            let staged = &fetched.listed.staged;
            remove_dir(&staged.root)
                .with_context(|| format!("cannot clear {}", staged.root.display()))?;
            let mut storage = lock_storage(&fetched.storage)?;
            restore::download(&mut storage, &fetched.listed)
                .context("the backup cannot be fetched")?;
            let title = fetched.title.clone();
            let report =
                restore::run(&mut storage, &title, &plan, &stage_root, wineserver_running)?;
            Ok(Restored::new(title, &plan, report))
        }));
    }

    pub(super) fn restored_ui(&self, ui: &mut Ui) {
        let Some(restored) = &self.restored else {
            return;
        };
        ui.add_space(16.0);
        ui.heading(format!("Restored {}", restored.title));
        restored.lines_ui(ui);
    }
}

/// What a failed restore job tells the user; nothing was written.
pub(super) fn describe_run_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<RunError>() {
        Some(RunError::WineserverRunning) => "Nothing was restored: a Wine or Proton program is \
             running. Close the game and Wine first, then restore again."
            .to_owned(),
        _ => format!("Nothing was restored: {err:#}"),
    }
}
