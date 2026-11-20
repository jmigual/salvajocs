//! Restoring several games at once: each ticked game with its default snapshot and default
//! targets, a confirm of what each one writes, then `restore::run` per game, each behind its own
//! safety backup (ADR-0005, ADR-0010). A game that fails does not stop the others.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use eframe::egui::{self, Role, Ui, WidgetInfo};
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::manifest::Manifest;
use salvajocs_core::restore::{self, GameBackups, PrepareError, RestorePlan, WriteState};
use salvajocs_core::scan::Environment;
use salvajocs_core::selection::GameSettings;
use salvajocs_core::storage::SnapshotInfo;

use super::{
    Fetched, Label, Labelled, RestoreTab, Restored, WriteTree, blocking, describe_run_error,
    hover_label, into_line, name_context, nothing_to_restore, own_dir, remove_dir, skipped_ui,
    snapshot_label, ticked_plan, warnings_ui,
};
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::tabs::tree::{Badge, Clicked, Mode};
use crate::theme::{self, icons};

/// A game of the batch ready to restore.
struct Planned {
    fetched: Arc<Fetched>,
    /// The fetched snapshot, as the picker shows it.
    snapshot: String,
    labelled: Labelled,
    /// What the run restores of the game, all ticked at first; never saved (ADR-0025).
    selection: GameSettings,
}

impl Planned {
    /// The game planned as `labelled`, with everything ticked.
    fn new(fetched: Arc<Fetched>, snapshot: String, labelled: Labelled) -> Self {
        Self {
            fetched,
            snapshot,
            labelled,
            selection: GameSettings::default(),
        }
    }

