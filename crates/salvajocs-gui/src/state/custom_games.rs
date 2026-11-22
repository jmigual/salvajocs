//! This PC's custom games (ADR-0028, ADR-0029): their edits, which stay local until a Back up
//! saves them, and the reads of the repository's into this PC's copy, one at a time.

use std::collections::BTreeMap;

use eframe::egui;
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::custom_games::{self, CustomGameError, Stored, Unsent};
use salvajocs_core::manifest::CustomGame;
use salvajocs_core::retention::Retention;

use super::{State, chain, lock_storage};
use crate::job::Job;

/// A change of one custom game: an add, an edit (a rename included) or a remove.
#[derive(Clone, Debug)]
pub struct Edit {
    /// The game as the change started from it; `None` for a new game.
    pub based_on: Option<CustomGame>,
    /// The game to save, with this PC's Wine prefix; `None` removes `based_on`.
    pub game: Option<CustomGame>,
}

/// A change of this PC's copy.
enum Update {
    /// Puts `game` in place of the local game named `at`, or at the end when there is none.
    Put { at: String, game: Box<CustomGame> },
    /// Drops the local game of this name.
    Drop(String),
}

/// The repository's custom games as a read listed them ([`custom_games::load_all`]).
type Listed = BTreeMap<String, Result<Stored, CustomGameError>>;

/// A running read, with the repository key ([`AppConfig::secret_scope`]) it was asked for and
/// the [`CustomGameJobs::sent`] count when it started.
struct Reading {
    scope: String,
    sent: u64,
    job: Job<Listed>,
}

/// The reads of the custom games in the repository.
#[derive(Default)]
pub struct CustomGameJobs {
    reading: Option<Reading>,
    /// Another read was asked for while one ran.
    again: bool,
    /// How many Back ups saved custom games to the repository: a read started before one of
    /// them ended listed the repository before that write.
    sent: u64,
    unreadable: BTreeMap<String, String>,
}

impl CustomGameJobs {
    /// Whether a read runs or waits.
    pub fn busy(&self) -> bool {
        self.reading.is_some() || self.again
    }

    /// Whether a read runs.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Why the repository's newest copy of the game `name` cannot be used, from the last read.
    pub fn unreadable(&self, name: &str) -> Option<&str> {
        self.unreadable.get(name).map(String::as_str)
    }

    /// The games of the last read whose newest copy cannot be used, with why.
    pub fn all_unreadable(&self) -> &BTreeMap<String, String> {
        &self.unreadable
    }
}

impl State {
    /// Reads the repository's custom games in the background and merges them into this PC's
    /// copy once the read ends; it writes nothing to the repository (ADR-0029). A read asked for
    /// while one runs follows it. Nothing happens without an open repository.
    pub fn read_custom_games(&mut self, ctx: &egui::Context) {
        if self.custom_games.reading.is_some() {
            self.custom_games.again = true;
            return;
        }
        let (Some(config), Some(session)) = (&self.config, &self.session) else {
            return;
        };
        let storage = session.storage.clone();
        let job = Job::spawn(ctx, move || {
            Ok(custom_games::load_all(&mut *lock_storage(&storage)?)?)
        });
        self.custom_games.reading = Some(Reading {
            scope: config.secret_scope(),
            sent: self.custom_games.sent,
            job,
        });
    }

    /// Applies `edit` to this PC's copy and marks the names it changes as unsent, so the next
    /// Back up saves them (ADR-0029), then saves the config. A rename marks both names; a change
    /// of the Wine prefix alone marks nothing, as it is never stored.
    pub fn edit_custom_game(&mut self, ctx: &egui::Context, edit: Edit) {
        let Some(config) = &mut self.config else {
            return;
        };
        let Edit { based_on, game } = edit;
        let old = based_on.as_ref().map(|game| game.name.clone());
        if based_on.as_ref().map(shared) != game.as_ref().map(shared) {
            if let Some(old) = &old {
                mark(&mut config.custom_games_unsent, old, based_on.clone());
            }
            if let Some(game) = game
                .as_ref()
                .filter(|game| Some(&game.name) != old.as_ref())
            {
                mark(&mut config.custom_games_unsent, &game.name, None);
            }
        }
        let update = match (game, old) {
            (Some(game), at) => Update::Put {
                at: at.unwrap_or_else(|| game.name.clone()),
                game: Box::new(game),
            },
            (None, Some(old)) => Update::Drop(old),
            (None, None) => return,
        };
        apply(&mut config.custom_games, update);
        self.save_config(ctx);
    }

