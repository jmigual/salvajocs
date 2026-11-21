//! The custom games in the repository (ADR-0028): their sync into this PC's copy and their
//! saves, one job at a time, shared by the Custom games tab and the restore mapping dialogs.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use eframe::egui;
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{Choice, SnapshotRoot};
use salvajocs_core::custom_games::{self, CustomGameError};
use salvajocs_core::manifest::CustomGame;
use salvajocs_core::retention::Retention;
use salvajocs_core::storage::Storage;

use super::{State, lock_storage};
use crate::job::Job;

/// A change of one custom game: an add, an edit (a rename included) or a remove.
#[derive(Clone, Debug)]
pub struct Edit {
    /// The game as the change started from it; `None` for a new game.
    pub based_on: Option<CustomGame>,
    /// The game to save, with this PC's Wine prefix; `None` removes `based_on`.
    pub game: Option<CustomGame>,
}

/// Why a save from the Custom games form failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// The repository's copy changed since the form read it; the local copy now holds it.
    Changed,
    /// Any other failure, as a sentence.
    Failed(String),
}

enum Work {
    Sync,
    Edit(Box<Edit>),
    Choices {
        title: String,
        choices: Vec<(SnapshotRoot, Choice)>,
    },
}

/// A change of this PC's copy, applied in order once a job ends.
enum Update {
    /// Puts `game` in place of the local game named `at`, or at the end when there is none. With
    /// `keep_prefix`, the game takes that local game's Wine prefix.
    Put {
        at: String,
        game: Box<CustomGame>,
        keep_prefix: bool,
    },
    /// Drops the local game of this name.
    Drop(String),
}

/// What a sync found, its errors already put in words.
struct Synced {
    games: Vec<CustomGame>,
    /// [`custom_games::Synced::complete`].
    complete: bool,
    unreadable: BTreeMap<String, String>,
    notices: Vec<String>,
}

/// What a save did, its errors already put in words.
struct Saved {
    updates: Vec<Update>,
    notices: Vec<String>,
    error: Option<EditError>,
}

enum Done {
    Synced(Synced),
    Saved(Saved),
}

/// The custom games jobs: the one running and those waiting, each with the repository key
/// ([`AppConfig::secret_scope`]) it was asked for.
#[derive(Default)]
pub struct CustomGameJobs {
    queue: VecDeque<(String, Work)>,
    running: Option<(String, Job<Done>, Kind)>,
    edited: Option<Result<(), EditError>>,
    unreadable: BTreeMap<String, String>,
}

/// What the running job does, so a job that failed as a whole is reported where it belongs.
enum Kind {
    /// A sync; `migrates` when the copy does not mirror the repository yet, so it uploads.
    Sync {
        migrates: bool,
    },
    Form,
    /// The remove of the game of this name.
    Remove(String),
    /// A mapping dialog's choices for the game of this title.
    Choices(String),
}

impl CustomGameJobs {
    /// Whether a sync or a save runs or waits.
    pub fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    /// Whether a sync runs.
    pub fn syncing(&self) -> bool {
        matches!(self.running, Some((_, _, Kind::Sync { .. })))
    }

    /// Whether a save runs or waits.
    pub fn saving(&self) -> bool {
        matches!(&self.running, Some((_, _, kind)) if !matches!(kind, Kind::Sync { .. }))
            || (self.queue.iter()).any(|(_, work)| !matches!(work, Work::Sync))
    }

    /// Whether a save runs or waits, or a sync runs that uploads this PC's games; a sync that
    /// only reads, or has not started, is not cut short by closing the app.
    pub fn writing(&self) -> bool {
        self.saving() || matches!(self.running, Some((_, _, Kind::Sync { migrates: true })))
    }

    /// The outcome of the last save from the form, once; `None` while it runs.
    pub fn take_edited(&mut self) -> Option<Result<(), EditError>> {
        self.edited.take()
    }

    /// Why the repository's newest copy of the game `name` cannot be used, from the last sync.
    pub fn unreadable(&self, name: &str) -> Option<&str> {
        self.unreadable.get(name).map(String::as_str)
    }

