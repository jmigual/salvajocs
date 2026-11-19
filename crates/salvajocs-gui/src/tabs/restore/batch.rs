//! Restoring several games at once: each ticked game with its default snapshot and default
//! targets, a confirm of what each one writes, then `restore::run` per game, each behind its own
//! safety backup (ADR-0005, ADR-0010). A game that fails does not stop the others.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Role, Ui, WidgetInfo};
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::layout::Context;
use salvajocs_core::manifest::Manifest;
use salvajocs_core::restore::{self, GameBackups, PrepareError, RestorePlan, Targets, WriteState};
use salvajocs_core::scan::Environment;
use salvajocs_core::storage::SnapshotInfo;

use super::mapping::{Dialog, DialogAction};
use super::{
    Fetched, RestoreTab, Restored, describe_context, describe_run_error, own_dir, remove_dir,
    skipped_ui, snapshot_label, warnings_ui,
};
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::theme::{self, icons};

/// A game of the batch ready to restore.
struct Planned {
    fetched: Arc<Fetched>,
    /// The fetched snapshot, as the picker shows it.
    snapshot: String,
    plan: RestorePlan,
    /// What each of `plan.writes` does to its target, or why it will fail.
    states: Vec<Result<WriteState, String>>,
}

/// A game of the batch waiting for its turn in the mapping dialog (ADR-0011).
struct Mapping {
    fetched: Arc<Fetched>,
    /// The fetched snapshot, as the picker shows it.
    snapshot: String,
    dialog: Dialog,
}

/// What planning made of one ticked game.
enum Outcome {
    Planned(Box<Planned>),
    /// A folder of the other OS's build has no target yet.
    NeedsMapping(Box<Mapping>),
    /// The user left the game out in the mapping dialog.
    Skipped,
    /// Why nothing will be restored.
    Failed(String),
}

/// The state of each write of a plan, or why the game would write nothing; see [`compare`].
type Compared = Result<Vec<Result<WriteState, String>>, String>;

/// What a batch dialog's confirm job hands back.
struct Mapped {
    /// The game's index in the batch.
    index: usize,
    choices: Vec<(SnapshotRoot, Choice)>,
    plan: Result<(RestorePlan, Compared), PrepareError>,
}

/// One ticked game of a batch.
struct BatchGame {
    title: String,
    outcome: Outcome,
}

/// How one game of a batch run ended.
enum GameRestored {
    Restored(Restored),
    /// Nothing was written for the game.
    Failed {
        title: String,
        error: String,
    },
}

/// The batch restore of the Restore tab.
#[derive(Default)]
pub(super) struct Batch {
    /// The planning job, with the [`RestoreTab::generation`] it started in.
    planning: Option<(u64, Job<Vec<BatchGame>>)>,
    /// The ticked games in list order, while the dialogs and the confirm show.
    games: Option<Vec<BatchGame>>,
    /// A dialog's confirm job, with the generation it started in.
    mapping: Option<(u64, Job<Mapped>)>,
    running: Option<Job<Vec<GameRestored>>>,
    /// How many games the running batch restores.
    total: usize,
    /// How many of them are done, counted by the job.
    done: Arc<AtomicUsize>,
    /// How each game of the last batch ended.
    restored: Option<Vec<GameRestored>>,
    /// The job removing the fetched snapshots of a cancelled batch.
    cleaning: Option<Job<()>>,
}

impl Batch {
    /// Whether the batch's planning, dialog confirm, run or cleanup job is running.
    pub(super) fn busy(&self) -> bool {
        self.planning.is_some()
            || self.mapping.is_some()
            || self.running.is_some()
            || self.cleaning.is_some()
    }

    /// Whether the run, which writes the games' files, is running.
    pub(super) fn writing(&self) -> bool {
        self.running.is_some()
    }

    /// Whether a mapping dialog or the confirm shows, before anything is written.
    pub(super) fn open(&self) -> bool {
        self.games.is_some()
    }

    /// The first game still waiting for the mapping dialog, with its index.
    fn waiting(&mut self) -> Option<(usize, &str, &mut Mapping)> {
        let games = self.games.as_mut()?.iter_mut().enumerate();
        games
            .filter_map(|(index, game)| match &mut game.outcome {
                Outcome::NeedsMapping(mapping) => {
                    Some((index, game.title.as_str(), &mut **mapping))
                }
                _ => None,
            })
            .next()
    }

    /// Whether there is neither a confirm nor a result to show.
    pub(super) fn untouched(&self) -> bool {
        !self.open() && self.restored.is_none()
    }

    /// Closes the confirm. A running job is kept; the generation drops a planning job's result.
    pub(super) fn close(&mut self) {
        self.games = None;
    }

