//! The "Update available" notice in the top bar and the one-click update (ADR-0013).

use std::path::PathBuf;

use eframe::egui::{self, Ui};
use salvajocs_core::update::{self, Release};

use crate::job::Job;
use crate::state::State;

/// The running version, which the release is compared with.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The update check started with the app, and the update the user asked for.
#[derive(Default)]
pub struct Updater {
    checking: Option<Job<Option<Release>>>,
    release: Option<Release>,
    /// Whether the installer installed this app, read once the release is known.
    installed: bool,
    downloading: Option<Job<PathBuf>>,
    /// A verified installer, waiting for background jobs to end.
    ready: Option<PathBuf>,
}

impl Updater {
    /// Starts the check in the background, when the environment has one and the config allows it.
    pub fn start(&mut self, ctx: &egui::Context, state: &State) {
        let Some(check) = state.env.check_update else {
            return;
        };
        if state
            .config
            .as_ref()
            .is_some_and(|config| !config.check_updates)
        {
            return;
        }
        self.checking = Some(Job::spawn(ctx, move || Ok(check(VERSION)?)));
    }

    /// Collects the check and the download, and draws the notice in a right-to-left layout.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        if let Some(job) = &mut self.checking
            && let Some(result) = job.take_result()
        {
            self.checking = None;
            match result {
                Ok(release) => {
                    self.installed = (state.env.installed_by_installer)();
                    self.release = release;
                }
                // Offline users and GitHub's rate limit are no reason to bother anyone.
                Err(err) => eprintln!("salvajocs: update check failed: {err:#}"),
            }
        }
        if let Some(job) = &mut self.downloading
            && let Some(result) = job.take_result()
        {
            self.downloading = None;
            match result {
                Ok(installer) => self.ready = Some(installer),
                Err(err) => state.notices.push(format!("The update failed: {err:#}")),
            }
        }
        let Some(release) = &self.release else {
            return;
        };
        if self.downloading.is_some() {
            ui.label("Downloading the update");
            ui.spinner();
        } else if self.ready.is_some() {
            ui.label("Update ready, installs when the running task finishes");
        } else if self.installed {
            if ui.button("Update now").clicked() {
                let (release, dir) = (release.clone(), state.env.cache_dir.join("update"));
                self.downloading = Some(Job::spawn(ui.ctx(), move || {
                    Ok(update::download_installer(&release, VERSION, &dir)?)
                }));
            }
        } else {
            ui.hyperlink_to("Release page", &release.html_url);
        }
        ui.label(format!("Update available: v{}", release.version));
    }

    /// Starts the downloaded installer and closes the window, unless `busy`: closing would cut
    /// a running job, such as a backup, a restore or a config save, short.
    pub fn install_when_idle(&mut self, ctx: &egui::Context, state: &mut State, busy: bool) {
        if busy {
            return;
        }
        let Some(installer) = self.ready.take() else {
            return;
        };
        match (state.env.run_installer)(&installer) {
            // The installer waits for the app to close, then starts it again.
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(err) => state.notices.push(format!("The update failed: {err:#}")),
        }
    }
}

/// Makes `installer` the downloaded update, as if its download had just finished.
#[cfg(test)]
impl Updater {
    pub(crate) fn set_ready(&mut self, installer: PathBuf) {
        self.ready = Some(installer);
    }
}

#[cfg(test)]
mod tests;