    /// The games of the last sync whose newest copy cannot be used, with why.
    pub fn all_unreadable(&self) -> &BTreeMap<String, String> {
        &self.unreadable
    }
}

impl State {
    /// Loads the repository's custom games into this PC's copy in the background, uploading the
    /// local ones it lacks when the copy does not mirror this repository yet (ADR-0028).
    /// Nothing happens without an open repository or while a sync of it waits.
    pub fn sync_custom_games(&mut self, ctx: &egui::Context) {
        let scope = self.config.as_ref().map(AppConfig::secret_scope);
        let waiting = (self.custom_games.queue.iter())
            .any(|(queued, work)| matches!(work, Work::Sync) && Some(queued) == scope.as_ref());
        if !waiting {
            self.queue_custom_games(ctx, Work::Sync);
        }
    }

    /// Saves `edit` to the repository in the background, then to this PC's copy. A remove's
    /// failure is a notice; the form's outcome waits in [`CustomGameJobs::take_edited`].
    pub fn edit_custom_game(&mut self, ctx: &egui::Context, edit: Edit) {
        self.queue_custom_games(ctx, Work::Edit(Box::new(edit)));
    }

    /// Saves a restore mapping dialog's `choices` for `title` on the repository's newest copy of
    /// the game in the background, or on this PC's copy when the repository has none, then puts
    /// the result in this PC's.
    pub fn save_mapping_choices(
        &mut self,
        ctx: &egui::Context,
        title: String,
        choices: Vec<(SnapshotRoot, Choice)>,
    ) {
        self.queue_custom_games(ctx, Work::Choices { title, choices });
    }

    /// Takes what a Back up saved to the repository open now: the custom games `sent`
    /// ([`custom_games::Sent::sent`]) and the Old backups rules `rules`, when it sent them. A
    /// change still as sent is no longer unsent; one changed during the backup is based on what
    /// was sent (ADR-0029). Saves the config.
    pub fn custom_games_sent(
        &mut self,
        ctx: &egui::Context,
        sent: &[(String, Option<CustomGame>)],
        rules: Option<Retention>,
    ) {
        let Some(config) = &mut self.config else {
            return;
        };
        custom_games::after_send(&mut config.custom_games_unsent, &config.custom_games, sent);
        if let (Some(rules), Some(unsent)) = (rules, &mut config.retention_unsent) {
            if unsent.policy == rules {
                config.retention_unsent = None;
            } else {
                unsent.based_on = rules;
            }
        }
        self.save_config(ctx);
    }

    fn queue_custom_games(&mut self, ctx: &egui::Context, work: Work) {
        let Some(config) = &self.config else {
            return;
        };
        let scope = config.secret_scope();
        self.custom_games.queue.push_back((scope, work));
        self.start_custom_games(ctx);
    }

    /// Starts the next waiting job when none runs. Work asked for another repository than the
    /// one open now is dropped.
    fn start_custom_games(&mut self, ctx: &egui::Context) {
        while self.custom_games.running.is_none() {
            let Some((scope, work)) = self.custom_games.queue.pop_front() else {
                return;
            };
            let (Some(config), Some(session)) = (&self.config, &self.session) else {
                self.drop_custom_games_work(&work, "the repository is not open");
                continue;
            };
            if config.secret_scope() != scope {
                self.drop_custom_games_work(&work, "another repository was connected");
                continue;
            }
            let storage = session.storage.clone();
            let stage_root = self.env.cache_dir.join("stage");
            let (kind, job) = match work {
                Work::Sync => {
                    let local = config.custom_games.clone();
                    let mirrors = config.custom_games_repository.as_ref() == Some(&scope);
                    let config_dir = self.env.config_dir.clone();
                    let job = Job::spawn(ctx, move || {
                        let mut storage = lock_storage(&storage)?;
                        sync(&mut storage, &local, mirrors, &stage_root, &config_dir)
                    });
                    (Kind::Sync { migrates: !mirrors }, job)
                }
                Work::Edit(edit) => {
                    let kind = match (&edit.game, &edit.based_on) {
                        (None, Some(game)) => Kind::Remove(game.name.clone()),
                        (Some(_), _) | (None, None) => Kind::Form,
                    };
                    let job = Job::spawn(ctx, move || {
                        let mut storage = lock_storage(&storage)?;
                        Ok(Done::Saved(save_edit(&mut storage, *edit, &stage_root)))
                    });
                    (kind, job)
                }
                Work::Choices { title, choices } => {
                    let kind = Kind::Choices(title.clone());
                    let local = (config.custom_games.iter())
                        .find(|game| game.name == title)
                        .cloned();
                    let job = Job::spawn(ctx, move || {
                        let mut storage = lock_storage(&storage)?;
                        Ok(Done::Saved(save_choices(
                            &mut storage,
                            &title,
                            local.as_ref(),
                            &choices,
                            &stage_root,
                        )))
                    });
                    (kind, job)
                }
            };
            self.custom_games.running = Some((scope, job, kind));
        }
    }

