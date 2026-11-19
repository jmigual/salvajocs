//! The Backup tab: scan this machine for games, choose which to back up and see what each
//! backup and the cleanup after it did (ADR-0004, ADR-0010, ADR-0015).

use std::borrow::Cow;
use std::error::Error;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use eframe::egui::emath::Rot2;
use eframe::egui::{
    self, Checkbox, Label, Rect, RichText, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo,
};
use salvajocs_core::backup::{self, GameBackup};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::retention::{self, Cleanup, Retention};
use salvajocs_core::scan::{self, Environment, GameScan, ProblemKind, ScanProblem, ScanReport};
use salvajocs_core::selection::{self, Change, GameSettings, Pending};

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::tree::{Clicked, FileTree};
use crate::tabs::{count, format_size};
use crate::theme::{self, icons};
use choices::{AfterBackup, Choices, Games};

mod choices;

/// What a backup did for one game.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// A new snapshot was stored.
    Saved,
    /// Nothing changed since the last snapshot.
    Unchanged,
    /// The game could not be backed up; holds why.
    Failed(String),
}

/// One game's line in the results.
#[derive(Debug, PartialEq, Eq)]
struct GameResult {
    title: String,
    outcome: Outcome,
}

/// `error` and its sources, joined.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text = format!("{text}: {next}");
        source = next.source();
    }
    text
}

fn summarize(backup: &GameBackup) -> GameResult {
    let outcome = match &backup.result {
        Ok(Some(_)) => Outcome::Saved,
        Ok(None) => Outcome::Unchanged,
        Err(err) => Outcome::Failed(chain(err)),
    };
    GameResult {
        title: backup.title.clone(),
        outcome,
    }
}

/// What the scan found for a game: the count and size of the files backed up, given as
/// `included`, how many are excluded, whether registry keys are backed up, and unreadable paths.
fn summary(game: &GameScan, (files, size): (usize, u64), settings: &GameSettings) -> String {
    let mut text = format!("{}, {}", count(files, "file"), format_size(size));
    let excluded = game.files.len() - files;
    if excluded > 0 {
        text += &format!(", {excluded} excluded");
    }
    let keys = &game.registry_keys;
    if game.registry_source.is_some() && keys.iter().any(|key| settings.registry_included(key)) {
        text += ", plus registry keys";
    }
    if !game.unreadable.is_empty() {
        text += &format!(
            ", {} (the files may be incomplete)",
            count(game.unreadable.len(), "unreadable path")
        );
    }
    text
}

fn describe_problem(problem: &ScanProblem) -> String {
    let path = problem.path.display();
    match &problem.kind {
        ProblemKind::AmbiguousPrefix(users) => format!(
            "{path}: the Wine prefix has several users ({}) and none is steamuser",
            users.join(", ")
        ),
        ProblemKind::NoUser => format!("{path}: the Wine prefix has no user yet"),
        ProblemKind::NotAPrefix => format!("{path}: not a Wine prefix"),
        ProblemKind::Unreadable(error) => format!("{path}: cannot be read ({error})"),
    }
}

/// How far the running backup is, shared with its job.
#[derive(Default)]
struct Progress {
    /// The games the backup goes through, known once its scan is done.
    total: usize,
    /// The outcome of each game finished so far.
    done: Vec<GameResult>,
    /// Every game is done and old backups are being removed.
    cleaning: bool,
}

/// What saving the selection before a backup ran into.
#[derive(Default)]
struct SaveProblems {
    /// The games whose changes could not be saved, with why; their changes stay pending.
    failed: Vec<String>,
    /// Problems that lost no change.
    warnings: Vec<String>,
}

/// What the cleanup after a backup did.
struct Cleaned {
    /// Why the retention policy could not be read, so nothing was forgotten.
    policy_error: Option<String>,
    /// What the cleanup did, up to `error` if it stopped.
    done: Cleanup,
    /// Why the cleanup stopped, if it did.
    error: Option<String>,
}

/// What a finished backup job hands back.
struct Backed {
    /// The scan the backup went by, which replaces the shown list.
    report: ScanReport,
    /// Why the run stopped early, if it did.
    error: Option<String>,
    /// What saving the selection ran into.
    saving: SaveProblems,
    /// What the backup left of the selection.
    after: AfterBackup,
    /// The cleanup after the backup, `None` when the run stopped early.
    cleanup: Option<Cleaned>,
}

