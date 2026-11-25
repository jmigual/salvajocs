//! The Backup tab: scan this machine for games, choose which to back up, see what differs from
//! the last backups, and what each backup and the cleanup after it did (ADR-0004, ADR-0010,
//! ADR-0015, ADR-0018).

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use eframe::egui::{self, Checkbox, Ui};
use salvajocs_core::config::{ListSort, SortKey};
use salvajocs_core::crossbuild::MappingEntry;
use salvajocs_core::retention::Cleanup;
use salvajocs_core::scan::{self, Environment, ProblemKind, ScanProblem, ScanReport};
use salvajocs_core::selection::{self, Change, GameSettings};
use salvajocs_core::slug::slug;
use salvajocs_core::stats::{self, BackupStats};
use salvajocs_core::storage::Storage;

use crate::job::Job;
use crate::state::{SharedStorage, State, chain};
use crate::tabs::count;
use crate::tabs::details::Details;
use crate::tabs::mapping::Dialog;
use crate::tabs::sort::{self, Line};
use crate::tabs::space::{Polled, Space};
use crate::theme::{self, icons};
use crate::widgets::named_button;
use choices::{AfterBackup, Choices};
use preview::{
    FileAction, Listed, Preview, badges, change_count, files_ui, header_of, preview_ui, to_choose,
    unknown,
};
use results::{GameResult, Outcome};
use unsent::{Asking, Checked, Shared, is_session};

mod bar;
mod choices;
mod preview;
mod results;
mod run;
pub(crate) mod unsent;

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
        _ => format!("{path}: cannot be scanned"),
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
    /// This PC's unsent custom games and rules are being saved, before anything else.
    sending: bool,
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
    /// This PC's rules could not be saved, so nothing was forgotten.
    rules_failed: bool,
    /// Why the retention policy could not be read, so nothing was forgotten.
    policy_error: Option<String>,
    /// What the cleanup did, up to `error` if it stopped.
    done: Cleanup,
    /// Why the cleanup stopped, if it did.
    error: Option<String>,
}

/// What a finished backup job hands back.
struct Backed {
    /// The repository the backup ran on.
    source: Weak<Mutex<Storage>>,
    /// What was saved of this PC's unsent changes.
    shared: Shared,
    /// The scan the backup went by, which replaces the shown list; `None` when no game was
    /// chosen, so only the unsent changes were saved.
    report: Option<ScanReport>,
    /// Why the run stopped early, if it did.
    error: Option<String>,
    /// What saving the selection ran into.
    saving: SaveProblems,
    /// What the backup left of the selection; `None` when no game was chosen.
    after: Option<AfterBackup>,
    /// The cleanup after the backup, `None` when the run stopped early or no game was chosen.
    cleanup: Option<Cleaned>,
}