    fn drop_custom_games_work(&mut self, work: &Work, why: &str) {
        match work {
            Work::Sync => {}
            Work::Edit(edit) => match (&edit.game, &edit.based_on) {
                (Some(_), _) => {
                    self.custom_games.edited = Some(Err(EditError::Failed(format!(
                        "The game was not saved: {why}."
                    ))));
                }
                (None, Some(game)) => self
                    .notices
                    .push(format!("{} was not removed: {why}.", game.name)),
                (None, None) => {}
            },
            Work::Choices { title, .. } => self.notices.push(format!(
                "The folder choices for {title} were not saved to the repository: {why}."
            )),
        }
    }

    /// Applies a finished custom games job and starts the next.
    pub(super) fn poll_custom_games(&mut self, ctx: &egui::Context) {
        let Some((_, job, _)) = &mut self.custom_games.running else {
            return;
        };
        let Some(result) = job.take_result() else {
            return;
        };
        let Some((scope, _, kind)) = self.custom_games.running.take() else {
            return;
        };
        let current = self.config.as_ref().map(AppConfig::secret_scope);
        if current.as_ref() != Some(&scope) {
            // The job ran on a repository the app no longer uses; the sync of the new one
            // brings the games, and this PC's copy goes along without the job's change.
            match kind {
                Kind::Form => {
                    self.custom_games.edited = Some(Err(EditError::Failed(
                        "Another repository was connected while the game was saved.".to_owned(),
                    )));
                }
                Kind::Remove(name) => self.notices.push(format!(
                    "{name} was not removed here: another repository was connected while it \
                     was removed. Remove it again if you do not want it."
                )),
                Kind::Choices(title) => self.notices.push(format!(
                    "The folder choices for {title} went to the repository used before, so the \
                     one now in use may not have them."
                )),
                Kind::Sync { .. } => {}
            }
        } else {
            match result {
                Ok(Done::Synced(synced)) => self.apply_synced(ctx, synced, scope),
                Ok(Done::Saved(saved)) => self.apply_saved(ctx, saved, kind),
                Err(err) => self.custom_games_failed(kind, &format!("{err:#}")),
            }
        }
        self.start_custom_games(ctx);
    }

    fn apply_synced(&mut self, ctx: &egui::Context, synced: Synced, scope: String) {
        let Some(config) = &mut self.config else {
            return;
        };
        // An incomplete sync leaves no marker, so the next sync migrates again.
        let marker = synced.complete.then_some(scope);
        let changed =
            config.custom_games != synced.games || config.custom_games_repository != marker;
        config.custom_games = synced.games;
        config.custom_games_repository = marker;
        self.custom_games.unreadable = synced.unreadable;
        self.notices.extend(synced.notices);
        if changed {
            self.save_config(ctx);
        }
    }

    fn apply_saved(&mut self, ctx: &egui::Context, saved: Saved, kind: Kind) {
        let Some(config) = &mut self.config else {
            return;
        };
        // The mapping dialogs change the copy in memory first, so an update can find it equal.
        let changed = !saved.updates.is_empty();
        for update in saved.updates {
            apply(&mut config.custom_games, update);
        }
        self.notices.extend(saved.notices);
        match (kind, saved.error) {
            (Kind::Form, error) => self.custom_games.edited = Some(error.map_or(Ok(()), Err)),
            (_, Some(EditError::Failed(message))) => self.notices.push(message),
            (_, Some(EditError::Changed)) => self.notices.push(
                "The game changed on another PC in the meantime; this PC now shows that version."
                    .to_owned(),
            ),
            (_, None) => {}
        }
        if changed {
            self.save_config(ctx);
        }
    }

