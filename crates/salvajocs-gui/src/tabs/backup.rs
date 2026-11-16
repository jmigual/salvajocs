//! The Backup tab: scan this machine for games, choose which to back up and see what each
//! backup did (ADR-0004, ADR-0010).

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::anyhow;
use eframe::egui::{self, Color32, Ui};
use salvajocs_core::backup::{self, GameBackup};
use salvajocs_core::scan::{self, Environment, GameScan, ProblemKind, ScanProblem, ScanReport};

use crate::job::Job;
use crate::state::{SharedStorage, State};

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

/// What the scan found for a game: file count and size, registry, and unreadable paths.
fn summary(game: &GameScan) -> String {
    let size: u64 = game.files.iter().map(|file| file.size).sum();
    let registry = if game.registry_source.is_some() {
        "yes"
    } else {
        "no"
    };
    let mut text = format!(
        "{} files, {}, registry: {registry}",
        game.files.len(),
        format_size(size)
    );
    if !game.unreadable.is_empty() {
        text += &format!(
            ", {} unreadable paths (the files may be incomplete)",
            game.unreadable.len()
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

/// A backup in flight.
struct Run {
    job: Job<()>,
    total: usize,
}

/// The state of the Backup tab.
#[derive(Default)]
pub struct BackupTab {
    scanning: Option<Job<ScanReport>>,
    report: Option<ScanReport>,
    scan_error: Option<String>,
    /// Titles of the games to back up.
    selected: BTreeSet<String>,
    run: Option<Run>,
    /// The outcomes of the current or last backup, filled in as each game finishes.
    results: Arc<Mutex<Vec<GameResult>>>,
    run_error: Option<String>,
}

impl BackupTab {
    /// Draws the tab. The scan and the backup run in jobs; `state` is only read.
    pub fn ui(&mut self, ui: &mut Ui, state: &State) {
        self.poll();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Backup");
            if state.config.is_none() {
                ui.label("Choose a repository first.");
                return;
            }
            self.scan_ui(ui, state);
            if let Some(error) = &self.scan_error {
                ui.colored_label(Color32::LIGHT_RED, format!("The scan failed: {error}"));
            }
            self.games_ui(ui, state);
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
                    self.selected = report.games.iter().map(|g| g.title.clone()).collect();
                    self.report = Some(report);
                    self.scan_error = None;
                }
                Err(err) => self.scan_error = Some(format!("{err:#}")),
            }
        }
        if let Some(run) = &mut self.run
            && let Some(result) = run.job.take_result()
        {
            self.run = None;
            self.run_error = result.err().map(|err| format!("{err:#}"));
        }
    }

    fn busy(&self) -> bool {
        self.scanning.is_some() || self.run.is_some()
    }

    fn scan_ui(&mut self, ui: &mut Ui, state: &State) {
        let manifest = state.manifest.as_ref();
        let enabled = manifest.is_some() && !self.busy();
        let button = ui.add_enabled(enabled, egui::Button::new("Scan for games"));
        if let (true, Some(manifest), Some(config)) = (button.clicked(), manifest, &state.config) {
            let (manifest, config) = (manifest.clone(), config.clone());
            self.scan_error = None;
            self.scanning = Some(Job::spawn(ui.ctx(), move || {
                Ok(scan::scan(&manifest, &config, &Environment::detect()))
            }));
        }
        if manifest.is_none() {
            ui.weak("The game manifest is not available yet.");
        }
        if self.scanning.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning");
            });
        }
    }

    fn games_ui(&mut self, ui: &mut Ui, state: &State) {
        let Some(report) = &self.report else {
            return;
        };
        if !report.problems.is_empty() {
            ui.separator();
            ui.colored_label(
                Color32::YELLOW,
                format!("Scan problems ({})", report.problems.len()),
            );
            for problem in &report.problems {
                ui.colored_label(Color32::YELLOW, describe_problem(problem));
            }
        }
        ui.separator();
        ui.label(format!("{} games found", report.games.len()));
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
        let ready = state.session.is_some() && selected > 0 && !self.busy();
        let button = egui::Button::new(format!("Back up selected ({selected})"));
        let clicked = ui.add_enabled(ready, button).clicked();
        if state.session.is_none() {
            ui.weak("The repository is not open.");
        }
        if let (true, Some(session)) = (clicked, &state.session) {
            let games: Vec<GameScan> = report
                .games
                .iter()
                .filter(|game| self.selected.contains(&game.title))
                .cloned()
                .collect();
            self.start_backup(ui.ctx(), games, &session.storage, &state.env.cache_dir);
        }
    }

    /// Backs up `games` in a job, with the staging mirrors under `cache_dir` (ADR-0004).
    fn start_backup(
        &mut self,
        ctx: &egui::Context,
        games: Vec<GameScan>,
        storage: &SharedStorage,
        cache_dir: &Path,
    ) {
        let results = Arc::new(Mutex::new(Vec::new()));
        self.results = results.clone();
        self.run_error = None;
        let total = games.len();
        let (storage, stage_root, waker) = (storage.clone(), cache_dir.join("stage"), ctx.clone());
        let job = Job::spawn(ctx, move || {
            let mut storage = storage
                .lock()
                .map_err(|_| anyhow!("the repository lock is poisoned"))?;
            backup::backup_games(&mut storage, &games, &stage_root, &[], |backup| {
                results
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(summarize(backup));
                waker.request_repaint();
            })?;
            Ok(())
        });
        self.run = Some(Run { job, total });
    }

    fn results_ui(&self, ui: &mut Ui) {
        let results = self.results.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(run) = &self.run {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Backing up: {} of {}", results.len(), run.total));
            });
        }
        if let Some(error) = &self.run_error {
            ui.colored_label(Color32::LIGHT_RED, format!("The backup failed: {error}"));
        }
        if results.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Results");
        for GameResult { title, outcome } in results.iter() {
            match outcome {
                Outcome::Saved => ui.label(format!("{title}: new snapshot")),
                Outcome::Unchanged => ui.label(format!("{title}: unchanged")),
                Outcome::Failed(error) => {
                    ui.colored_label(Color32::LIGHT_RED, format!("{title}: failed: {error}"))
                }
            };
        }
    }
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

#[cfg(test)]
mod tests;