    /// Drops the result of the last batch, as another restore starts.
    pub(super) fn clear_result(&mut self) {
        self.restored = None;
    }
}

/// The checkbox that ticks `game` for a batch. A game with only safety backups has no default
/// snapshot (ADR-0010), so it cannot be ticked.
pub(super) fn include_ui(ui: &mut Ui, checked: &mut BTreeSet<String>, game: &GameBackups) {
    let enabled = game.default.is_some();
    let mut included = checked.contains(&game.slug);
    let response = ui.add_enabled(enabled, egui::Checkbox::without_text(&mut included));
    // Not the bare title, which names the game's own button.
    let label = format!("Include {}", game.title);
    response
        .widget_info(|| WidgetInfo::selected(Role::CheckBox, response.enabled(), included, &label));
    if response.changed() {
        if included {
            checked.insert(game.slug.clone());
        } else {
            checked.remove(&game.slug);
        }
    }
}

impl RestoreTab {
    /// The listed games ticked for a batch.
    fn ticked(&self) -> impl Iterator<Item = &GameBackups> {
        (self.games.iter()).filter(|game| self.checked.contains(&game.slug))
    }

    /// Ticks every game that can be ticked, or none.
    pub(super) fn select_all_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let quiet = |text| egui::Button::new(text).frame_when_inactive(false);
            if ui.add(quiet("Select all")).clicked() {
                let defaults = self.games.iter().filter(|game| game.default.is_some());
                self.checked.extend(defaults.map(|game| game.slug.clone()));
            }
            if ui.add(quiet("Deselect all")).clicked() {
                self.checked.clear();
            }
        });
    }

    /// The button that plans the ticked games; hidden while the confirm shows its own.
    pub(super) fn batch_button_ui(&mut self, ui: &mut Ui, state: &State) {
        if self.batch.open() {
            return;
        }
        let ticked = self.ticked().count();
        let label = format!("Restore {}", count(ticked, "game"));
        let enabled = ticked > 0 && state.session.is_some();
        ui.add_space(8.0);
        let glyph = Some(icons::CLOCK_COUNTER_CLOCKWISE);
        if theme::primary(ui, enabled, glyph, &label).clicked() {
            self.start_batch(ui.ctx(), state);
        }
    }

    /// Plans every ticked game in one job, in list order: its default snapshot is fetched into a
    /// dir of its own under the cache dir, kept until the run, and planned into its default
    /// targets without mapping choices.
    fn start_batch(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let games: Vec<(String, Option<SnapshotInfo>)> = (self.ticked())
            .map(|game| {
                let snapshot = game.default.and_then(|index| game.snapshots.get(index));
                (game.title.clone(), snapshot.cloned())
            })
            .collect();
        self.forget_plan();
        self.selected = None;
        self.pick = None;
        self.error = None;
        self.removed = None;
        self.restored = None;
        self.batch.restored = None;
        let (storage, manifest, config) = (
            session.storage.clone(),
            state.manifest_or_empty(),
            config.clone(),
        );
        let environment = state.env.environment.clone();
        let root = own_dir(&state.env.cache_dir, "restore-batch");
        let job = Job::spawn(ctx, move || {
            if let Some(parent) = root.parent() {
                remove_stale(parent);
            }
            remove_dir(&root).with_context(|| format!("cannot clear {}", root.display()))?;
            let environment = environment.unwrap_or_else(Environment::detect);
            let games = (games.into_iter().enumerate())
                .map(|(index, (title, snapshot))| {
                    let outcome = match snapshot {
                        // By index: a slug is snapshot data, so it never names a path.
                        Some(snapshot) => plan_game(
                            title.clone(),
                            &snapshot,
                            &root.join(index.to_string()),
                            &storage,
                            &manifest,
                            &config,
                            &environment,
                        ),
                        None => Outcome::Failed("it has only pre-restore backups".to_owned()),
                    };
                    BatchGame { title, outcome }
                })
                .collect();
            Ok(games)
        });
        self.batch.planning = Some((self.generation, job));
    }

    /// Takes the results of the batch's jobs. A finished run lists the backups again, which gain
    /// the safety backups.
    pub(super) fn poll_batch(&mut self, ctx: &egui::Context, state: &mut State) {
        if let Some((generation, job)) = &mut self.batch.planning
            && let Some(result) = job.take_result()
        {
            let current = *generation == self.generation;
            self.batch.planning = None;
            match result {
                // A batch planned in a repository Settings replaced is not shown.
                _ if !current => {}
                Ok(games) => self.batch.games = Some(games),
                Err(err) => self.error = Some(format!("The restore cannot be planned: {err:#}")),
            }
        }
        if let Some((generation, job)) = &mut self.batch.mapping
            && let Some(result) = job.take_result()
        {
            let current = *generation == self.generation;
            self.batch.mapping = None;
            match result {
                _ if !current => {}
                Ok(mapped) => self.mapped(mapped, ctx, state),
                Err(err) => self.error = Some(format!("The restore cannot be planned: {err:#}")),
            }
        }
        if let Some(job) = &mut self.batch.running
            && let Some(result) = job.take_result()
        {
            self.batch.running = None;
            match result {
                Ok(restored) => self.batch.restored = Some(restored),
                Err(err) => self.error = Some(format!("{err:#}")),
            }
            self.checked.clear();
            self.start_listing(ctx, state);
        }
        // Best effort: the next batch clears the dir again.
        if let Some(job) = &mut self.batch.cleaning
            && job.take_result().is_some()
        {
            self.batch.cleaning = None;
        }
    }

    /// Closes the batch without writing anything and removes its fetched snapshots in a job.
    pub(super) fn cancel_batch(&mut self, ctx: &egui::Context, state: &State) {
        self.batch.close();
        let root = own_dir(&state.env.cache_dir, "restore-batch");
        self.batch.cleaning = Some(Job::spawn(ctx, move || Ok(remove_dir(&root)?)));
    }

    /// A dialog's plan for the game it was confirmed for: shown on the confirm, with the choices
    /// saved on the game's custom entry as in the single-game view (ADR-0011).
    fn mapped(&mut self, mapped: Mapped, ctx: &egui::Context, state: &mut State) {
        let Mapped {
            index,
            choices,
            plan,
        } = mapped;
        let Some(game) = (self.batch.games.as_mut()).and_then(|games| games.get_mut(index)) else {
            return;
        };
        let Outcome::NeedsMapping(mapping) = &mut game.outcome else {
            return;
        };
        match plan {
            Ok((plan, states)) => {
                if let Some(config) = &mut state.config {
                    crossbuild::apply_choices(config, &game.title, &choices);
                    state.save_config(ctx);
                }
                game.outcome = match states {
                    Ok(states) => Outcome::Planned(Box::new(Planned {
                        fetched: mapping.fetched.clone(),
                        snapshot: mapping.snapshot.clone(),
                        plan,
                        states,
                    })),
                    Err(reason) => Outcome::Failed(reason),
                };
            }
            Err(PrepareError::Unclassifiable { root, template }) => {
                mapping.dialog.reject(&root, &template);
            }
            Err(err) => {
                game.outcome = Outcome::Failed(format!("the restore cannot be planned: {err}"));
            }
        }
    }

    /// The mapping dialog of the first game that needs one, one game after another. Cancel drops
    /// the whole batch.
    pub(super) fn batch_mapping_ui(&mut self, ctx: &egui::Context, state: &State) {
        let (busy, error, generation) = (self.busy(), self.error.clone(), self.generation);
        let Some((index, title, mapping)) = self.batch.waiting() else {
            return;
        };
        let action = theme::modal(ctx, "restore_batch_mapping", 720.0, |ui| {
            (mapping.dialog).ui(ui, title, busy, false, true, error.as_deref())
        });
        match action {
            Some(DialogAction::Confirm(choices)) if state.session.is_some() => {
                let fetched = mapping.fetched.clone();
                let job = Job::spawn(ctx, move || {
                    let plan = fetched.prepare(&choices).map(|plan| {
                        let states = compare(&plan);
                        (plan, states)
                    });
                    Ok(Mapped {
                        index,
                        choices,
                        plan,
                    })
                });
                self.batch.mapping = Some((generation, job));
            }
            Some(DialogAction::SkipGame) => {
                if let Some(game) =
                    (self.batch.games.as_mut()).and_then(|games| games.get_mut(index))
                {
                    game.outcome = Outcome::Skipped;
                }
            }
            Some(DialogAction::Cancel) => self.cancel_batch(ctx, state),
            Some(DialogAction::Confirm(_) | DialogAction::UsePrefix) | None => {}
        }
    }

    /// The confirm: per ticked game what it writes and where, or why it will not be restored.
    /// It waits for the mapping dialogs.
    pub(super) fn batch_ui(&mut self, ui: &mut Ui, state: &State) {
        if self.batch.waiting().is_some() {
            return;
        }
        let Some(games) = &self.batch.games else {
            return;
        };
        ui.add_space(16.0);
        ui.heading("Plan");
        let error_color = ui.visuals().error_fg_color;
        let mut planned = 0;
        for game in games {
            let title = &game.title;
            match &game.outcome {
                Outcome::Planned(game) => {
                    planned += 1;
                    planned_ui(ui, title, game);
                }
                Outcome::Skipped => {
                    ui.weak(format!("{title}: skipped in the folder mapping dialog"));
                }
                // The dialogs come first, so none is left here.
                Outcome::NeedsMapping(_) => {}
                Outcome::Failed(reason) => theme::status(
                    ui,
                    icons::X_CIRCLE,
                    error_color,
                    format!("{title}: will not be restored: {reason}"),
                ),
            }
        }
        let (mut cancel, mut run) = (false, false);
        let label = format!("Restore {}", count(planned, "game"));
        let enabled = planned > 0 && state.session.is_some();
        theme::row_end(ui, |ui| {
            cancel |= ui.button("Cancel").clicked();
            let glyph = Some(icons::CLOCK_COUNTER_CLOCKWISE);
            run |= theme::primary(ui, enabled, glyph, &label).clicked();
        });
        if run {
            self.start_batch_run(ui.ctx(), state);
        } else if cancel {
            self.cancel_batch(ui.ctx(), state);
        }
    }

    /// Restores the planned games one after another in one job, each with `restore::run` and
    /// so behind its own safety backup, staged under the cache dir. Then the fetched snapshots
    /// are removed.
    fn start_batch_run(&mut self, ctx: &egui::Context, state: &State) {
        let Some(games) = self.batch.games.take() else {
            return;
        };
        let planned: Vec<(Arc<Fetched>, RestorePlan)> = (games.into_iter())
            .filter_map(|game| match game.outcome {
                Outcome::Planned(planned) => Some((planned.fetched, planned.plan)),
                Outcome::NeedsMapping(_) | Outcome::Skipped | Outcome::Failed(_) => None,
            })
            .collect();
        self.error = None;
        self.restored = None;
        self.batch.total = planned.len();
        self.batch.done = Arc::default();
        let done = self.batch.done.clone();
        let stage_root = own_dir(&state.env.cache_dir, "restore-stage");
        let root = own_dir(&state.env.cache_dir, "restore-batch");
        let wineserver_running = state.env.wineserver_running;
        let waker = ctx.clone();
        self.batch.running = Some(Job::spawn(ctx, move || {
            let restored = (planned.into_iter())
                .map(|(fetched, plan)| {
                    let title = fetched.title.clone();
                    // A failure to lock fails this game, not the job.
                    let report = lock_storage(&fetched.storage).and_then(|mut storage| {
                        let running = wineserver_running();
                        Ok(restore::run(
                            &mut storage,
                            &title,
                            &plan,
                            &stage_root,
                            running,
                        )?)
                    });
                    done.fetch_add(1, Ordering::Relaxed);
                    waker.request_repaint();
                    match report {
                        Ok(report) => GameRestored::Restored(Restored::new(title, &plan, report)),
                        Err(err) => GameRestored::Failed {
                            error: describe_run_error(&err),
                            title,
                        },
                    }
                })
                .collect();
            // Best effort: what is left is cleared when the next batch is planned.
            let _ = remove_dir(&root);
            Ok(restored)
        }));
    }

    /// The running batch's progress, or how each game of the last batch ended.
    pub(super) fn batch_restored_ui(&self, ui: &mut Ui) {
        let batch = &self.batch;
        if batch.running.is_some() {
            let done = batch.done.load(Ordering::Relaxed);
            ui.label(format!("Restored {done} of {}", count(batch.total, "game")));
            return;
        }
        let Some(games) = &batch.restored else {
            return;
        };
        let restored = (games.iter())
            .filter(|game| matches!(game, GameRestored::Restored(_)))
            .count();
        ui.add_space(16.0);
        ui.heading(format!(
            "Restored {restored} of {}",
            count(games.len(), "game")
        ));
        let error_color = ui.visuals().error_fg_color;
        for game in games {
            ui.add_space(8.0);
            match game {
                GameRestored::Restored(restored) => {
                    ui.strong(format!("{}:", restored.title));
                    restored.lines_ui(ui);
                }
                GameRestored::Failed { title, error } => {
                    ui.strong(format!("{title}:"));
                    theme::status(ui, icons::X_CIRCLE, error_color, error);
                }
            }
        }
    }
}