    fn custom_games_failed(&mut self, kind: Kind, message: &str) {
        match kind {
            Kind::Sync { .. } => self.notices.push(format!(
                "Could not load the custom games from the repository: {message}"
            )),
            Kind::Form => {
                self.custom_games.edited = Some(Err(EditError::Failed(format!(
                    "Could not save the game to the repository: {message}"
                ))));
            }
            Kind::Remove(_) => self
                .notices
                .push(format!("Could not remove the custom game: {message}")),
            Kind::Choices(_) => self.notices.push(format!(
                "Could not save the folder choices to the repository: {message}"
            )),
        }
    }
}

/// Puts `update` into this PC's copy `games`.
fn apply(games: &mut Vec<CustomGame>, update: Update) {
    match update {
        Update::Put {
            at,
            mut game,
            keep_prefix,
        } => match games.iter_mut().find(|local| local.name == at) {
            Some(local) => {
                if keep_prefix {
                    game.wine_prefix = local.wine_prefix.take();
                }
                *local = *game;
            }
            None => games.push(*game),
        },
        Update::Drop(name) => games.retain(|game| game.name != name),
    }
}

/// An error and its causes in one line.
fn describe(err: CustomGameError) -> String {
    format!("{:#}", anyhow::Error::new(err))
}

/// [`custom_games::sync`], keeping a copy of the config before it first replaces a local game.
fn sync(
    storage: &mut Storage,
    local: &[CustomGame],
    mirrors: bool,
    stage_root: &Path,
    config_dir: &Path,
) -> anyhow::Result<Done> {
    let synced = custom_games::sync(storage, local, mirrors, stage_root)?;
    if !synced.replaced.is_empty() || !synced.removed.is_empty() {
        AppConfig::keep_before_custom_games(config_dir)?;
    }
    let mut notices = Vec::new();
    if !synced.uploaded.is_empty() {
        // A count, not the names: a name in the banner reads like the game's own row.
        notices.push(format!(
            "Saved {} of this PC's custom games to the repository, so every PC that uses it \
             sees them.",
            synced.uploaded.len()
        ));
    }
    if !synced.replaced.is_empty() {
        notices.push(format!(
            "This PC's copy of {} differed from the repository's; the repository's is used. The \
             config from before the first time this happened is kept as {} in the config \
             folder.",
            synced.replaced.join(", "),
            salvajocs_core::config::BEFORE_CUSTOM_GAMES_FILE,
        ));
    }
    if !synced.removed.is_empty() {
        notices.push(format!(
            "Removed {} of this PC's custom games that were removed on another PC: {}. The \
             config from before the first time this happened is kept as {} in the config \
             folder.",
            synced.removed.len(),
            synced.removed.join(", "),
            salvajocs_core::config::BEFORE_CUSTOM_GAMES_FILE,
        ));
    }
    for (name, err) in synced.failed {
        notices.push(format!(
            "Could not save the custom game {name} to the repository ({}); it is tried again \
             next time.",
            describe(err)
        ));
    }
    for (name, err) in synced.forget_errors {
        notices.push(format!(
            "The custom game {name} was saved, but its older copies could not be removed: {err}"
        ));
    }
    let unreadable = (synced.unreadable.into_iter())
        .map(|(name, err)| {
            let why = match err {
                CustomGameError::NewerFormat(_) => {
                    "Written by a newer salvajocs; it cannot be changed here.".to_owned()
                }
                err => format!("Cannot be read from the repository: {}", describe(err)),
            };
            (name, why)
        })
        .collect();
    Ok(Done::Synced(Synced {
        games: synced.games,
        complete: synced.complete,
        unreadable,
        notices,
    }))
}