/// The state of the Backup tab.
#[derive(Default)]
pub struct BackupTab {
    scanning: Option<Job<ScanReport>>,
    report: Option<ScanReport>,
    scan_error: Option<String>,
    /// One tree per game of `report`, in its order.
    trees: Vec<FileTree>,
    /// The `Wine prefix: …` lines of each game of `report`, in its order.
    prefixes: Vec<Vec<String>>,
    /// What to back up of each game.
    choices: Choices,
    running: Option<Job<Backed>>,
    progress: Arc<Mutex<Progress>>,
    /// The games the running scan, or the backup's, has checked and its total; 0 before it starts.
    checked: Arc<Mutex<(usize, usize)>>,
    run_error: Option<String>,
    saving: SaveProblems,
    cleanup: Option<Cleaned>,
    /// A cleanup forgot snapshots since [`BackupTab::take_removed`] last ran.
    removed: bool,
    /// Back up was clicked; the backup starts once the pending file is written.
    backup_requested: bool,
}

impl BackupTab {
    /// Draws the tab. The scan and the backup run in jobs; `state` only gains notices.
    /// `manifest_loading` is whether the manifest job is still running.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        self.poll(ui.ctx(), state);
        // Before the page, so the bar takes its height from the bottom first. It waits for a scan
        // report, so the empty state has one call to action.
        if state.config.is_some() && self.report.is_some() {
            self.save_bar_ui(ui, state, manifest_loading);
        }
        theme::page(ui, |ui| {
            if state.config.is_none() {
                ui.label("Choose a repository first.");
                return;
            }
            if self.report.is_some() {
                self.games_ui(ui, state, manifest_loading);
            } else {
                self.empty_ui(ui, state, manifest_loading);
            }
            if let Some(error) = &self.scan_error {
                let color = ui.visuals().error_fg_color;
                theme::status(
                    ui,
                    icons::X_CIRCLE,
                    color,
                    format!("The scan failed: {error}"),
                );
            }
            self.results_ui(ui);
        });
    }

    /// Collects the tab's finished jobs and starts what waits on them. The app calls it every
    /// frame, whichever tab is in front, so a pending file write or a load never stalls.
    pub(crate) fn poll(&mut self, ctx: &egui::Context, state: &State) {
        if let Some(job) = &mut self.scanning
            && let Some(result) = job.take_result()
        {
            self.scanning = None;
            match result {
                Ok(report) => {
                    self.adopt(report);
                    self.scan_error = None;
                }
                Err(err) => self.scan_error = Some(format!("{err:#}")),
            }
        }
        if let Some(job) = &mut self.running
            && let Some(result) = job.take_result()
        {
            self.running = None;
            match result {
                Ok(Backed {
                    report,
                    error,
                    saving,
                    after,
                    cleanup,
                }) => {
                    self.adopt(report);
                    self.run_error = error;
                    self.saving = saving;
                    self.removed |= cleanup.as_ref().is_some_and(|c| c.done.forgotten > 0);
                    self.cleanup = cleanup;
                    if self.choices.backed_up(after) {
                        self.trees.iter_mut().for_each(FileTree::invalidate);
                    }
                }
                Err(err) => self.run_error = Some(format!("{err:#}")),
            }
        }
        if self.choices.poll(ctx, state) {
            self.trees.iter_mut().for_each(FileTree::invalidate);
        }
        self.start_requested(ctx, state);
    }

    /// Whether a finished cleanup forgot snapshots since the last call, so a loaded list of
    /// backups may name one that is gone. A flag rather than `poll`'s result, since `ui` polls
    /// too and would swallow it.
    pub(crate) fn take_removed(&mut self) -> bool {
        std::mem::take(&mut self.removed)
    }

    /// Shows `report`, with a file tree per game.
    fn adopt(&mut self, report: ScanReport) {
        self.trees = report
            .games
            .iter()
            .map(|game| {
                let keys = game.files.iter();
                FileTree::new(
                    keys.map(|file| (selection::path_key(file), file.size, file.real.clone())),
                )
            })
            .collect();
        self.prefixes = report
            .games
            .iter()
            .map(|game| {
                let line = |prefix: &PathBuf| format!("Wine prefix: {}", prefix.display());
                game.prefixes.iter().map(line).collect()
            })
            .collect();
        self.report = Some(report);
    }

    /// Whether a scan, backup or other job of this tab is running or waits to.
    pub(crate) fn busy(&self) -> bool {
        self.scanning.is_some()
            || self.running.is_some()
            || self.backup_requested
            || self.choices.busy()
    }

    /// Whether a backup runs, which closing the app would cut short.
    pub(crate) fn writing(&self) -> bool {
        self.running.is_some()
    }

    /// Finishes the pending file writes; for when the app closes.
    pub(crate) fn flush(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        self.choices.flush(config_dir)
    }

    /// How many games of the report a backup would store: those its settings leave something
    /// of, as [`selection::apply`] decides, and those whose settings cannot be read. Counts
    /// files from each tree's cached totals.
    fn chosen(&mut self) -> usize {
        let Some(report) = &self.report else {
            return 0;
        };
        let choices = &self.choices;
        let games = report.games.iter().zip(&mut self.trees);
        games
            .map(|(game, tree)| match choices.game(&game.title) {
                Ok(settings) => {
                    let (files, _) = tree.included(&|key| settings.path_included(key));
                    let keys = &game.registry_keys;
                    !settings.skip
                        && (files > 0
                            || game.registry_source.is_some()
                                && keys.iter().any(|key| settings.registry_included(key)))
                }
                Err(_) => true,
            })
            .filter(|&chosen| chosen)
            .count()
    }

    /// Before the first scan: what the tab is for and the scan as its one action.
    fn empty_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            let muted = ui.visuals().weak_text_color();
            theme::mark(ui, icons::GAME_CONTROLLER, 40.0, muted);
            ui.heading("Find your saves");
            ui.weak(
                "Scan this machine for the games in the ludusavi manifest and your custom games.",
            );
            ui.add_space(8.0);
            self.scan_ui(ui, state, manifest_loading, true);
            self.scan_notes_ui(ui, state, manifest_loading);
        });
    }

    /// The scan button: `primary` in the empty state, the one framed button of the list header
    /// otherwise.
    fn scan_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool, primary: bool) {
        let enabled = !manifest_loading && self.scanning.is_none() && self.running.is_none();
        let glyph = icons::MAGNIFYING_GLASS;
        let button = if primary {
            theme::primary(ui, enabled, Some(glyph), "Scan for games")
        } else {
            let button = egui::Button::new((theme::icon(glyph), "Scan for games"));
            ui.add_enabled(enabled, button)
        };
        if let (true, Some(config)) = (button.clicked(), &state.config) {
            let (manifest, config) = (state.manifest_or_empty(), config.clone());
            let environment = state.env.environment.clone();
            self.scan_error = None;
            self.checked = Arc::default();
            let progress = scan_progress(&self.checked, ui.ctx());
            self.scanning = Some(Job::spawn(ui.ctx(), move || {
                let environment = environment.unwrap_or_else(Environment::detect);
                Ok(scan::scan(&manifest, &config, &environment, progress))
            }));
        }
    }

    /// What limits the scan, and whether one runs.
    fn scan_notes_ui(&self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        if manifest_loading {
            ui.weak("The game manifest is loading.");
        } else if state.manifest.is_none() {
            ui.weak("The game manifest is not available; only custom games are scanned.");
        }
        if self.scanning.is_some() {
            let (done, total) = *lock(&self.checked);
            if total == 0 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning");
                });
            } else {
                let text = format!("Checked {done} of {}", count(total, "game"));
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(text));
            }
        }
    }

    /// The scan report: its problems, a header with the count and the selection buttons, and the
    /// games as a boxed list.
    fn games_ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        let Some(report) = &self.report else {
            return;
        };
        let games = report.games.len();
        let editable = self.choices.editable() && self.running.is_none();
        let mut select = None;
        theme::row(
            ui,
            |ui| {
                ui.label(format!("{} found", count(games, "game")));
            },
            |ui| {
                // Frameless until hovered, so the scan reads as the header's main action.
                let quiet = |text| egui::Button::new(text).frame_when_inactive(false);
                if ui.add_enabled(editable, quiet("Select all")).clicked() {
                    select = Some(true);
                }
                if ui.add_enabled(editable, quiet("Deselect all")).clicked() {
                    select = Some(false);
                }
                self.scan_ui(ui, state, manifest_loading, false);
            },
        );
        self.scan_notes_ui(ui, state, manifest_loading);
        self.choices.status_ui(ui, state);
        let Some(report) = &self.report else {
            return;
        };
        // By index into `report.games`, applied after drawing, like the folder to open.
        let mut changes = Vec::new();
        let mut opened = None;
        if let Some(select) = select {
            for (index, game) in report.games.iter().enumerate() {
                if self
                    .choices
                    .game(&game.title)
                    .is_ok_and(|s| s.skip == select)
                {
                    changes.push((index, Change::Skip(!select)));
                }
            }
        }
        if !report.problems.is_empty() {
            let warn = ui.visuals().warn_fg_color;
            theme::status(
                ui,
                icons::WARNING,
                warn,
                format!("Scan problems ({})", report.problems.len()),
            );
            ui.indent("scan_problems", |ui| {
                for problem in &report.problems {
                    ui.colored_label(warn, describe_problem(problem));
                }
            });
        }
        if report.games.is_empty() {
            ui.weak(
                "No saves found. Add the folders where your games live in Settings, or add a \
                 custom game.",
            );
            return;
        }
        let (choices, trees, prefixes) = (&self.choices, &mut self.trees, &self.prefixes);
        theme::group(ui, |ui| {
            // A game's title and its summary read as one row.
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 24.0;
            let games = report.games.iter().zip(trees).zip(prefixes);
            for (index, ((game, tree), prefixes)) in games.enumerate() {
                if index > 0 {
                    theme::row_separator(ui);
                }
                let (settings, error) = match choices.game(&game.title) {
                    Ok(settings) => (settings, None),
                    Err(error) => (Cow::Owned(GameSettings::default()), Some(error)),
                };
                let enabled = editable && error.is_none();
                let mut checked = !settings.skip;
                let checkbox = Checkbox::new(&mut checked, &game.title);
                if ui.add_enabled(enabled, checkbox).changed() {
                    changes.push((index, Change::Skip(!checked)));
                }
                // Under the title, without the indent line: the row already groups them.
                ui.scope(|ui| {
                    ui.visuals_mut().indent_has_left_vline = false;
                    ui.indent(&game.title, |ui| {
                        if let Some(error) = error {
                            let color = ui.visuals().error_fg_color;
                            theme::status(
                                ui,
                                icons::X_CIRCLE,
                                color,
                                format!("What to back up cannot be read, so all of it is: {error}"),
                            );
                        }
                        for prefix in prefixes {
                            ui.weak(prefix);
                        }
                        let change = files_ui(ui, game, tree, &settings, enabled, &mut opened);
                        if let Some(change) = change {
                            changes.push((index, change));
                        }
                    });
                });
            }
        });
        for (index, change) in changes {
            self.choices
                .change(ui.ctx(), state, &report.games[index].title, change);
            self.trees[index].invalidate();
        }
        if let Some(path) = opened
            && let Err(err) = (state.env.open_folder)(&path)
        {
            let path = path.display();
            state.notices.push(format!("Could not open {path}: {err}"));
        }
    }

    /// The bar along the bottom: where backups go and the backup button, or while a backup runs,
    /// the autosave mark and its progress.
    fn save_bar_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let visuals = ui.visuals();
        let frame = egui::Frame::new()
            .fill(visuals.window_fill)
            .inner_margin(egui::Margin::symmetric(0, 12));
        let bar = egui::Panel::bottom("save_bar")
            .frame(frame)
            .resizable(false)
            .show(ui, |ui| {
                theme::column(ui, |ui| self.save_bar_contents(ui, state, manifest_loading));
            });
        let progress = lock(&self.progress);
        let (checked, games) = *lock(&self.checked);
        // While the backup's scan runs, the rule shows the scan.
        let (done, total) = if checked < games {
            (checked, games)
        } else {
            (progress.done.len(), progress.total)
        };
        if self.running.is_some() && total > 0 {
            let done = done as f32 / total as f32;
            let rect = bar.response.rect;
            let rule = Rect::from_min_size(rect.left_top(), Vec2::new(rect.width() * done, 3.0));
            ui.painter()
                .rect_filled(rule, 0.0, theme::palette(ui).accent);
        }
    }

    fn save_bar_contents(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let selected = self.chosen();
        // A pending file write does not hold the button back: the click waits for it.
        let ready = state.session.is_some()
            && self.choices.loaded()
            && selected > 0
            && !manifest_loading
            && self.scanning.is_none()
            && self.running.is_none()
            && !self.backup_requested;
        let label = format!("Back up {}", count(selected, "game"));
        let back_up = theme::row(
            ui,
            |ui| self.save_bar_status(ui, state),
            |ui| theme::primary(ui, ready, Some(icons::FLOPPY_DISK), &label),
        );
        if back_up.clicked() {
            self.backup_requested = true;
            self.start_requested(ui.ctx(), state);
        }
    }

    /// Starts the requested backup once no pending file write runs, so the backup saves every
    /// change and is the file's only writer.
    fn start_requested(&mut self, ctx: &egui::Context, state: &State) {
        if self.backup_requested && !self.choices.busy() && self.running.is_none() {
            self.backup_requested = false;
            self.start_backup(ctx, state);
        }
    }

    /// The left of the save bar: the repository and whether it is encrypted, or the running
    /// backup's progress.
    fn save_bar_status(&self, ui: &mut Ui, state: &State) {
        let muted = ui.visuals().weak_text_color();
        if self.running.is_some() {
            autosave_mark(ui);
            let progress = lock(&self.progress);
            let (checked, games) = *lock(&self.checked);
            if progress.cleaning {
                ui.label("Removing old backups");
            } else if checked < games {
                ui.label(format!("Checked {checked} of {}", count(games, "game")));
            } else {
                ui.label(format!(
                    "Backed up {} of {}",
                    progress.done.len(),
                    count(progress.total, "game")
                ));
            }
        } else if let (Some(config), Some(_)) = (&state.config, &state.session) {
            let (glyph, encryption) = match config.password {
                PasswordMode::None => (icons::LOCK_SIMPLE_OPEN, "Not encrypted"),
                PasswordMode::Keyring | PasswordMode::Ask => (icons::LOCK_SIMPLE, "Encrypted"),
            };
            theme::mark(ui, glyph, 16.0, muted)
                .on_hover_text(encryption)
                .widget_info(|| WidgetInfo::labeled(Role::Image, true, encryption));
            ui.add(Label::new(RichText::new(&config.repository).color(muted)).truncate());
        } else {
            ui.weak("The repository is not open.");
        }
    }

    /// Backs up the chosen games in a job, with the staging mirrors under the cache dir
    /// (ADR-0004). The job first saves the pending selection changes to the repository and reads
    /// every game's settings again, so another machine's newer choices count. Then it scans
    /// again, so a save made since the last scan is included, and filters each game by its
    /// settings. Last, it removes old backups by the repository's retention policy.
    fn start_backup(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let Some((pending, settings)) = self.choices.for_backup() else {
            return;
        };
        let progress = Arc::new(Mutex::new(Progress {
            total: self.chosen(),
            done: Vec::new(),
            cleaning: false,
        }));
        self.progress = progress.clone();
        self.checked = Arc::default();
        let scanned = scan_progress(&self.checked, ctx);
        self.run_error = None;
        self.saving = SaveProblems::default();
        self.cleanup = None;
        let (manifest, config) = (state.manifest_or_empty(), config.clone());
        let (storage, stage_root, waker) = (
            session.storage.clone(),
            state.env.cache_dir.join("stage"),
            ctx.clone(),
        );
        let config_dir = state.env.config_dir.clone();
        let environment = state.env.environment.clone();
        self.running = Some(Job::spawn(ctx, move || {
            let mut pending = pending;
            let (mut saving, written) = match &mut pending {
                Some(pending) => save_selection(
                    &storage,
                    pending,
                    &config,
                    &settings,
                    &stage_root,
                    &config_dir,
                ),
                None => (SaveProblems::default(), None),
            };
            let reloaded = choices::load(&storage)
                .map(|games| choices::replay(games, pending.as_ref(), &config.secret_scope()));
            let filter = match &reloaded {
                Ok(games) => games,
                Err(err) => {
                    saving.warnings.push(format!(
                        "What to back up could not be read again, so the choices shown are \
                         used: {err:#}"
                    ));
                    &settings
                }
            };
            let environment = environment.unwrap_or_else(Environment::detect);
            let report = scan::scan(&manifest, &config, &environment, scanned);
            let games = chosen_files(&report, filter);
            lock(&progress).total = games.len();
            let backed_up = back_up(&storage, &games, &stage_root, |backup| {
                lock(&progress).done.push(summarize(backup));
                waker.request_repaint();
            });
            // A failed game still cleans up; a run that stopped early does not.
            let (error, cleanup) = match backed_up {
                Ok(made) => {
                    lock(&progress).cleaning = true;
                    waker.request_repaint();
                    (None, Some(clean_up(&storage, &made)))
                }
                Err(err) => (Some(format!("{err:#}")), None),
            };
            Ok(Backed {
                report,
                error,
                saving,
                cleanup,
                after: AfterBackup {
                    source: Arc::downgrade(&storage),
                    pending,
                    written,
                    games: reloaded.ok(),
                },
            })
        }));
    }

    fn results_ui(&self, ui: &mut Ui) {
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
        for failed in &self.saving.failed {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("What to back up was not saved for {failed}"),
            );
        }
        let warn = ui.visuals().warn_fg_color;
        for warning in &self.saving.warnings {
            theme::status(ui, icons::WARNING, warn, warning);
        }
        let accent = theme::palette(ui).accent;
        if let Some(Cleaned {
            policy_error,
            done,
            error,
        }) = &self.cleanup
        {
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

/// The app icon's save mark, the arc with its arrowhead, turning once every 1.2 s in the accent
/// green while a backup runs: the "saving, do not turn off" sign of games.
fn autosave_mark(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    // The icon's mark spans about 92 of its 128 units; its arc is centered on (64, 63).
    let scale = rect.width() / 92.0;
    let turn = ui.input(|input| input.time) as f32 * TAU / 1.2;
    let rotation = Rot2::from_angle(turn);
    let at = |x: f32, y: f32| rect.center() + rotation * (Vec2::new(x, y) * scale);
    // 270° from the top, clockwise, ending on the left where the arrowhead sits.
    let arc = (0..=24)
        .map(|step| {
            let angle = -FRAC_PI_2 + 1.5 * PI * step as f32 / 24.0;
            at(39.0 * angle.cos(), 39.0 * angle.sin())
        })
        .collect();
    let accent = theme::palette(ui).accent;
    let painter = ui.painter();
    painter.add(Shape::line(arc, Stroke::new(11.0 * scale, accent)));
    painter.add(Shape::convex_polygon(
        vec![at(-53.0, 2.0), at(-25.0, 2.0), at(-39.0, -18.0)],
        accent,
        Stroke::NONE,
    ));
    ui.ctx().request_repaint();
}

fn lock<T>(progress: &Mutex<T>) -> MutexGuard<'_, T> {
    progress.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scan progress callback that stores into `checked` and wakes `ctx`.
fn scan_progress(
    checked: &Arc<Mutex<(usize, usize)>>,
    ctx: &egui::Context,
) -> impl FnMut(usize, usize) + Send + 'static {
    let (checked, ctx) = (checked.clone(), ctx.clone());
    move |done, total| {
        *lock(&checked) = (done, total);
        // Once per percent: a repaint per game would wake the UI tens of thousands of times.
        if (done - 1) * 100 / total != done * 100 / total {
            ctx.request_repaint();
        }
    }
}