/// The state of the Backup tab.
#[derive(Default)]
pub struct BackupTab {
    scanning: Option<Job<ScanReport>>,
    report: Option<ScanReport>,
    scan_error: Option<String>,
    /// Each game of `report`, in its order.
    listed: Vec<Listed>,
    /// Compares each game of `report` with its last backups; not counted as busy, since it only
    /// reads.
    previewing: Option<Job<Vec<Preview>>>,
    /// The preview of each game of `report`, in its order; empty until it is known.
    previews: Vec<Preview>,
    /// The repository `previewing` and `previews` compare with.
    compared_in: Option<SharedStorage>,
    /// The `Wine prefix: …` lines of each game of `report`, in its order.
    prefixes: Vec<Vec<String>>,
    /// The slug of each game of `report`, in its order.
    slugs: Vec<String>,
    /// The backup figures of the repository's games by slug, from the last comparison's
    /// listing; kept across scans so the order does not jump while a comparison runs. `None`
    /// until a listing arrives (ADR-0027).
    stats: Option<HashMap<String, BackupStats>>,
    /// Where the running comparison puts the figures as soon as it has listed the snapshots.
    listed_stats: Arc<Mutex<Option<HashMap<String, BackupStats>>>>,
    /// Works out the space of each game's backups.
    space: Option<Space>,
    /// Counts the changes of `report`, of the figures, and under Changes of the previews and the
    /// loaded settings, which the order is built from.
    generation: u64,
    /// The indices of `report.games` in the order shown.
    order: Vec<usize>,
    /// The sort and `generation` that `order` was built for.
    sorted: Option<(ListSort, u64)>,
    /// The line of figures under each game of `report`, in its order; `None` until drawn.
    lines: Vec<Option<Line>>,
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
    /// A backup saved snapshots since [`BackupTab::take_saved`] last ran.
    saved: bool,
    /// Back up was clicked; the backup starts once the pending file is written.
    backup_requested: bool,
    /// Checks the repository it was started for for changes made on another PC to what this PC
    /// is about to save; it only reads.
    checking: Option<(Weak<Mutex<Storage>>, Job<Checked>)>,
    /// The open question about changes made on another PC, before the backup.
    asking: Option<Asking>,
    /// The repository a passed check, or the answered question, was for: the backup starts once
    /// no pending file write runs.
    cleared: Option<Weak<Mutex<Storage>>>,
    /// The slugs of the games the user chose to back up although their saves differ from the
    /// newest backup's; for the next backup only, never saved (ADR-0035).
    back_up_anyway: BTreeSet<String>,
    /// The slug of the one game the requested backup is for, from its **Back up anyway**; that
    /// game is backed up whatever its state, with no question, and no other. For the next backup
    /// only, never saved (ADR-0035).
    only: Option<String>,
    /// What the last backup saved of this PC's unsent custom games and rules.
    shared: Shared,
    /// A backup saved this PC's rules, or the question about them was answered, since
    /// [`BackupTab::take_rules_changed`] last ran.
    rules_changed: bool,
    /// The open mapping dialog: the game's title and the roots it shows.
    mapping: Option<(String, Vec<MappingEntry>, Dialog)>,
    /// The open details of a changed file; it only reads, so it is not counted as busy.
    details: Option<Details>,
}

