use std::cell::OnceCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Ui};
use salvajocs_core::backup;
use salvajocs_core::config::AppConfig;
use salvajocs_core::layout::Hashes;
use salvajocs_core::manifest::Manifest;
use salvajocs_core::restore::{self, RestorePlan, RestoreReport, RunError, SafetyBackup};
use salvajocs_core::scan::Environment;
use salvajocs_core::selection::{GameSettings, Pending};
use salvajocs_core::slug::slug;
use salvajocs_core::storage::{SnapshotInfo, Storage};
use salvajocs_core::sync::{self, Base};

use super::RestoreTab;
use super::batch::remove_stale_restore_dirs;
use super::dirs::{own_dir, remove_dir};
use super::plan::{Fetched, deletes_on, ticked_plan};
use crate::job::Job;
use crate::state::{State, lock_storage};
use crate::tabs::backup::choices::{self, Games, Saved};
use crate::tabs::backup::run::to_stage;
use crate::theme::{self, icons};

/// What a finished restore shows.
pub(super) struct Restored {
    pub(super) title: String,
    pub(super) safety_backup: String,
    /// One line per write, for the registry and per deletion, with whether it failed, and one
    /// when the base could not be updated or was updated without this PC's saves.
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
        let deleted = (plan.deletes.iter().zip(report.deletes)).map(|(delete, result)| {
            let target = delete.target.display();
            match result {
                Ok(()) => (format!("{target}: deleted"), false),
                Err(err) => (format!("{target}: failed: {:#}", anyhow!(err)), true),
            }
        });
        lines.extend(deleted);
        Self {
            title,
            safety_backup,
            lines,
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

/// What a restore job needs to update this PC's base of the game it restored (ADR-0035): the
/// restored backup, with this PC's saves after it as Back up stages them. It also checks the
/// deletions again against what to back up as it is now (ADR-0036).
pub(super) struct BaseUpdate {
    manifest: Arc<Manifest>,
    config: AppConfig,
    environment: Option<Environment>,
    config_dir: PathBuf,
    /// Back up's staging mirror, so the list is the one a Back up would stage.
    stage_root: PathBuf,
    /// What Back up would filter by, read by the first game that needs it and kept for the job:
    /// a later game's list is at worst stale, which shows it changed.
    choices: OnceCell<Result<(Pending, Games), String>>,
}

impl BaseUpdate {
    /// The update for the repository of `config`, with `state`'s manifest, environment and dirs.
    pub(super) fn new(state: &State, config: &AppConfig) -> Self {
        Self {
            manifest: state.manifest_or_empty(),
            config: config.clone(),
            environment: state.env.environment.clone(),
            config_dir: state.env.config_dir.clone(),
            stage_root: state.env.cache_dir.join("stage"),
            choices: OnceCell::new(),
        }
    }

    /// Keeps only the deletions of `plan`, made from `fetched`, that `restore::deletions` still
    /// offers with the game's selection read now, for each game afresh, so a file unticked in
    /// what to back up since the plan is kept (ADR-0036). Returns a result line for each file it
    /// keeps; when the selection cannot be read, it deletes nothing and returns one failed line
    /// instead, which also keeps [`Self::apply`] from recording the base. Call it under the
    /// storage lock taken for the restore.
    pub(super) fn recheck_deletes(
        &self,
        storage: &mut Storage,
        fetched: &Fetched,
        plan: &mut RestorePlan,
    ) -> Vec<(String, bool)> {
        if plan.deletes.is_empty() {
            return Vec::new();
        }
        let read = selections(storage, &self.config_dir, &self.config.secret_scope());
        let Some(today) = selection_of(&read, &fetched.title) else {
            plan.deletes.clear();
            let line = "No file was deleted: what to back up of this game cannot be read, so this \
                        PC's sync state was not updated.";
            return vec![(line.to_owned(), true)];
        };
        let (listed, vars, info) = (&fetched.listed, &fetched.vars, &fetched.info);
        let offered: HashSet<PathBuf> =
            (restore::deletions(listed, plan, vars, info, Some(&today)))
                .files
                .into_iter()
                .map(|delete| delete.target)
                .collect();
        let mut kept = Vec::new();
        plan.deletes.retain(|delete| {
            let offered = offered.contains(&delete.target);
            if !offered {
                let target = delete.target.display();
                let line = format!("{target}: kept: unticked in what to back up since the plan");
                kept.push((line, false));
            }
            offered
        });
        kept
    }

    /// Makes `snapshot`, which `restored` wrote (a pre-restore safety backup too), this PC's
    /// base of its game when no line of `restored` failed: no write, deletion or check of the
    /// deletions ([`Self::recheck_deletes`]). Its saves are this PC's now, as Back up stages
    /// them; none when the scan does not find the game or its settings skip it, nor when they
    /// cannot be listed, which adds a line. A failed record adds a line and leaves the restore
    /// as it is. Call it while holding the repository's storage lock, taken for the restore, so
    /// no save made since is listed.
    pub(super) fn apply(
        &self,
        storage: &mut Storage,
        restored: &mut Restored,
        snapshot: &SnapshotInfo,
    ) {
        let slug = (snapshot.tags.iter()).find_map(|tag| tag.strip_prefix("game:"));
        let Some(slug) = slug.filter(|_| restored.lines.iter().all(|(_, failed)| !failed)) else {
            return;
        };
        let saves = self.saves(storage, slug).unwrap_or_else(|err| {
            let line = format!("This PC's saves were not listed in its sync state: {err:#}");
            restored.lines.push((line, true));
            None
        });
        let base = Base::new(snapshot.id.clone(), saves);
        let scope = self.config.secret_scope();
        if let Err(err) = sync::record(&self.config_dir, &scope, [(slug.to_owned(), base)]) {
            let line = format!("This PC's sync state was not updated: {:#}", anyhow!(err));
            restored.lines.push((line, true));
        }
    }

    /// Stages the game `game_slug` as Back up would now ([`to_stage`]), scanning only it, and
    /// returns its staged list; `None` when the scan does not find it or its settings skip it.
    /// Fails, without scanning, when the pending changes or the settings cannot be read: Back up
    /// would then use the choices it shows, which this job does not have.
    fn saves(&self, storage: &mut Storage, game_slug: &str) -> anyhow::Result<Option<Hashes>> {
        let choices = self
            .choices
            .get_or_init(|| read_choices(storage, &self.config_dir));
        let (pending, games) = choices.as_ref().map_err(|err| anyhow!(err.clone()))?;
        let environment = (self.environment.clone()).unwrap_or_else(Environment::detect);
        let (_, _, scanned) = to_stage(
            Ok(games.clone()),
            Some(pending),
            &Games::new(),
            (&self.manifest, &self.config, &environment),
            |title| slug(title) == game_slug,
            |_, _| {},
        );
        let Some(game) = scanned.first() else {
            return Ok(None);
        };
        Ok(Some(backup::stage_game(game, &self.stage_root)?))
    }
}

/// This PC's pending changes and the repository's settings, or why they cannot be read.
fn read_choices(storage: &mut Storage, config_dir: &Path) -> Result<(Pending, Games), String> {
    let pending = Pending::load(config_dir)
        .context("the pending choices of what to back up cannot be read")
        .map_err(|err| format!("{err:#}"))?;
    let games = choices::read(storage)
        .context("what to back up cannot be read")
        .map_err(|err| format!("{err:#}"))?;
    Ok((pending, games))
}

/// Each game's selection now: the repository's settings with this PC's pending changes for the
/// repository `scope` replayed, or why they cannot be read.
pub(super) fn selections(
    storage: &mut Storage,
    config_dir: &Path,
    scope: &str,
) -> Result<Games, String> {
    let (pending, games) = read_choices(storage, config_dir)?;
    Ok(choices::replay(games, Some(&pending), scope))
}

/// The selection of the game `title` among `selections`: the default for a game without
/// settings, `None` when they cannot be read.
pub(super) fn selection_of(
    selections: &Result<Games, String>,
    title: &str,
) -> Option<GameSettings> {
    match selections.as_ref().ok()?.get(title) {
        Some(Ok(settings)) => Some(settings.clone()),
        Some(Err(_)) => None,
        None => Some(GameSettings::default()),
    }
}

/// Drops `plan`'s deletions when the Backup tab could not save its latest change to what to
/// back up (`saved`), which the restore cannot read (ADR-0036); returns the failed line saying
/// so, which also keeps the base from being recorded.
pub(super) fn unsaved_deletes(plan: &mut RestorePlan, saved: Saved) -> Option<(String, bool)> {
    if saved != Saved::Failed || plan.deletes.is_empty() {
        return None;
    }
    plan.deletes.clear();
    let line = "No file was deleted: the latest change to what to back up was not saved, so this \
                PC's sync state was not updated.";
    Some((line.to_owned(), true))
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
        let on = deletes_on(state, &preview.fetched.title);
        let mut plan = ticked_plan(
            &preview.plan,
            &preview.tree.keys,
            &preview.keys,
            &preview.selection,
            &preview.deleted(on),
        );
        let unsaved = unsaved_deletes(&mut plan, self.choices_saved);
        let fetched = preview.fetched.clone();
        let stage_root = own_dir(&state.env.cache_dir, "restore-stage");
        let cache_dir = state.env.cache_dir.clone();
        let base_update = BaseUpdate::new(state, config);
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
            let kept = base_update.recheck_deletes(&mut storage, &fetched, &mut plan);
            let report =
                restore::run(&mut storage, &title, &plan, &stage_root, wineserver_running)?;
            let mut restored = Restored::new(title, &plan, report);
            restored.lines.extend(unsaved.into_iter().chain(kept));
            base_update.apply(&mut storage, &mut restored, &fetched.listed.snapshot);
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