/// Backs up `games` into `storage`, calling `progress` as each game is done. Returns the ids of
/// the new snapshots.
fn back_up(
    storage: &SharedStorage,
    games: &[GameScan],
    stage_root: &Path,
    progress: impl FnMut(&GameBackup),
) -> anyhow::Result<Vec<String>> {
    let mut storage = lock_storage(storage)?;
    let backups = backup::backup_games(&mut storage, games, stage_root, &[], progress)?;
    let made = backups
        .into_iter()
        .filter_map(|backup| backup.result.ok().flatten());
    Ok(made.map(|snapshot| snapshot.id).collect())
}

/// Forgets and prunes by the repository's retention policy, never the snapshots `made`
/// (ADR-0015). A policy that cannot be read forgets nothing, but marked packs are still pruned.
fn clean_up(storage: &SharedStorage, made: &[String]) -> Cleaned {
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            return Cleaned {
                policy_error: None,
                done: Cleanup::default(),
                error: Some(format!("{err:#}")),
            };
        }
    };
    let (policy, policy_error) = match retention::load(&mut storage) {
        Ok(policy) => (policy, None),
        Err(err) => (Retention::default(), Some(chain(&err))),
    };
    let keep: Vec<&str> = made.iter().map(String::as_str).collect();
    let (done, error) = match retention::apply(&mut storage, &policy, &keep, SystemTime::now()) {
        Ok(done) => (done, None),
        Err(err) => {
            let done = Cleanup {
                forgotten: err.forgotten,
                ..Cleanup::default()
            };
            (done, Some(chain(&err.source)))
        }
    };
    Cleaned {
        policy_error,
        done,
        error,
    }
}