impl BackupTab {
    /// Draws the tab. The scan and the backup run in jobs; `state` only gains notices.
    /// `manifest_loading` is whether the manifest job is still running.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        self.poll(ui.ctx(), state);
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
        self.mapping_ui(ui.ctx(), state);
        self.asking_ui(ui.ctx(), state);
        if (self.details.as_mut()).is_some_and(|details| details.ui(ui.ctx())) {
            self.details = None;
        }
    }

    /// Collects the tab's finished jobs and starts what waits on them. The app calls it every
    /// frame, whichever tab is in front, so a pending file write or a load never stalls.
    pub(crate) fn poll(&mut self, ctx: &egui::Context, state: &mut State) {
        if let Some(job) = &mut self.scanning
            && let Some(result) = job.take_result()
        {
            self.scanning = None;
            match result {
                Ok(report) => {
                    self.adopt(report);
                    self.start_preview(ctx, state);
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
                    source,
                    shared,
                    report,
                    error,
                    saving,
                    after,
                    cleanup,
                }) => {
                    if let Some(report) = report {
                        self.adopt(report);
                        self.start_preview(ctx, state);
                    }
                    self.run_error = error;
                    self.saving = saving;
                    // A game saved before the run stopped early counts too.
                    self.saved |= (lock(&self.progress).done.iter()).any(|game| {
                        matches!(game.outcome, Outcome::Saved | Outcome::SavedAsNewest)
                    });
                    self.removed |= cleanup.as_ref().is_some_and(|c| c.done.forgotten > 0);
                    self.cleanup = cleanup;
                    if let Some(after) = after
                        && self.choices.backed_up(after)
                    {
                        self.listed.iter_mut().for_each(Listed::invalidate);
                    }
                    // Another repository, connected meanwhile, never got these changes.
                    let sent = !shared.games.is_empty() || shared.rules.is_some();
                    if sent && is_session(&source, state) {
                        state.custom_games_sent(ctx, &shared.games, shared.rules);
                        self.rules_changed |= shared.rules.is_some();
                    }
                    self.shared = shared;
                }
                Err(err) => self.run_error = Some(format!("{err:#}")),
            }
        }
        // A comparison with a repository Settings replaced is dropped and made again.
        let current = state.session.as_ref().map(|session| &session.storage);
        if (self.compared_in.as_ref())
            .is_some_and(|compared| !current.is_some_and(|current| Arc::ptr_eq(compared, current)))
        {
            self.compared_in = None;
            self.previewing = None;
            self.details = None;
            self.previews = Vec::new();
            self.stats = None;
            self.space = None;
            self.generation += 1;
            if let Some(report) = &self.report {
                self.listed = (report.games.iter())
                    .map(|game| Listed::new(game, None))
                    .collect();
            }
            self.start_preview(ctx, state);
        }
        let listed_stats = lock(&self.listed_stats).take();
        if let Some(stats) = listed_stats {
            self.adopt_stats(ctx, state, stats);
        }
        if let Some(space) = &mut self.space {
            match space.poll() {
                Polled::Arrived => self.lines.fill(None),
                // The order changes once, when every figure is in.
                Polled::Finished => self.generation += 1,
                Polled::Nothing => {}
            }
        }
        let by_changes =
            (state.config.as_ref()).is_some_and(|c| c.backup_sort.by == SortKey::Changes);
        if let Some(job) = &mut self.previewing
            && let Some(result) = job.take_result()
        {
            self.previewing = None;
            let games = self.report.as_ref().map_or(0, |report| report.games.len());
            self.previews = match result {
                Ok(previews) => previews,
                Err(err) => (0..games).map(|_| Err(format!("{err:#}"))).collect(),
            };
            // Other keys must not reorder on previews: the space figures arrive on their own.
            if by_changes {
                self.generation += 1;
            }
            if let Some(report) = &self.report {
                let games = report.games.iter().zip(&self.previews);
                self.listed = games
                    .map(|(game, preview)| Listed::new(game, Some(preview)))
                    .collect();
            }
        }
        if let Some(details) = &mut self.details {
            details.poll();
        }
        if self.choices.poll(ctx, state) {
            self.listed.iter_mut().for_each(Listed::invalidate);
            // The loaded settings change the counts the order by changes was built from.
            if by_changes {
                self.generation += 1;
            }
        }
        if let Some((source, job)) = &mut self.checking
            && let Some(result) = job.take_result()
        {
            let source = source.clone();
            self.checking = None;
            // A check of a repository Settings replaced is dropped, and so is its backup.
            if is_session(&source, state) {
                let syncs = self.sync_asks();
                match result {
                    Ok(Checked { games, rules })
                        if games.is_empty() && rules.is_none() && syncs.is_empty() =>
                    {
                        self.cleared = Some(source);
                    }
                    Ok(Checked { games, rules }) => {
                        self.asking = Some(Asking {
                            source,
                            games: games.into_iter().map(|conflict| (conflict, None)).collect(),
                            rules: rules.map(|stored| (stored, None)),
                            syncs,
                        });
                    }
                    Err(err) => {
                        self.run_error = Some(format!(
                            "The repository could not be checked for changes made on another \
                             PC: {err:#}"
                        ));
                    }
                }
            }
        }
        if (self.asking.as_ref()).is_some_and(|asking| !is_session(&asking.source, state)) {
            self.asking = None;
        }
        if (self.cleared.as_ref()).is_some_and(|source| !is_session(source, state)) {
            self.cleared = None;
        }
        self.start_requested(ctx, state);
    }

    /// Whether a finished cleanup forgot snapshots since the last call, so a loaded list of
    /// backups may name one that is gone. A flag rather than `poll`'s result, since `ui` polls
    /// too and would swallow it.
    pub(crate) fn take_removed(&mut self) -> bool {
        std::mem::take(&mut self.removed)
    }

    /// Whether a finished backup saved snapshots since the last call, so a loaded list of
    /// backups misses them. A flag for the same reason as [`BackupTab::take_removed`].
    pub(crate) fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    /// Whether a finished backup saved this PC's Old backups rules, or the question about them
    /// was answered, since the last call, so the rules shown in Settings are read again and the
    /// next edit starts from them.
    pub(crate) fn take_rules_changed(&mut self) -> bool {
        std::mem::take(&mut self.rules_changed)
    }

    /// Shows `stats`, the figures of the repository's games, and works out the space of the
    /// report's games that the session has not yet.
    fn adopt_stats(
        &mut self,
        ctx: &egui::Context,
        state: &State,
        stats: HashMap<String, BackupStats>,
    ) {
        if let Some(session) = &state.session {
            let games = (self.slugs.iter())
                .filter_map(|slug| Some(stats.get(slug)?.snapshot_ids.clone()))
                .collect();
            let space = Space::start(ctx, &session.storage, &session.space, games);
            self.space = Some(space);
        }
        self.stats = Some(stats);
        self.generation += 1;
    }

    /// Builds the order of the games by `sort`, and drops the lines of figures, when the sort,
    /// the report, the figures or (under Changes) the previews changed since it was last built.
    fn sort_rows(&mut self, sort: ListSort) {
        if self.sorted == Some((sort, self.generation)) {
            return;
        }
        let Some(report) = &self.report else {
            return;
        };
        if let (Some(figures), Some(space)) = (&mut self.stats, &self.space) {
            for game in figures.values_mut() {
                game.space_used = space.get(&game.snapshot_ids).and_then(Result::ok);
            }
        }
        let figures = self.stats.as_ref();
        let rows: Vec<_> = (report.games.iter().zip(&self.slugs).enumerate())
            .map(|(index, (game, slug))| {
                // Read once per build: a tick must not move the game under the cursor.
                let changes = (self.previews.get(index)).and_then(|preview| {
                    let settings = self.choices.game(&game.title).unwrap_or_default();
                    change_count(game, preview, &settings)
                });
                (
                    game.title.as_str(),
                    figures.and_then(|f| f.get(slug)),
                    changes,
                )
            })
            .collect();
        self.order = stats::order(&rows, sort);
        self.lines = vec![None; report.games.len()];
        self.sorted = Some((sort, self.generation));
    }

    /// Shows `report`, with a file tree per game and no preview yet.
    fn adopt(&mut self, report: ScanReport) {
        self.listed = (report.games.iter())
            .map(|game| Listed::new(game, None))
            .collect();
        self.slugs = report.games.iter().map(|game| slug(&game.title)).collect();
        self.generation += 1;
        self.previews = Vec::new();
        self.previewing = None;
        self.details = None;
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
            || self.checking.is_some()
            || self.asking.is_some()
            || self.cleared.is_some()
            || self.choices.busy()
    }

    /// Whether a Back up was asked for and has not ended: its check, question, wait or job.
    pub(crate) fn backing_up(&self) -> bool {
        self.backup_requested
            || self.checking.is_some()
            || self.asking.is_some()
            || self.cleared.is_some()
            || self.running.is_some()
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
        let games = report.games.iter().zip(&mut self.listed);
        let previews = &self.previews;
        games
            .enumerate()
            .map(|(index, (game, listed))| match choices.game(&game.title) {
                Ok(settings) => {
                    let badge = badges(game, previews.get(index), &settings);
                    let (files, _) = listed
                        .tree
                        .included(&|key| settings.path_included(key), &badge);
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
        // A Back up waiting on its check, or to start, scans again itself.
        let enabled = !manifest_loading
            && self.scanning.is_none()
            && self.running.is_none()
            && self.checking.is_none()
            && self.cleared.is_none();
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
    /// games as a boxed list in the config's order, under the Sort by row.
    fn games_ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        let sort = (state.config.as_ref()).map_or(ListSort::default(), |c| c.backup_sort);
        self.sort_rows(sort);
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
        if self.previewing.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Comparing with the last backups");
            });
        }
        self.choices.status_ui(ui, state);
        let can_anyway = self.can_start(state, manifest_loading) && self.previewing.is_none();
        let Some(report) = &self.report else {
            return;
        };
        // By index into `report.games`, applied after drawing, like the folder to open.
        let mut changes = Vec::new();
        // The game's index and what was clicked in its tree.
        let mut clicked: Option<(usize, FileAction)> = None;
        let mut choose = None;
        let mut anyway = None;
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
        let picked = sort::sort_ui(ui, "backup_sort", SortKey::ALL, sort);
        let (choices, listed, prefixes) = (&self.choices, &mut self.listed, &self.prefixes);
        let (previews, order, lines) = (&self.previews, &self.order, &mut self.lines);
        let (figures, space, slugs) = (&self.stats, &self.space, &self.slugs);
        theme::group(ui, |ui| {
            // A game's title and its summary read as one row.
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 24.0;
            for (position, &index) in order.iter().enumerate() {
                if position > 0 {
                    theme::row_separator(ui);
                }
                let (game, listed) = (&report.games[index], &mut listed[index]);
                let prefixes = &prefixes[index];
                let (settings, error) = match choices.game(&game.title) {
                    Ok(settings) => (settings, None),
                    Err(error) => (Cow::Owned(GameSettings::default()), Some(error)),
                };
                let enabled = editable && error.is_none();
                let preview = previews.get(index);
                let header =
                    (listed.header).get_or_insert_with(|| header_of(game, preview, &settings));
                theme::row(
                    ui,
                    |ui| {
                        let mut checked = !settings.skip;
                        let checkbox = Checkbox::new(&mut checked, &game.title);
                        if ui.add_enabled(enabled, checkbox).changed() {
                            changes.push((index, Change::Skip(!checked)));
                        }
                    },
                    |ui| preview_ui(ui, header),
                );
                // Under the title, without the indent line: the row already groups them.
                ui.scope(|ui| {
                    ui.visuals_mut().indent_has_left_vline = false;
                    ui.indent(&game.title, |ui| {
                        // No line before the listing: "No backup" would be a guess.
                        if let Some(figures) = figures {
                            let line = lines[index].get_or_insert_with(|| {
                                let stats = figures.get(&slugs[index]);
                                let ids = stats.map(|stats| stats.snapshot_ids.as_slice());
                                let space_used = space.as_ref().zip(ids);
                                let space_used = space_used.and_then(|(space, ids)| space.get(ids));
                                sort::backup_line(stats, space_used, sort.by)
                            });
                            line.ui(ui);
                        }
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
                        if let Some(entries) = to_choose(preview) {
                            ui.horizontal(|ui| {
                                ui.weak(format!(
                                    "The newest backup has {} from the other build without a \
                                     place here.",
                                    count(unknown(entries), "folder")
                                ));
                                if ui.button("Choose folders…").clicked() {
                                    choose = Some(index);
                                }
                            });
                        }
                        if header.offers_anyway() {
                            let name = format!("Back up {} anyway", game.title);
                            if named_button(ui, can_anyway, "Back up anyway", &name).clicked() {
                                anyway = Some(index);
                            }
                        }
                        let tree = &mut listed.tree;
                        let mut action = None;
                        let change =
                            files_ui(ui, game, tree, preview, &settings, enabled, &mut action);
                        if let Some(change) = change {
                            changes.push((index, change));
                        }
                        if let Some(action) = action {
                            clicked = Some((index, action));
                        }
                    });
                });
            }
        });
        if let (Some(picked), Some(config)) = (picked, &mut state.config) {
            config.backup_sort = picked;
            state.save_config(ui.ctx());
        }
        for (index, change) in changes {
            self.choices
                .change(ui.ctx(), state, &report.games[index].title, change);
            self.listed[index].invalidate();
        }
        match clicked {
            Some((_, FileAction::Open(path))) => {
                if let Err(err) = (state.env.open_folder)(&path) {
                    let path = path.display();
                    state.notices.push(format!("Could not open {path}: {err}"));
                }
            }
            Some((index, FileAction::Details(file))) => {
                let game = &report.games[index];
                let key = selection::path_key(&game.files[file]);
                if let Some(Ok(Some(baseline))) = self.previews.get(index)
                    && let Some(backed_up) = baseline.newest.backed_up(&key)
                    && let Some(session) = &state.session
                {
                    let (ctx, storage) = (ui.ctx(), &session.storage);
                    let local = game.files[file].real.clone();
                    let details =
                        Details::start(ctx, storage, slug(&game.title), key, local, backed_up);
                    self.details = Some(details);
                }
            }
            None => {}
        }
        if let Some(index) = choose
            && let Some(entries) = to_choose(self.previews.get(index))
        {
            let title = report.games[index].title.clone();
            self.mapping = Some((title, entries.to_vec(), Dialog::new(entries.to_vec())));
        }
        if let Some(index) = anyway {
            let slug = self.slugs[index].clone();
            self.request_anyway(ui.ctx(), state, slug);
        }
    }
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

/// Gives the tab a backup that runs until the returned sender is dropped.
#[cfg(test)]
impl BackupTab {
    /// Makes the tab report saved snapshots, as after a backup.
    pub(crate) fn mark_saved(&mut self) {
        self.saved = true;
    }

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