/// One save of `edit`: a rename writes the new name, then removes the old one.
fn save_edit(storage: &mut Storage, edit: Edit, stage_root: &Path) -> Saved {
    let mut saved = Saved {
        updates: Vec::new(),
        notices: Vec::new(),
        error: None,
    };
    let Edit { based_on, game } = edit;
    let old = based_on.as_ref().map(|game| game.name.clone());
    let renamed = matches!((&old, &game), (Some(old), Some(game)) if *old != game.name);
    if let Some(game) = &game {
        let based_on = based_on.as_ref().filter(|_| !renamed);
        match custom_games::save(storage, &game.name, based_on, Some(game), stage_root) {
            Ok(done) => note_forget(&mut saved, &game.name, done.forget_error),
            Err(err) => return failed(saved, &game.name, err),
        }
        if !renamed {
            saved.updates.push(Update::Put {
                at: game.name.clone(),
                game: Box::new(game.clone()),
                keep_prefix: false,
            });
            return saved;
        }
    }
    let (Some(old), Some(based_on)) = (old, &based_on) else {
        return saved;
    };
    match custom_games::save(storage, &old, Some(based_on), None, stage_root) {
        Ok(done) => {
            note_forget(&mut saved, &old, done.forget_error);
            saved.updates.push(match game {
                Some(game) => Update::Put {
                    at: old,
                    game: Box::new(game),
                    keep_prefix: false,
                },
                None => Update::Drop(old),
            });
            saved
        }
        Err(err) => {
            if let Some(game) = game {
                // The new name is written: both names are in the repository now.
                saved.updates.push(Update::Put {
                    at: game.name.clone(),
                    game: Box::new(game.clone()),
                    keep_prefix: false,
                });
                saved.error = Some(EditError::Failed(format!(
                    "The game was saved as {}, but {old} could not be removed ({}). Both are \
                     listed; remove the one you do not want.",
                    game.name,
                    describe(err)
                )));
                return saved;
            }
            failed(saved, &old, err)
        }
    }
}

/// [`custom_games::save_choices`] for the restore mapping dialogs, with this PC's copy `local`.
fn save_choices(
    storage: &mut Storage,
    title: &str,
    local: Option<&CustomGame>,
    choices: &[(SnapshotRoot, Choice)],
    stage_root: &Path,
) -> Saved {
    let saved = Saved {
        updates: Vec::new(),
        notices: Vec::new(),
        error: None,
    };
    match custom_games::save_choices(storage, title, local, choices, stage_root) {
        Ok(done) => {
            let mut saved = saved;
            note_forget(&mut saved, title, done.forget_error);
            if let Some(game) = done.game {
                saved.updates.push(Update::Put {
                    at: title.to_owned(),
                    game: Box::new(game),
                    keep_prefix: true,
                });
            }
            saved
        }
        Err(err) => {
            let mut saved = saved;
            saved.error = Some(EditError::Failed(format!(
                "Could not save the folder choices for {title} to the repository: {}",
                describe(err)
            )));
            saved
        }
    }
}

fn note_forget(
    saved: &mut Saved,
    name: &str,
    error: Option<salvajocs_core::storage::StorageError>,
) {
    if let Some(err) = error {
        saved.notices.push(format!(
            "The custom game {name} was saved, but its older copies could not be removed: {err}"
        ));
    }
}

/// `saved` with the failure of the save of `name`. A conflict brings the repository's copy into
/// this PC's, so a reload of the form shows it.
fn failed(mut saved: Saved, name: &str, err: CustomGameError) -> Saved {
    saved.error = Some(match err {
        CustomGameError::Changed(newest) => {
            saved.updates.push(match newest {
                Some(game) => Update::Put {
                    at: name.to_owned(),
                    game,
                    keep_prefix: true,
                },
                None => Update::Drop(name.to_owned()),
            });
            EditError::Changed
        }
        CustomGameError::NewerFormat(_) => EditError::Failed(format!(
            "{name} was written by a newer salvajocs and cannot be changed here."
        )),
        err => EditError::Failed(format!(
            "Could not save {name} to the repository: {}",
            describe(err)
        )),
    });
    saved
}

#[cfg(test)]
mod tests;