/// Saves this repository's pending changes of each game whose settings could be read; each
/// saved game's changes leave `pending` and the pending file in `config_dir`. Settings stage
/// below `stage_root` (ADR-0014). Returns the problems and, when the file was written, how the
/// last write went.
fn save_selection(
    storage: &SharedStorage,
    pending: &mut Pending,
    config: &AppConfig,
    settings: &Games,
    stage_root: &Path,
    config_dir: &Path,
) -> (SaveProblems, Option<Result<(), String>>) {
    let mut problems = SaveProblems::default();
    let scope = config.secret_scope();
    let games = pending
        .repositories
        .get(&scope)
        .cloned()
        .unwrap_or_default();
    if games.is_empty() {
        return (problems, None);
    }
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            problems.failed.push(format!("any game: {err:#}"));
            return (problems, None);
        }
    };
    let mut written = None;
    for (title, changes) in games {
        // Settings that could not be read are never written over.
        if matches!(settings.get(&title), Some(Err(_))) {
            continue;
        }
        match selection::save(&mut storage, &title, &changes, stage_root) {
            Ok(saved) => {
                if let Some(err) = saved.forget_error {
                    problems.warnings.push(format!(
                        "{title}: older copies of what to back up stay in the repository: {}",
                        chain(&err)
                    ));
                }
                pending.remove(config, &title);
                written = Some(pending.save(config_dir).map_err(|err| chain(&err)));
            }
            Err(err) => problems.failed.push(format!("{title}: {}", chain(&err))),
        }
    }
    (problems, written)
}

