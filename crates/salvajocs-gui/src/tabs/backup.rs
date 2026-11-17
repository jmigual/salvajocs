//! The Backup tab: scan this machine for games, choose which to back up and see what each
//! backup did (ADR-0004, ADR-0010).

use std::collections::BTreeSet;
use std::error::Error;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui::emath::Rot2;
use eframe::egui::{
    self, Align, Label, Layout, Rect, RichText, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo,
};
use salvajocs_core::backup::{self, GameBackup};
use salvajocs_core::config::PasswordMode;
use salvajocs_core::scan::{self, Environment, GameScan, ProblemKind, ScanProblem, ScanReport};

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::theme::{self, icons};

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

/// A byte count with a binary unit, such as `1.5 MiB`.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn summary(game: &GameScan) -> String {
    let size: u64 = game.files.iter().map(|file| file.size).sum();
    let mut text = format!("{}, {}", count(game.files.len(), "file"), format_size(size));
    if game.registry_source.is_some() {
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
}

/// What a finished backup job hands back.
struct Backed {
    /// The scan the backup went by, which replaces the shown list.
    report: ScanReport,
    /// Why the run stopped early, if it did.
    error: Option<String>,
}

/// The state of the Backup tab.
#[derive(Default)]
pub struct BackupTab {
    scanning: Option<Job<ScanReport>>,
    report: Option<ScanReport>,
    scan_error: Option<String>,
    /// Titles of the games to back up.
    selected: BTreeSet<String>,
    running: Option<Job<Backed>>,
    progress: Arc<Mutex<Progress>>,
    run_error: Option<String>,
}

impl BackupTab {
    /// Draws the tab. The scan and the backup run in jobs; `state` is only read.
    /// `manifest_loading` is whether the manifest job is still running.
    pub fn ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        self.poll();
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

