//! The "Update available" notice in the bottom bar and the one-click update (ADR-0013, ADR-0022).

use std::path::PathBuf;

use eframe::egui::{self, Ui};
use salvajocs_core::update::{self, Release};

use crate::job::Job;
use crate::state::State;
use crate::status_bar;
use crate::theme::{self, icons};

/// The running version, which the release is compared with.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The update check started with the app, and the update the user asked for.
#[derive(Default)]
pub struct Updater {
    checking: Option<Job<Option<Release>>>,
    release: Option<Release>,
    /// Whether the installer installed this app, read once the release is known.
    installed: bool,
    /// The running AppImage when its folder takes the new one, read once the release is known.
    appimage: Option<PathBuf>,
    downloading: Option<Job<PathBuf>>,
    /// The AppImage was replaced by the new release, so **Restart now** starts it.
    replaced: bool,
    /// A verified installer, or the replaced AppImage after **Restart now**, waiting for
    /// background jobs to end.
    ready: Option<PathBuf>,
    /// The replaced AppImage, started by [`Updater::restart`] once the window closed.
    restart: Option<PathBuf>,
    /// An open form with edits holds the ready update, which no running job does.
    held_by_form: bool,
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

    /// Collects the check and the download.
    pub fn poll(&mut self, state: &mut State) {
        if let Some(job) = &mut self.checking
            && let Some(result) = job.take_result()
        {
            self.checking = None;
            match result {
                Ok(release) => {
                    self.installed = (state.env.installed_by_installer)();
                    // Before any download, so a read-only folder gets the link, not a failure.
                    self.appimage = state
                        .env
                        .appimage
                        .clone()
                        .filter(|appimage| release.is_some() && update::replaceable(appimage));
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
                Ok(_) if self.appimage.is_some() => self.replaced = true,
                Ok(installer) => self.ready = Some(installer),
                Err(err) => state.notices.push(format!("The update failed: {err:#}")),
            }
        }
    }

    /// Draws the notice left to right, once [`Updater::poll`] found a newer release.
    pub fn ui(&mut self, ui: &mut Ui, state: &State) {
        let Some(release) = &self.release else {
            return;
        };
        let (updatable, downloading, replaced, ready) = (
            self.installed || self.appimage.is_some(),
            self.downloading.is_some(),
            self.replaced,
            self.ready.is_some(),
        );
        let notice = format!("Update available: v{}", release.version);
        // What keeps its room while the text around it gives way: the spinner or the action.
        let fixed = |ui: &mut Ui, notice_dropped: bool| {
            if downloading {
                ui.spinner();
                false
            } else if ready {
                false
            } else if replaced {
                theme::primary(ui, true, None, "Restart now").clicked()
            } else if updatable {
                theme::primary(ui, true, None, "Update now").clicked()
            } else {
                let link = ui.hyperlink_to("Release page", &release.html_url);
                if notice_dropped {
                    // Screen readers and the pointer still get what the dropped text said.
                    let link = link.on_hover_text(&notice);
                    ui.ctx().accesskit_node_builder(link.id, |node| {
                        node.set_label(format!("Release page, {notice}"));
                    });
                }
                let color = ui.visuals().hyperlink_color;
                theme::mark(ui, icons::ARROW_SQUARE_OUT, 16.0, color);
                false
            }
        };
        let reserved = if ready {
            0.0
        } else {
            theme::natural_width(ui, |ui| {
                fixed(ui, false);
            }) + ui.spacing().item_spacing.x
        };
        let room = ui.available_width() - reserved;
        let text = status_bar::text_fits(ui, room);
        if text {
            ui.scope(|ui| {
                ui.set_max_width(room);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                theme::mark(ui, icons::ARROW_CIRCLE_UP, 16.0, theme::palette(ui).accent);
                ui.label(&notice);
            });
        }
        if fixed(ui, !text) {
            if replaced {
                self.ready = self.appimage.clone();
            } else if let Some(appimage) = self.appimage.clone() {
                let release = release.clone();
                self.downloading = Some(Job::spawn(ui.ctx(), move || {
                    update::download_appimage(&release, VERSION, &appimage)?;
                    Ok(appimage)
                }));
            } else {
                let (release, dir) = (release.clone(), state.env.cache_dir.join("update"));
                self.downloading = Some(Job::spawn(ui.ctx(), move || {
                    Ok(update::download_installer(&release, VERSION, &dir)?)
                }));
            }
        }
        if downloading {
            status_bar::status_text(ui, |ui| {
                ui.label("Downloading the update");
            });
        } else if ready && self.held_by_form {
            status_bar::status_text(ui, |ui| {
                ui.weak("Save or cancel the open form to install the update");
            });
        } else if ready && replaced {
            status_bar::status_text(ui, |ui| {
                ui.weak("Restarts when the running task finishes");
            });
        } else if ready {
            status_bar::status_text(ui, |ui| {
                ui.weak("Update ready, installs when the running task finishes");
            });
        }
    }

    /// Draws the notice on a line of its own, for a dialog that covers the bottom bar, then the
    /// state's notices, such as a failed update, which the top bar would show out of sight.
    pub fn dialog_ui(&mut self, ui: &mut Ui, state: &State) {
        if self.release.is_some() {
            ui.horizontal(|ui| self.ui(ui, state));
        }
        for notice in &state.notices {
            theme::status(ui, icons::WARNING, ui.visuals().warn_fg_color, notice);
        }
    }

    /// Starts the downloaded installer and closes the window, or closes it for a restart into the
    /// replaced AppImage, unless `busy`: closing would cut a running job, such as a backup, a
    /// restore or a config save, short. A form that `editing` holds unsaved edits would be lost
    /// the same way; the notice says so.
    pub fn install_when_idle(
        &mut self,
        ctx: &egui::Context,
        state: &mut State,
        busy: bool,
        editing: bool,
    ) {
        let held = editing && !busy;
        if held != self.held_by_form {
            self.held_by_form = held;
            ctx.request_repaint();
        }
        if busy || editing {
            return;
        }
        let Some(installer) = self.ready.take() else {
            return;
        };
        if self.replaced {
            // Unlike the installer, the new AppImage does not wait for this one to exit.
            self.restart = Some(installer);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        match (state.env.run_installer)(&installer) {
            // The installer waits for the app to close, then starts it again.
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(err) => state.notices.push(format!("The update failed: {err:#}")),
        }
    }

    /// Puts back a restart whose close was cancelled, so it closes again once no job runs.
    pub fn cancel_restart(&mut self) {
        if let Some(appimage) = self.restart.take() {
            self.ready = Some(appimage);
        }
    }

    /// Starts the replaced AppImage after **Restart now** closed the window. Call it on exit,
    /// after the last writes, which the new instance reads at start.
    pub fn restart(&mut self, state: &State) {
        if let Some(appimage) = self.restart.take()
            && let Err(err) = (state.env.restart_appimage)(&appimage)
        {
            eprintln!("salvajocs: could not restart: {err:#}");
        }
    }
}

/// Makes `installer` the downloaded update, as if its download had just finished.
#[cfg(test)]
impl Updater {
    pub(crate) fn set_ready(&mut self, installer: PathBuf) {
        self.ready = Some(installer);
    }

    /// Makes `appimage` the replaced AppImage, as if the user had clicked **Restart now**.
    pub(crate) fn set_restarting(&mut self, appimage: PathBuf) {
        self.appimage = Some(appimage.clone());
        self.replaced = true;
        self.ready = Some(appimage);
    }

    /// Makes `appimage` replaced by the new release, so **Restart now** is offered.
    pub(crate) fn set_replaced(&mut self, appimage: PathBuf) {
        self.appimage = Some(appimage);
        self.replaced = true;
    }

    /// Offers `release` as if the check found it.
    pub(crate) fn offer(&mut self, release: Release, installed: bool) {
        self.release = Some(release);
        self.installed = installed;
    }

    /// Gives the updater a download that runs until the returned sender is dropped.
    pub(crate) fn hold_download(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.downloading = Some(job);
        release
    }
}

#[cfg(test)]
mod tests;
