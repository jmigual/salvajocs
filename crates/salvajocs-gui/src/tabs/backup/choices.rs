//! What to back up of each game (ADR-0014): the repository's settings, loaded in a job, with
//! this machine's unsaved changes replayed on them. Each change goes to `pending-selection.json`
//! at once; the next backup saves it to the repository.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use std::{mem, ptr, slice, thread};

use eframe::egui::{self, Ui};
use salvajocs_core::selection::{self, Change, GameSettings, Pending};
use salvajocs_core::storage::Storage;

use super::chain;
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::theme::{self, icons};

/// Each game's settings by title, or why they cannot be read.
pub(super) type Games = BTreeMap<String, Result<GameSettings, String>>;

/// Reads every game's settings from the repository; [`selection::load_all`] reindexes first.
pub(super) fn load(storage: &SharedStorage) -> anyhow::Result<Games> {
    let games = selection::load_all(&mut *lock_storage(storage)?)?;
    Ok(games
        .into_iter()
        .map(|(title, settings)| (title, settings.map_err(|err| chain(&err))))
        .collect())
}

/// `games` with the changes `pending` holds for the repository `scope`
/// (`AppConfig::secret_scope`) replayed on them. A game whose settings cannot be read keeps its
/// error.
pub(super) fn replay(mut games: Games, pending: Option<&Pending>, scope: &str) -> Games {
    let changes = pending.and_then(|pending| pending.repositories.get(scope));
    for (title, changes) in changes.into_iter().flatten() {
        let settings = games
            .entry(title.clone())
            .or_insert_with(|| Ok(GameSettings::default()));
        if let Ok(settings) = settings {
            settings.apply_changes(changes);
        }
    }
    games
}

/// What a load job hands back.
struct Loaded {
    /// The repository's settings.
    games: Games,
    /// The pending file, when this load read it.
    pending: Option<Result<Pending, String>>,
}

/// What a backup did to the selection, for [`Choices::backed_up`].
pub(super) struct AfterBackup {
    /// The repository the backup ran on.
    pub(super) source: Weak<Mutex<Storage>>,
    /// The pending changes left once the saved ones were dropped; `None` when the pending file
    /// could not be read.
    pub(super) pending: Option<Pending>,
    /// How writing the pending file went, when the backup wrote it.
    pub(super) written: Option<Result<(), String>>,
    /// The settings read again after saving, with the pending changes replayed; `None` when
    /// they could not be read.
    pub(super) games: Option<Games>,
}

/// The backup selection of the open repository.
#[derive(Default)]
pub(super) struct Choices {
    /// The repository the settings are loaded, or loading, for; `None` while none is open.
    source: Option<Weak<Mutex<Storage>>>,
    loading: Option<Job<Loaded>>,
    load_error: Option<String>,
    /// The repository's settings with the pending changes replayed; `None` until loaded.
    games: Option<Games>,
    /// Every repository's unsaved changes. Read once, then the file follows it, so a write still
    /// running is never undone by a reread. A file that could not be read is never written.
    pending: Option<Result<Pending, String>>,
    writing: Option<Job<()>>,
    write_again: bool,
    write_error: Option<String>,
}

impl Choices {
    /// Collects finished jobs, and loads the settings when the open repository is not the one
    /// they were loaded for. Returns whether the settings changed.
    pub(super) fn poll(&mut self, ctx: &egui::Context, state: &State) -> bool {
        let mut changed = false;
        if let Some(job) = &mut self.loading
            && let Some(result) = job.take_result()
        {
            self.loading = None;
            // A load for a repository no longer open is dropped; a new one starts below.
            if self.is_current(state) {
                match result {
                    Ok(loaded) => {
                        self.adopt(loaded, state);
                        changed = true;
                    }
                    Err(err) => self.load_error = Some(format!("{err:#}")),
                }
            }
        }
        if let Some(job) = &mut self.writing
            && let Some(result) = job.take_result()
        {
            self.writing = None;
            self.write_error = result.err().map(|err| format!("{err:#}"));
            if mem::take(&mut self.write_again) {
                self.write(ctx, state);
            }
        }
        if !self.is_current(state) && self.loading.is_none() {
            self.load(ctx, state);
            changed = true;
        }
        changed
    }

