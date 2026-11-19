//! The "Found by salvajocs" list of the Roots section: the Wine prefixes of Heroic, Lutris and
//! Bottles, each with a switch that leaves it out of scans and restores (ADR-0017).

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::Ui;
use salvajocs_core::launcher::{GameIndex, Launcher};
use salvajocs_core::manifest::Manifest;
use salvajocs_core::scan::Environment;

use crate::job::Job;
use crate::state::State;
use crate::theme;

fn launcher_label(launcher: Launcher) -> &'static str {
    match launcher {
        Launcher::Heroic => "Heroic",
        Launcher::Lutris => "Lutris",
        Launcher::Bottles => "Bottles",
        _ => "Other launcher",
    }
}

/// One prefix path. Several Heroic games without their own prefix share the default one, and
/// ignoring is by path, so they share a row.
struct Row {
    path: PathBuf,
    launchers: Vec<&'static str>,
    /// The titles of the games mapped to the prefix.
    titles: Vec<String>,
}

/// The launcher prefixes found, one row per path.
struct Found {
    /// The manifest the titles come from; the list is found again once another is loaded.
    manifest: Option<Arc<Manifest>>,
    rows: Vec<Row>,
}

/// The state of the list.
#[derive(Default)]
pub struct LaunchersSection {
    found: Option<Found>,
    finding: Option<Job<Found>>,
}

impl LaunchersSection {
    /// Draws the list, looking for the prefixes in the background first.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        if state.config.is_none() {
            return;
        }
        if let Some(job) = &mut self.finding
            && let Some(result) = job.take_result()
        {
            self.finding = None;
            self.found = Some(result.unwrap_or_else(|err| {
                state
                    .notices
                    .push(format!("Could not look for launcher prefixes: {err:#}"));
                Found {
                    manifest: state.manifest.clone(),
                    rows: Vec::new(),
                }
            }));
        }
        let current = state.manifest.as_ref().map(Arc::as_ptr);
        let stale = (self.found.as_ref())
            .is_none_or(|found| found.manifest.as_ref().map(Arc::as_ptr) != current);
        if stale && self.finding.is_none() {
            let environment = state.env.environment.clone();
            let manifest = state.manifest.clone();
            self.finding = Some(Job::spawn(ui.ctx(), move || {
                Ok(find(environment, manifest))
            }));
        }
        let Some(config) = &mut state.config else {
            return;
        };
        theme::row_separator(ui);
        ui.label(theme::semibold("Found by salvajocs"));
        ui.weak(
            "Wine prefixes of Heroic, Lutris and Bottles. Unticked ones are left out, unless the \
             same folder is also a Wine prefix root or a custom game's prefix.",
        );
        let Some(found) = &self.found else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for launcher prefixes");
            });
            return;
        };
        if found.rows.is_empty() {
            ui.weak("None found.");
        }
        let mut changed = false;
        for row in &found.rows {
            let launchers = row.launchers.join(", ");
            let path = row.path.display();
            let text = match row.titles.as_slice() {
                [] => format!("{path} ({launchers})"),
                titles => format!("{path} ({launchers}: {})", titles.join(", ")),
            };
            let ignored = &mut config.ignored_prefixes;
            let mut used = !ignored.contains(&row.path);
            if ui.checkbox(&mut used, text).changed() {
                if used {
                    ignored.retain(|path| *path != row.path);
                } else {
                    ignored.push(row.path.clone());
                }
                changed = true;
            }
        }
        if changed {
            state.save_config(ui.ctx());
        }
    }
}

/// The launcher prefixes of `environment`, or of this machine when `None`, by path, with the
/// titles they map to in `manifest`.
fn find(environment: Option<Environment>, manifest: Option<Arc<Manifest>>) -> Found {
    let environment = environment.unwrap_or_else(Environment::detect);
    let empty = Manifest::default();
    let index = GameIndex::new(manifest.as_deref().unwrap_or(&empty));
    let mut rows: Vec<Row> = Vec::new();
    for prefix in &environment.launcher_prefixes {
        let at = match rows.iter().position(|row| row.path == prefix.path) {
            Some(at) => at,
            None => {
                rows.push(Row {
                    path: prefix.path.clone(),
                    launchers: Vec::new(),
                    titles: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        let launcher = launcher_label(prefix.launcher);
        if !row.launchers.contains(&launcher) {
            row.launchers.push(launcher);
        }
        if let Some(title) = index.title(prefix)
            && !row.titles.iter().any(|known| known == title)
        {
            row.titles.push(title.to_owned());
        }
    }
    Found { manifest, rows }
}
