use std::path::Path;

use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Ui};
use salvajocs_core::restore::{self, RestorePlan, RestoreReport, RunError, SafetyBackup};
use salvajocs_core::storage::SnapshotInfo;
use salvajocs_core::sync::{self, Base};

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
    /// One line per write and for the registry, with whether it failed, and one when the base
    /// could not be updated.
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

    /// Updates this PC's base of the game of `snapshot`, which this restore wrote, in the
    /// repository `scope` (ADR-0035) when no write failed: `snapshot` becomes the base, a
    /// pre-restore safety backup too. A failed update adds a line and leaves the restore as it
    /// is. Call it while holding the repository's storage lock.
    pub(super) fn update_base(&mut self, config_dir: &Path, scope: &str, snapshot: &SnapshotInfo) {
        let slug = (snapshot.tags.iter()).find_map(|tag| tag.strip_prefix("game:"));
        let Some(slug) = slug.filter(|_| self.lines.iter().all(|(_, failed)| !failed)) else {
            return;
        };
        let base = Base::new(snapshot.id.clone(), None);
        if let Err(err) = sync::record(config_dir, scope, [(slug.to_owned(), base)]) {
            let line = format!("This PC's sync state was not updated: {:#}", anyhow!(err));
            self.lines.push((line, true));
        }
    }

    /// Draws the safety backup line and each of `lines`.
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
        let (Some(preview), Some(config)) = (&self.preview, &state.config) else {
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
        let (config_dir, scope) = (state.env.config_dir.clone(), config.secret_scope());
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
            let mut restored = Restored::new(title, &plan, report);
            restored.update_base(&config_dir, &scope, &fetched.listed.snapshot);
            Ok(restored)
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