/// Fetches `snapshot` of `title` into `work_dir` and plans its restore into its default targets,
/// then compares each write with its target.
fn plan_game(
    title: String,
    snapshot: &SnapshotInfo,
    work_dir: &Path,
    storage: &SharedStorage,
    manifest: &Manifest,
    config: &AppConfig,
    environment: &Environment,
) -> Outcome {
    let targets = restore::default_targets(&title, manifest, config, environment);
    let staged = lock_storage(storage)
        .and_then(|mut storage| Ok(restore::fetch(&mut storage, snapshot, work_dir)?));
    let staged = match staged {
        Ok(staged) => staged,
        Err(err) => return Outcome::Failed(format!("the backup cannot be fetched: {err:#}")),
    };
    let info = restore::game_info(&title, manifest, &config.custom_games);
    let fetched = Fetched {
        storage: storage.clone(),
        vars: restore::game_vars(&title, info.entry.as_ref(), config, environment),
        title,
        staged,
        targets,
        info,
    };
    match fetched.prepare(&[]) {
        Ok(plan) => match compare(&plan) {
            Ok(states) => Outcome::Planned(Box::new(Planned {
                states,
                snapshot: snapshot_label(snapshot),
                fetched: Arc::new(fetched),
                plan,
            })),
            Err(reason) => Outcome::Failed(reason),
        },
        Err(PrepareError::NeedsMapping(entries)) => Outcome::NeedsMapping(Box::new(Mapping {
            fetched: Arc::new(fetched),
            snapshot: snapshot_label(snapshot),
            dialog: Dialog::new(entries),
        })),
        Err(PrepareError::Unclassifiable { template, .. }) => Outcome::Failed(format!(
            "the saved target {template} is not a placeholder template of this OS"
        )),
        Err(err) => Outcome::Failed(format!("the restore cannot be planned: {err}")),
    }
}