    fn poll(&mut self) {
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
                Ok(Backed { report, error }) => {
                    self.adopt(report);
                    self.run_error = error;
                }
                Err(err) => self.run_error = Some(format!("{err:#}")),
            }
        }
    }

    /// Shows `report`. The user's choice stays for the games already listed; a game new to the
    /// list starts selected.
    fn adopt(&mut self, report: ScanReport) {
        let known: BTreeSet<&str> = self
            .report
            .iter()
            .flat_map(|old| old.games.iter().map(|game| game.title.as_str()))
            .collect();
        self.selected = report
            .games
            .iter()
            .filter(|game| {
                self.selected.contains(&game.title) || !known.contains(game.title.as_str())
            })
            .map(|game| game.title.clone())
            .collect();
        self.report = Some(report);
    }

    /// Whether a scan, backup or other job of this tab is running.
    pub(crate) fn busy(&self) -> bool {
        self.scanning.is_some() || self.running.is_some()
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
        let enabled = !manifest_loading && !self.busy();
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
            self.scanning = Some(Job::spawn(ui.ctx(), move || {
                let environment = environment.unwrap_or_else(Environment::detect);
                Ok(scan::scan(&manifest, &config, &environment))
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
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning");
            });
        }
    }

    /// The scan report: its problems, a header with the count and the selection buttons, and the
    /// games as a boxed list.
    fn games_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let Some(report) = &self.report else {
            return;
        };
        let (games, titles): (usize, Vec<String>) = (
            report.games.len(),
            report.games.iter().map(|game| game.title.clone()).collect(),
        );
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.scan_ui(ui, state, manifest_loading, false);
                // Frameless until hovered, so the scan reads as the header's main action.
                let quiet = |text| egui::Button::new(text).frame_when_inactive(false);
                if ui.add(quiet("Deselect all")).clicked() {
                    self.selected.clear();
                }
                if ui.add(quiet("Select all")).clicked() {
                    self.selected = titles.into_iter().collect();
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(format!("{} found", count(games, "game")));
                });
            });
        });
        self.scan_notes_ui(ui, state, manifest_loading);
        let Some(report) = &self.report else {
            return;
        };
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
        theme::group(ui, |ui| {
            // A game's title and its summary read as one row.
            ui.spacing_mut().item_spacing.y = 2.0;
            for (index, game) in report.games.iter().enumerate() {
                if index > 0 {
                    theme::row_separator(ui);
                }
                let mut checked = self.selected.contains(&game.title);
                if ui.checkbox(&mut checked, &game.title).changed() {
                    if checked {
                        self.selected.insert(game.title.clone());
                    } else {
                        self.selected.remove(&game.title);
                    }
                }
                // Under the title, without the indent line: the row already groups them.
                ui.scope(|ui| {
                    ui.visuals_mut().indent_has_left_vline = false;
                    ui.indent(&game.title, |ui| files_ui(ui, game));
                });
            }
        });
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
                theme::column(ui, |ui| {
                    ui.horizontal(|ui| {
                        self.save_bar_contents(ui, state, manifest_loading);
                    });
                });
            });
        let progress = lock(&self.progress);
        if self.running.is_some() && progress.total > 0 {
            let done = progress.done.len() as f32 / progress.total as f32;
            let rect = bar.response.rect;
            let rule = Rect::from_min_size(rect.left_top(), Vec2::new(rect.width() * done, 3.0));
            ui.painter()
                .rect_filled(rule, 0.0, theme::palette(ui).accent);
        }
    }

    fn save_bar_contents(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let selected = self.selected.len();
            let ready =
                state.session.is_some() && selected > 0 && !manifest_loading && !self.busy();
            let label = format!("Back up {}", count(selected, "game"));
            if theme::primary(ui, ready, Some(icons::FLOPPY_DISK), &label).clicked() {
                self.start_backup(ui.ctx(), state);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                self.save_bar_status(ui, state);
            });
        });
    }

    /// The left of the save bar: the repository and whether it is encrypted, or the running
    /// backup's progress.
    fn save_bar_status(&self, ui: &mut Ui, state: &State) {
        let muted = ui.visuals().weak_text_color();
        if self.running.is_some() {
            autosave_mark(ui);
            let progress = lock(&self.progress);
            ui.label(format!(
                "Backed up {} of {}",
                progress.done.len(),
                count(progress.total, "game")
            ));
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

    /// Backs up the selected games in a job, with the staging mirrors under the cache dir
    /// (ADR-0004). The job scans again first, so a save made since the last scan is included.
    fn start_backup(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let progress = Arc::new(Mutex::new(Progress {
            total: self.selected.len(),
            done: Vec::new(),
        }));
        self.progress = progress.clone();
        self.run_error = None;
        let (manifest, config, titles) = (
            state.manifest_or_empty(),
            config.clone(),
            self.selected.clone(),
        );
        let (storage, stage_root, waker) = (
            session.storage.clone(),
            state.env.cache_dir.join("stage"),
            ctx.clone(),
        );
        let environment = state.env.environment.clone();
        self.running = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            let report = scan::scan(&manifest, &config, &environment);
            let games: Vec<GameScan> = report
                .games
                .iter()
                .filter(|game| titles.contains(&game.title))
                .cloned()
                .collect();
            lock(&progress).total = games.len();
            let error = back_up(&storage, &games, &stage_root, |backup| {
                lock(&progress).done.push(summarize(backup));
                waker.request_repaint();
            })
            .err()
            .map(|err| format!("{err:#}"));
            Ok(Backed { report, error })
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
        if progress.done.is_empty() {
            return;
        }
        ui.add_space(16.0);
        ui.heading("Last backup");
        ui.spacing_mut().item_spacing.y = 4.0;
        let accent = theme::palette(ui).accent;
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
            ui.horizontal(|ui| {
                theme::mark(ui, icons::CHECK_CIRCLE, 16.0, accent);
                ui.label(text);
            });
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

fn lock(progress: &Mutex<Progress>) -> MutexGuard<'_, Progress> {
    progress.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Backs up `games` into `storage`, calling `progress` as each game is done.
fn back_up(
    storage: &SharedStorage,
    games: &[GameScan],
    stage_root: &Path,
    progress: impl FnMut(&GameBackup),
) -> anyhow::Result<()> {
    let mut storage = lock_storage(storage)?;
    backup::backup_games(&mut storage, games, stage_root, &[], progress)?;
    Ok(())
}

/// What the scan found for one game, with the file list folded away.
fn files_ui(ui: &mut Ui, game: &GameScan) {
    let summary = summary(game);
    if game.files.is_empty() {
        ui.weak(summary);
        return;
    }
    let summary = RichText::new(summary).color(ui.visuals().weak_text_color());
    egui::CollapsingHeader::new(summary)
        .id_salt(&game.title)
        .show(ui, |ui| {
            for file in &game.files {
                ui.weak(format!("{}/{}", file.base.dir_name(), file.tail));
            }
        });
}

/// Gives the tab a backup that runs until the returned sender is dropped.
#[cfg(test)]
impl BackupTab {
    pub(crate) fn hold_backup(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.running = Some(job);
        release
    }
}

#[cfg(test)]
mod tests;