    /// Whether the settings are for the repository open now, or for none when none is.
    fn is_current(&self, state: &State) -> bool {
        match (&self.source, &state.session) {
            (None, None) => true,
            (Some(source), Some(session)) => {
                ptr::eq(source.as_ptr(), Arc::as_ptr(&session.storage))
            }
            _ => false,
        }
    }

    /// Loads the open repository's settings in a job, and the pending file unless it was read.
    fn load(&mut self, ctx: &egui::Context, state: &State) {
        self.games = None;
        self.load_error = None;
        self.source = state
            .session
            .as_ref()
            .map(|session| Arc::downgrade(&session.storage));
        let Some(session) = &state.session else {
            return;
        };
        let storage = session.storage.clone();
        let dir = (!matches!(self.pending, Some(Ok(_)))).then(|| state.env.config_dir.clone());
        self.loading = Some(Job::spawn(ctx, move || {
            let pending = dir.map(|dir| Pending::load(&dir).map_err(|err| chain(&err)));
            Ok(Loaded {
                games: load(&storage)?,
                pending,
            })
        }));
    }

    fn adopt(&mut self, loaded: Loaded, state: &State) {
        if let Some(pending) = loaded.pending {
            self.pending = Some(pending);
        }
        let Some(config) = &state.config else {
            return;
        };
        let pending = self
            .pending
            .as_ref()
            .and_then(|pending| pending.as_ref().ok());
        self.games = Some(replay(loaded.games, pending, &config.secret_scope()));
    }