/// The games of `report` as `settings` filter them; a game skipped or with nothing left is
/// left out, and one whose settings cannot be read is backed up whole.
fn chosen_files(report: &ScanReport, settings: &Games) -> Vec<GameScan> {
    report
        .games
        .iter()
        .filter_map(|game| match settings.get(&game.title) {
            Some(Ok(settings)) => selection::apply(game, settings),
            _ => Some(game.clone()),
        })
        .collect()
}

/// What the scan found for one game, with its files as a checkbox tree and its registry keys
/// folded away. Returns the change the user made this frame; a folder to open goes to `opened`.
fn files_ui(
    ui: &mut Ui,
    game: &GameScan,
    tree: &mut FileTree,
    settings: &GameSettings,
    enabled: bool,
    opened: &mut Option<PathBuf>,
) -> Option<Change> {
    let included = |key: &str| settings.path_included(key);
    let summary = summary(game, tree.included(&included), settings);
    if game.files.is_empty() && game.registry_source.is_none() {
        ui.weak(summary);
        return None;
    }
    let summary = RichText::new(summary).color(ui.visuals().weak_text_color());
    egui::CollapsingHeader::new(summary)
        .id_salt(&game.title)
        .show(ui, |ui| {
            let clicked = ui.push_id(&game.title, |ui| tree.ui(ui, &included, enabled));
            let mut change = match clicked.inner {
                Some(Clicked::Toggle(node, included)) => Some(Change::Path { node, included }),
                Some(Clicked::Open(path)) => {
                    *opened = Some(path);
                    None
                }
                None => None,
            };
            if game.registry_source.is_some() {
                ui.weak("Registry");
                for key in &game.registry_keys {
                    let mut included = settings.registry_included(key);
                    if ui
                        .add_enabled(enabled, Checkbox::new(&mut included, key))
                        .changed()
                    {
                        change = Some(Change::Registry {
                            key: key.clone(),
                            included,
                        });
                    }
                }
            }
            change
        })
        .body_returned
        .flatten()
}

/// Gives the tab a backup that runs until the returned sender is dropped.
#[cfg(test)]
impl BackupTab {
    pub(crate) fn hold_backup(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.running = Some(job);
        release
    }

    /// Whether the backup's job has ended, without taking its result.
    pub(crate) fn run_ended(&mut self) -> bool {
        self.running.as_mut().is_none_or(|job| !job.is_running())
    }
}

#[cfg(test)]
mod tests;