    /// Applies a restore mapping dialog's `choices` for `title` to this PC's copy
    /// ([`crossbuild::apply_choices`]) and, when they change the game, marks it as unsent and
    /// saves the config (ADR-0029).
    pub fn apply_mapping_choices(
        &mut self,
        ctx: &egui::Context,
        title: &str,
        choices: &[(SnapshotRoot, Choice)],
    ) {
        let Some(config) = &mut self.config else {
            return;
        };
        let find = |games: &[CustomGame]| games.iter().find(|game| game.name == title).cloned();
        let before = find(&config.custom_games);
        crossbuild::apply_choices(&mut config.custom_games, title, choices);
        if find(&config.custom_games) != before {
            mark(&mut config.custom_games_unsent, title, before);
            self.save_config(ctx);
        }
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
        self.custom_games.sent += 1;
        self.save_config(ctx);
    }

    /// Merges a finished read into this PC's copy, and starts the next one asked for.
    pub(super) fn poll_custom_games(&mut self, ctx: &egui::Context) {
        let Some(reading) = &mut self.custom_games.reading else {
            return;
        };
        let Some(result) = reading.job.take_result() else {
            return;
        };
        let Some(Reading { scope, sent, .. }) = self.custom_games.reading.take() else {
            return;
        };
        let current = self.config.as_ref().map(AppConfig::secret_scope);
        if sent != self.custom_games.sent {
            // The listing is older than what a Back up wrote since.
            self.custom_games.again = true;
        } else if current.as_ref() == Some(&scope) {
            match result {
                Ok(listed) => self.merge_custom_games(ctx, &listed, scope),
                Err(err) => self.notices.push(format!(
                    "Could not read the custom games from the repository: {err:#}"
                )),
            }
        }
        // A read of a repository the app no longer uses is dropped; its successor reads anew.
        if std::mem::take(&mut self.custom_games.again) {
            self.read_custom_games(ctx);
        }
    }

    /// Merges `listed`, read from the repository of key `scope`, into this PC's copy as it is
    /// now, so an edit made while the read ran is kept.
    fn merge_custom_games(&mut self, ctx: &egui::Context, listed: &Listed, scope: String) {
        let Some(config) = &mut self.config else {
            return;
        };
        let mirrors = config.custom_games_repository.as_ref() == Some(&scope);
        let unsent = &config.custom_games_unsent;
        let merged = custom_games::merge(&config.custom_games, unsent, listed, mirrors);
        // An incomplete read leaves no marker, so the next read migrates again.
        let marker = merged.complete.then_some(scope);
        let changed = config.custom_games != merged.games
            || config.custom_games_repository != marker
            || !merged.new_unsent.is_empty();
        config.custom_games = merged.games;
        config.custom_games_repository = marker;
        config.custom_games_unsent.extend(merged.new_unsent);
        self.custom_games.unreadable = (listed.iter())
            .filter_map(|(name, stored)| {
                let why = match stored.as_ref().err()? {
                    CustomGameError::NewerFormat(_) => {
                        "Written by a newer salvajocs; it cannot be changed here.".to_owned()
                    }
                    err => format!("Cannot be read from the repository: {}", chain(err)),
                };
                Some((name.clone(), why))
            })
            .collect();
        if changed {
            self.save_config(ctx);
        }
    }
}

/// Marks `name` as unsent from `based_on`, unless it is already: the earliest base wins.
fn mark(unsent: &mut Vec<Unsent>, name: &str, based_on: Option<CustomGame>) {
    if !unsent.iter().any(|entry| entry.name == name) {
        unsent.push(Unsent::new(name.to_owned(), based_on));
    }
}

/// `game` as the repository holds it: without its Wine prefix.
fn shared(game: &CustomGame) -> CustomGame {
    let mut game = game.clone();
    game.wine_prefix = None;
    game
}

/// Puts `update` into this PC's copy `games`.
fn apply(games: &mut Vec<CustomGame>, update: Update) {
    match update {
        Update::Put { at, game } => match games.iter_mut().find(|local| local.name == at) {
            Some(local) => *local = *game,
            None => games.push(*game),
        },
        Update::Drop(name) => games.retain(|game| game.name != name),
    }
}

#[cfg(test)]
mod tests;