    /// The settings of the game `title`, all included until loaded, or why they cannot be read.
    pub(super) fn game(&self, title: &str) -> Result<Cow<'_, GameSettings>, &str> {
        match self.games.as_ref().and_then(|games| games.get(title)) {
            Some(Ok(settings)) => Ok(Cow::Borrowed(settings)),
            Some(Err(error)) => Err(error),
            None => Ok(Cow::Owned(GameSettings::default())),
        }
    }

    /// Whether the settings are loaded.
    pub(super) fn loaded(&self) -> bool {
        self.games.is_some()
    }

    /// Whether the settings are loaded and a change can be kept in the pending file.
    pub(super) fn editable(&self) -> bool {
        self.loaded() && matches!(self.pending, Some(Ok(_)))
    }

    /// Whether a load or a pending file write runs or waits.
    pub(super) fn busy(&self) -> bool {
        self.loading.is_some() || self.writing.is_some() || self.write_again
    }

    /// Applies `change` to the game `title` and writes it to the pending file in the background.
    /// Does nothing unless [`Choices::editable`], or when the game's settings cannot be read.
    pub(super) fn change(
        &mut self,
        ctx: &egui::Context,
        state: &State,
        title: &str,
        change: Change,
    ) {
        let (Some(games), Some(Ok(pending)), Some(config)) =
            (&mut self.games, &mut self.pending, &state.config)
        else {
            return;
        };
        match games
            .entry(title.to_owned())
            .or_insert_with(|| Ok(GameSettings::default()))
        {
            Ok(settings) => settings.apply_changes(slice::from_ref(&change)),
            Err(_) => return,
        }
        pending
            .games_mut(config)
            .entry(title.to_owned())
            .or_default()
            .push(change);
        self.write(ctx, state);
    }

    /// Writes the pending file in the background. A write already running is followed by
    /// another, so the file ends up with the latest changes.
    fn write(&mut self, ctx: &egui::Context, state: &State) {
        if self.writing.is_some() {
            self.write_again = true;
            return;
        }
        let Some(Ok(pending)) = &self.pending else {
            return;
        };
        let (pending, dir) = (pending.clone(), state.env.config_dir.clone());
        self.writing = Some(Job::spawn(ctx, move || Ok(pending.save(&dir)?)));
    }

    /// Finishes the pending file writes before the app closes: waits up to 5 s for the running
    /// one, then makes a queued one in `config_dir` at once.
    ///
    /// # Errors
    /// When the running write does not end in time, leaving the queued changes unwritten, or
    /// the queued write fails.
    pub(super) fn flush(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        if let Some(job) = &mut self.writing {
            let deadline = Instant::now() + Duration::from_secs(5);
            // ponytail: polls a write of one small file, and only on exit.
            while job.is_running() {
                // A second writer would race the stuck one on the same temp file.
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "the pending file write did not end within 5 s; the latest changes are lost"
                );
                thread::sleep(Duration::from_millis(5));
            }
            self.writing = None;
        }
        if mem::take(&mut self.write_again)
            && let Some(Ok(pending)) = &self.pending
        {
            pending.save(config_dir)?;
        }
        Ok(())
    }

    /// What a backup saves and filters by: the pending changes, when the file could be read,
    /// and every game's settings. `None` until loaded.
    pub(super) fn for_backup(&self) -> Option<(Option<Pending>, Games)> {
        let games = self.games.clone()?;
        let pending = match &self.pending {
            Some(Ok(pending)) => Some(pending.clone()),
            _ => None,
        };
        Some((pending, games))
    }

    /// Takes what a backup left: the pending changes it did not save and, when the backup ran
    /// on the repository still open, the settings it read again. Returns whether the settings
    /// changed.
    pub(super) fn backed_up(&mut self, after: AfterBackup) -> bool {
        if let Some(pending) = after.pending {
            self.pending = Some(Ok(pending));
        }
        if let Some(written) = after.written {
            self.write_error = written.err();
        }
        let same = self
            .source
            .as_ref()
            .is_some_and(|source| ptr::eq(source.as_ptr(), after.source.as_ptr()));
        match after.games {
            Some(games) if same && self.loading.is_none() => {
                self.games = Some(games);
                true
            }
            _ => false,
        }
    }

    /// Why the checkboxes are off, and what failed.
    pub(super) fn status_ui(&mut self, ui: &mut Ui, state: &State) {
        let error = ui.visuals().error_fg_color;
        let mut retry = false;
        if state.session.is_none() {
            ui.weak("Open the repository to choose what to back up.");
        } else if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading what to back up");
            });
        } else {
            if let Some(load_error) = &self.load_error {
                let text = format!("What to back up could not be loaded: {load_error}");
                retry |= error_with_retry(ui, error, text);
            }
            if let Some(Err(pending_error)) = &self.pending {
                let text = format!(
                    "pending-selection.json in the config folder cannot be read, so no choice \
                     can be made: {pending_error}"
                );
                retry |= error_with_retry(ui, error, text);
            }
        }
        if retry {
            // The next poll loads again, the pending file included.
            self.source = None;
            ui.ctx().request_repaint();
        }
        if let Some(write_error) = &self.write_error {
            let warn = ui.visuals().warn_fg_color;
            theme::status(
                ui,
                icons::WARNING,
                warn,
                format!("The unsaved choices could not be written: {write_error}"),
            );
        }
    }
}

/// An error line with a Retry button; returns whether it was clicked.
fn error_with_retry(ui: &mut Ui, color: egui::Color32, text: String) -> bool {
    ui.horizontal(|ui| {
        theme::status(ui, icons::X_CIRCLE, color, text);
        ui.button("Retry").clicked()
    })
    .inner
}

/// Test hooks that hold a job of [`Choices`] until the returned sender is dropped.
#[cfg(test)]
impl Choices {
    pub(super) fn hold_write(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.writing = Some(job);
        release
    }

    pub(super) fn hold_load(
        &mut self,
        ctx: &egui::Context,
        storage: &SharedStorage,
    ) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.source = Some(Arc::downgrade(storage));
        self.games = None;
        self.loading = Some(job);
        release
    }
}