/// [`restore::compare_writes`] with each error as the text the confirm shows; `Err` when the
/// restore of the game would write nothing.
fn compare(plan: &RestorePlan) -> Result<Vec<Result<WriteState, String>>, String> {
    let states = restore::compare_writes(plan).map_err(|err| format!("{:#}", anyhow!(err)))?;
    Ok((states.into_iter())
        .map(|state| state.map_err(|err| format!("{:#}", anyhow!(err))))
        .collect())
}

/// Removes the batch dirs under `parent` that earlier sessions left: named by another process id
/// and untouched for a day, so the batch of another running salvajocs is kept.
fn remove_stale(parent: &Path) {
    const STALE: Duration = Duration::from_secs(24 * 60 * 60);
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let ours = std::process::id().to_string();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let other = (name.to_str()).is_some_and(|name| name != ours && name.parse::<u32>().is_ok());
        let old = (entry.metadata().and_then(|meta| meta.modified()).ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if other && old {
            // Best effort: a dir that stays is tried again by the next batch.
            let _ = remove_dir(&entry.path());
        }
    }
}

/// One planned game of the confirm: its snapshot and targets, how many files are new, changed
/// or unchanged, the writes that will fail, the plan's warnings, registry and skipped items.
fn planned_ui(ui: &mut Ui, title: &str, planned: &Planned) {
    let Planned {
        fetched,
        snapshot,
        plan,
        states,
    } = planned;
    ui.add_space(8.0);
    theme::group(ui, |ui| {
        ui.strong(format!("{title}, snapshot {snapshot}"));
        ui.label(format!("Into {}", describe_targets(&fetched.targets)));
        let of = |wanted| {
            (states.iter())
                .filter(|state| state.as_ref() == Ok(&wanted))
                .count()
        };
        ui.label(format!(
            "{} new, {} changed, {} unchanged",
            of(WriteState::New),
            of(WriteState::Changed),
            of(WriteState::Unchanged)
        ));
        let error_color = ui.visuals().error_fg_color;
        for (write, state) in plan.writes.iter().zip(states) {
            if let Err(reason) = state {
                let text = format!("{}: will fail: {reason}", write.target.display());
                theme::status(ui, icons::X_CIRCLE, error_color, text);
            }
        }
        warnings_ui(ui, plan);
        if let Some((export, ctx)) = &plan.registry {
            ui.label(format!(
                "Registry: {} keys into {}, replaced",
                export.keys.len(),
                describe_context(ctx)
            ));
        }
        skipped_ui(ui, plan);
    });
}

/// Where a game's files go, as the confirm names it.
fn describe_targets(targets: &Targets) -> String {
    let linux = targets.linux.as_ref().map(|_| "native Linux".to_owned());
    let win = targets.win.as_ref().map(|ctx| match ctx {
        Context::WindowsNative(_) => "this Windows user's folders".to_owned(),
        ctx => describe_context(ctx),
    });
    let parts: Vec<String> = linux.into_iter().chain(win).collect();
    if parts.is_empty() {
        "no folder on this machine".to_owned()
    } else {
        parts.join(" and ")
    }
}