    /// Whether the run restores the game: something is ticked and no ticked write blocks its
    /// safety backup.
    fn runs(&self) -> bool {
        let Labelled {
            plan,
            labels,
            tree,
            keys,
        } = &self.labelled;
        let ticked = tree.ticked(&self.selection);
        (ticked.contains(&true) || keys.any_ticked()) && blocking(plan, labels, &ticked).is_empty()
    }
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

/// What a batch dialog's confirm job hands back.
struct Mapped {
    /// The game's index in the batch.
    index: usize,
    choices: Vec<(SnapshotRoot, Choice)>,
    plan: Result<Labelled, PrepareError>,
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
    /// How many of them are downloaded, counted by the job.
    downloaded: Arc<AtomicUsize>,
    /// How many of them are done, counted by the job.
    done: Arc<AtomicUsize>,
    /// How each game of the last batch ended.
    restored: Option<Vec<GameRestored>>,
}

impl Batch {
    /// Whether the batch's planning, dialog confirm or run job is running.
    pub(super) fn busy(&self) -> bool {
        self.planning.is_some() || self.mapping.is_some() || self.running.is_some()
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

    /// Plans every ticked game in one job, in list order: its default snapshot is read from its
    /// listing, with a dir of its own under the cache dir for the run to download it into, and
    /// planned into its default targets without mapping choices (ADR-0020).
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
            Ok(labelled) => {
                if let Some(config) = &mut state.config {
                    crossbuild::apply_choices(config, &game.title, &choices);
                    state.save_config(ctx);
                }
                let (fetched, snapshot) = (mapping.fetched.clone(), mapping.snapshot.clone());
                game.outcome =
                    Outcome::Planned(Box::new(Planned::new(fetched, snapshot, labelled)));
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
                    Ok(Mapped {
                        index,
                        plan: fetched.labelled(fetched.prepare(&choices)),
                        choices,
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
            Some(DialogAction::Cancel) => self.batch.close(),
            Some(DialogAction::Confirm(_) | DialogAction::UsePrefix) | None => {}
        }
    }

    /// The confirm: per ticked game what it writes and where, or why it will not be restored.
    /// It waits for the mapping dialogs.
    pub(super) fn batch_ui(&mut self, ui: &mut Ui, state: &mut State) {
        if self.batch.waiting().is_some() {
            return;
        }
        let busy = self.busy();
        let Some(games) = &mut self.batch.games else {
            return;
        };
        ui.add_space(16.0);
        ui.heading("Plan");
        let error_color = ui.visuals().error_fg_color;
        let mut planned = 0;
        let mut opened = None;
        for game in games {
            let title = &game.title;
            match &mut game.outcome {
                Outcome::Planned(game) => {
                    opened = planned_ui(ui, title, game, !busy).or(opened);
                    planned += usize::from(game.runs());
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
            self.batch.close();
        }
        if let Some(path) = opened
            && let Err(err) = (state.env.open_folder)(&path)
        {
            let path = path.display();
            state.notices.push(format!("Could not open {path}: {err}"));
        }
    }

    /// Restores the ticked part of each planned game that runs in one job: downloads every
    /// game's snapshot first, so no download waits for the reindex after a safety backup
    /// (ADR-0003, ADR-0020), then runs `restore::run` for each, behind its own safety backup
    /// staged under the cache dir. Then the downloads are removed.
    fn start_batch_run(&mut self, ctx: &egui::Context, state: &State) {
        let Some(games) = self.batch.games.take() else {
            return;
        };
        let planned: Vec<(Arc<Fetched>, RestorePlan)> = (games.into_iter())
            .filter_map(|game| match game.outcome {
                Outcome::Planned(planned) if planned.runs() => {
                    let Planned {
                        fetched,
                        labelled,
                        selection,
                        ..
                    } = *planned;
                    let Labelled {
                        plan, tree, keys, ..
                    } = &labelled;
                    let plan = ticked_plan(plan, &tree.keys, keys, &selection);
                    Some((fetched, plan))
                }
                Outcome::Planned(_)
                | Outcome::NeedsMapping(_)
                | Outcome::Skipped
                | Outcome::Failed(_) => None,
            })
            .collect();
        self.error = None;
        self.restored = None;
        self.batch.total = planned.len();
        self.batch.downloaded = Arc::default();
        self.batch.done = Arc::default();
        let (downloaded, done) = (self.batch.downloaded.clone(), self.batch.done.clone());
        let stage_root = own_dir(&state.env.cache_dir, "restore-stage");
        let root = own_dir(&state.env.cache_dir, "restore-batch");
        let wineserver_running = state.env.wineserver_running;
        let waker = ctx.clone();
        self.batch.running = Some(Job::spawn(ctx, move || {
            if let Some(parent) = root.parent() {
                remove_stale(parent);
            }
            remove_dir(&root).with_context(|| format!("cannot clear {}", root.display()))?;
            let planned: Vec<_> = (planned.into_iter())
                .map(|(fetched, plan)| {
                    // A failure to lock fails this game, not the job.
                    let fetch = lock_storage(&fetched.storage).and_then(|mut storage| {
                        Ok(restore::download(&mut storage, &fetched.listed)?)
                    });
                    downloaded.fetch_add(1, Ordering::Relaxed);
                    waker.request_repaint();
                    (fetched, plan, fetch)
                })
                .collect();
            let restored = (planned.into_iter())
                .map(|(fetched, plan, fetch)| {
                    let title = fetched.title.clone();
                    let fetch = fetch.context("the backup cannot be fetched");
                    let storage = fetch.and_then(|()| lock_storage(&fetched.storage));
                    let report = storage.and_then(|mut storage| {
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
            // Best effort: what is left is cleared when the next batch runs.
            let _ = remove_dir(&root);
            Ok(restored)
        }));
    }

    /// The running batch's progress, or how each game of the last batch ended.
    pub(super) fn batch_restored_ui(&self, ui: &mut Ui) {
        let batch = &self.batch;
        if batch.running.is_some() {
            let downloaded = batch.downloaded.load(Ordering::Relaxed);
            let done = batch.done.load(Ordering::Relaxed);
            let total = count(batch.total, "game");
            if downloaded < batch.total {
                ui.label(format!("Downloading {downloaded} of {total}"));
            } else {
                ui.label(format!("Restored {done} of {total}"));
            }
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

/// Reads `snapshot` of `title`, to be downloaded into `work_dir`, and plans its restore into its
/// default targets, then labels each write as the single game's plan does.
fn plan_game(
    title: String,
    snapshot: &SnapshotInfo,
    work_dir: &Path,
    storage: &SharedStorage,
    manifest: &Manifest,
    config: &AppConfig,
    environment: &Environment,
) -> Outcome {
    let listed = lock_storage(storage)
        .and_then(|mut storage| Ok(restore::read(&mut storage, snapshot, work_dir)?));
    let listed = match listed {
        Ok(listed) => Arc::new(listed),
        Err(err) => return Outcome::Failed(format!("the backup cannot be read: {err:#}")),
    };
    let staged = Some(&listed.staged);
    let defaults = restore::default_targets(&title, manifest, config, environment, staged);
    let info = restore::game_info(&title, manifest, &config.custom_games);
    let fetched = Fetched {
        storage: storage.clone(),
        vars: restore::game_vars(&title, info.entry.as_ref(), config, environment),
        title,
        listed,
        targets: defaults.targets,
        prefix_origin: defaults.origin,
        reason: defaults.reason,
        info,
    };
    match fetched.labelled(fetched.prepare(&[])) {
        Ok(labelled) => Outcome::Planned(Box::new(Planned::new(
            Arc::new(fetched),
            snapshot_label(snapshot),
            labelled,
        ))),
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

/// The badge of a write in `state`, `Err` with why it will fail, and its tooltip; `None` for an
/// unchanged one.
pub(super) fn write_badge(state: Result<WriteState, &str>) -> Option<(Badge, String)> {
    match state {
        Ok(WriteState::New) => Some((Badge::New, "New: not on this PC yet".to_owned())),
        Ok(WriteState::Changed) => {
            Some((Badge::Changed, "Changed: will be overwritten".to_owned()))
        }
        Ok(WriteState::Unchanged) => None,
        Ok(_) => Some((Badge::Unknown, "Could not be compared".to_owned())),
        Err(reason) => Some((Badge::WillFail, format!("Will fail: {reason}"))),
    }
}

/// The icons counting the new, changed, unchanged and not compared writes among `states`, and
/// nothing when there are none: when every write will fail, its failures say so.
pub(super) fn state_chips(ui: &mut Ui, states: &[WriteState]) {
    let of = |wanted| states.iter().filter(|&&state| state == wanted).count();
    let (new, changed, unchanged, unknown) = (
        of(WriteState::New),
        of(WriteState::Changed),
        of(WriteState::Unchanged),
        of(WriteState::Unknown),
    );
    if new + changed + unchanged + unknown == 0 {
        return;
    }
    let chips = [
        (Badge::New.glyph(), Badge::New.color(ui), new),
        (Badge::Changed.glyph(), Badge::Changed.color(ui), changed),
        (icons::EQUALS, ui.visuals().weak_text_color(), unchanged),
        (Badge::Unknown.glyph(), Badge::Unknown.color(ui), unknown),
    ];
    let text: Vec<_> = [
        (new, "new"),
        (changed, "changed"),
        (unchanged, "unchanged"),
        (unknown, "could not be compared"),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, kind)| format!("{count} {kind}"))
    .collect();
    theme::chips(ui, &chips, &text.join(", "));
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

/// One planned game of the confirm: its snapshot with how many ticked files are new, changed or
/// unchanged, its targets, the ticked writes that will fail or block it, the tree of its files
/// with a checkbox on each, the plan's warnings, the registry with a checkbox per key and the
/// skipped items. `enabled` false greys the checkboxes. Returns the folder the user asked to open.
fn planned_ui(ui: &mut Ui, title: &str, planned: &mut Planned, enabled: bool) -> Option<PathBuf> {
    let ticked = planned.labelled.tree.ticked(&planned.selection);
    let Planned {
        fetched,
        snapshot,
        labelled:
            Labelled {
                plan,
                labels,
                tree: WriteTree { tree, .. },
                keys,
            },
        selection,
    } = planned;
    ui.add_space(8.0);
    theme::group(ui, |ui| {
        let compared: Vec<_> = (labels.iter().zip(&ticked))
            .filter_map(|(label, &ticked)| match label {
                Label::State(state) if ticked => Some(*state),
                Label::State(_) | Label::WillFail(_) | Label::Blocks(_) => None,
            })
            .collect();
        theme::row(
            ui,
            |ui| {
                ui.strong(format!("{title}, snapshot {snapshot}"));
            },
            |ui| state_chips(ui, &compared),
        );
        let used = restore::used_targets(plan, &fetched.targets);
        let (into, hover) = into_line(&used, fetched.prefix_origin.as_ref(), fetched.reason);
        hover_label(ui, into, hover);
        let error_color = ui.visuals().error_fg_color;
        for ((write, label), _) in
            (plan.writes.iter().zip(labels.iter()).zip(&ticked)).filter(|(_, ticked)| **ticked)
        {
            if let Label::WillFail(reason) = label {
                let text = format!("{}: will fail: {reason}", write.target.display());
                theme::status(ui, icons::X_CIRCLE, error_color, text);
            }
        }
        let blocking = blocking(plan, labels, &ticked);
        if !blocking.is_empty() {
            let text = "Nothing will be restored: the safety backup cannot read these files:";
            theme::status(ui, icons::X_CIRCLE, error_color, text);
            for path in &blocking {
                ui.weak(path);
            }
            ui.weak("Untick them to restore the rest of this game.");
        } else if nothing_to_restore(plan) {
            ui.weak("Nothing to restore, so this game will not be restored.");
        } else if !ticked.contains(&true) && !keys.any_ticked() {
            ui.weak("Nothing ticked, so this game will not be restored.");
        }
        let badge = |index: usize, _: &str| labels[index].badge().map(|(badge, _)| badge);
        let hint = |index: usize, _: &str, _| {
            (labels[index].badge())
                .map(|(_, hint)| hint)
                .unwrap_or_default()
        };
        let included = |key: &str| selection.path_included(key);
        let mode = Mode::Select {
            included: &included,
            enabled,
        };
        let clicked = ui.push_id(title, |ui| {
            tree.ui_with_rows(ui, mode, &badge, &hint, &mut |_, _, _, _| {})
        });
        warnings_ui(ui, plan);
        if let Some((_, ctx)) = &plan.registry {
            let (name, hover) = name_context(ctx, fetched.prefix_origin.as_ref());
            let text = format!(
                "Registry: {} keys into {name}, replaced",
                keys.ticked_count()
            );
            hover_label(ui, text, hover);
            // Two games may declare the same key.
            ui.push_id((title, "registry"), |ui| keys.ui(ui, enabled));
        }
        skipped_ui(ui, plan);
        match clicked.inner {
            Some(Clicked::Open(path)) => Some(path),
            Some(Clicked::Toggle(key, included)) => {
                selection.set_path(&key, included);
                tree.invalidate();
                None
            }
            None => None,
        }
    })
}
