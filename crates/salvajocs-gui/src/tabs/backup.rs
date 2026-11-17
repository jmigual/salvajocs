//! The Backup tab: scan this machine for games, choose which to back up and see what each
//! backup did (ADR-0004, ADR-0010).

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui::{self, Ui};
use salvajocs_core::backup::{self, GameBackup};
use salvajocs_core::scan::{self, Environment, GameScan, ProblemKind, ScanProblem, ScanReport};

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;

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
        egui::ScrollArea::vertical().show(ui, |ui| {
            if state.config.is_none() {
                ui.label("Choose a repository first.");
                return;
            }
            self.scan_ui(ui, state, manifest_loading);
            if let Some(error) = &self.scan_error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("The scan failed: {error}"),
                );
            }
            self.games_ui(ui, state, manifest_loading);
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

    fn scan_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let enabled = !manifest_loading && !self.busy();
        let button = ui.add_enabled(enabled, egui::Button::new("Scan for games"));
        if let (true, Some(config)) = (button.clicked(), &state.config) {
            let (manifest, config) = (state.manifest_or_empty(), config.clone());
            let environment = state.env.environment.clone();
            self.scan_error = None;
            self.scanning = Some(Job::spawn(ui.ctx(), move || {
                let environment = environment.unwrap_or_else(Environment::detect);
                Ok(scan::scan(&manifest, &config, &environment))
            }));
        }
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

    fn games_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let Some(report) = &self.report else {
            return;
        };
        if !report.problems.is_empty() {
            ui.separator();
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Scan problems ({})", report.problems.len()),
            );
            for problem in &report.problems {
                ui.colored_label(ui.visuals().warn_fg_color, describe_problem(problem));
            }
        }
        ui.separator();
        ui.label(format!("{} found", count(report.games.len(), "game")));
        ui.horizontal(|ui| {
            if ui.button("Select all").clicked() {
                self.selected = report.games.iter().map(|g| g.title.clone()).collect();
            }
            if ui.button("Deselect all").clicked() {
                self.selected.clear();
            }
        });
        for game in &report.games {
            let mut checked = self.selected.contains(&game.title);
            if ui.checkbox(&mut checked, &game.title).changed() {
                if checked {
                    self.selected.insert(game.title.clone());
                } else {
                    self.selected.remove(&game.title);
                }
            }
            ui.indent(&game.title, |ui| files_ui(ui, game));
        }
        ui.separator();
        let selected = self.selected.len();
        let ready = state.session.is_some() && selected > 0 && !manifest_loading && !self.busy();
        let button = egui::Button::new(format!("Back up {}", count(selected, "game")));
        let clicked = ui.add_enabled(ready, button).clicked();
        if state.session.is_none() {
            ui.weak("The repository is not open.");
        }
        if clicked {
            self.start_backup(ui.ctx(), state);
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
        if self.running.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Backed up {} of {}",
                    progress.done.len(),
                    count(progress.total, "game")
                ));
            });
        }
        if let Some(error) = &self.run_error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("The backup failed: {error}"),
            );
        }
        if progress.done.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Last backup");
        for GameResult { title, outcome } in &progress.done {
            match outcome {
                Outcome::Saved => ui.label(format!("{title}: backed up")),
                Outcome::Unchanged => ui.label(format!("{title}: up to date")),
                Outcome::Failed(error) => ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{title}: failed: {error}"),
                ),
            };
        }
    }
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
    egui::CollapsingHeader::new(summary)
        .id_salt(&game.title)
        .show(ui, |ui| {
            for file in &game.files {
                ui.label(format!("{}/{}", file.base.dir_name(), file.tail));
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
