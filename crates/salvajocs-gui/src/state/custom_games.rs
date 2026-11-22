//! This PC's custom games (ADR-0028, ADR-0029): their edits, which stay local until a Back up
//! saves them, and the reads of the repository's into this PC's copy, one at a time.

use std::collections::BTreeMap;

use eframe::egui;
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::custom_games::{self, CustomGameError, NotNow, Stored, Unsent};
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
    /// The repository's games of the last read that this PC uses only once accepted, by name
    /// ([`custom_games::Merged::offered`]).
    offered: BTreeMap<String, CustomGame>,
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

    /// The repository's copy of the game `name`, without a Wine prefix, when the last read found
    /// it new here or different from this PC's and the user has not answered it (ADR-0030).
    pub fn offered(&self, name: &str) -> Option<&CustomGame> {
        self.offered.get(name)
    }

    /// Every game [`CustomGameJobs::offered`] holds, by name.
    pub fn all_offered(&self) -> &BTreeMap<String, CustomGame> {
        &self.offered
    }

    /// The offers the Custom games tab lists, by name: those whose name has no unsent change in
    /// `config`, set aside ([`AppConfig::set_aside`]) or not.
    pub fn listed<'a>(&'a self, config: &'a AppConfig) -> impl Iterator<Item = &'a CustomGame> {
        let unsent =
            |name: &str| (config.custom_games_unsent.iter()).any(|entry| entry.name == name);
        (self.offered.values()).filter(move |game| !unsent(&game.name))
    }

    /// How many listed offers wait for an answer: those not set aside.
    pub fn waiting(&self, config: &AppConfig) -> usize {
        self.listed(config)
            .filter(|game| !config.set_aside(game))
            .count()
    }

    /// Forgets what the last read found, offers and unreadable games, for a repository that is
    /// no longer the one open.
    pub(super) fn forget_read(&mut self) {
        self.offered.clear();
        self.unreadable.clear();
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
                mark(
                    &mut config.custom_games_unsent,
                    &mut self.custom_games.offered,
                    old,
                    based_on.clone(),
                );
            }
            if let Some(game) = game
                .as_ref()
                .filter(|game| Some(&game.name) != old.as_ref())
            {
                mark(
                    &mut config.custom_games_unsent,
                    &mut self.custom_games.offered,
                    &game.name,
                    None,
                );
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
            mark(
                &mut config.custom_games_unsent,
                &mut self.custom_games.offered,
                title,
                before,
            );
            self.save_config(ctx);
        }
    }

    /// Uses the offered copy `shown`, as the user saw it, on this PC with this PC's Wine prefix,
    /// and saves the config (ADR-0030). Nothing is unsent: the copy is the repository's. Nothing
    /// happens when the offer of its name is gone or is no longer `shown`, as a read that ended
    /// since may have replaced it.
    pub fn accept_custom_game(&mut self, ctx: &egui::Context, shown: &CustomGame) {
        let Some(config) = &mut self.config else {
            return;
        };
        let Some(mut game) = take_offer(&mut self.custom_games.offered, shown) else {
            return;
        };
        let name = &shown.name;
        if let Some(not_now) = &mut config.custom_games_not_now {
            not_now.games.retain(|game| game.name != *name);
            if not_now.games.is_empty() {
                config.custom_games_not_now = None;
            }
        }
        let local = config.custom_games.iter().find(|game| game.name == *name);
        game.wine_prefix = local.and_then(|game| game.wine_prefix.clone());
        let at = name.clone();
        apply(
            &mut config.custom_games,
            Update::Put {
                at,
                game: Box::new(game),
            },
        );
        self.save_config(ctx);
    }

    /// Sets the offered copy `shown`, as the user saw it, aside with **Not now** (ADR-0030): it
    /// stays offered, listed apart and without a notice, until the repository holds another copy
    /// of its name. Writes nothing to the repository; saves the config. Nothing happens when the
    /// offer of its name is gone or is no longer `shown`.
    pub fn set_custom_game_aside(&mut self, ctx: &egui::Context, shown: &CustomGame) {
        let Some(config) = &mut self.config else {
            return;
        };
        if self.custom_games.offered.get(&shown.name) != Some(shown) {
            return;
        }
        let scope = config.secret_scope();
        let not_now = (config.custom_games_not_now)
            .take()
            .filter(|not_now| not_now.repository == scope);
        let mut not_now = not_now.unwrap_or_else(|| NotNow::new(scope));
        not_now.games.retain(|game| game.name != shown.name);
        not_now.games.push(shown.clone());
        config.custom_games_not_now = Some(not_now);
        self.save_config(ctx);
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
    /// now, so an edit made while the read ran is kept. Its offers replace the last read's, and
    /// those not offered before and not set aside get one notice. A copy set aside from this
    /// repository stays so while this read offered it again or could not read its name.
    fn merge_custom_games(&mut self, ctx: &egui::Context, listed: &Listed, scope: String) {
        let Some(config) = &mut self.config else {
            return;
        };
        let mirrors = config.custom_games_repository.as_ref() == Some(&scope);
        let unsent = &config.custom_games_unsent;
        let merged = custom_games::merge(&config.custom_games, unsent, listed, mirrors);
        // Another repository's copies set nothing aside here; they wait for a read of theirs.
        let not_now = match config.custom_games_not_now.clone() {
            Some(mut not_now) if not_now.repository == scope => {
                not_now.games.retain(|game| {
                    merged.offered.contains(game) || matches!(listed.get(&game.name), Some(Err(_)))
                });
                Some(not_now).filter(|not_now| !not_now.games.is_empty())
            }
            other => other,
        };
        // An incomplete read leaves no marker, so the next read migrates again.
        let marker = merged.complete.then_some(scope);
        let changed = config.custom_games != merged.games
            || config.custom_games_repository != marker
            || !merged.new_unsent.is_empty()
            || config.custom_games_not_now != not_now;
        config.custom_games = merged.games;
        config.custom_games_repository = marker;
        config.custom_games_unsent.extend(merged.new_unsent);
        config.custom_games_not_now = not_now;
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
        let new: Vec<&str> = (merged.offered.iter())
            .filter(|game| self.custom_games.offered.get(&game.name) != Some(*game))
            .filter(|game| !config.set_aside(game))
            .map(|game| game.name.as_str())
            .collect();
        if !new.is_empty() {
            self.notices.push(format!(
                "New or changed custom games from other PCs wait for review in Custom games: {}",
                new.join(", ")
            ));
        }
        self.custom_games.offered = (merged.offered.into_iter())
            .map(|game| (game.name.clone(), game))
            .collect();
        if changed {
            self.save_config(ctx);
        }
    }
}

/// Marks `name` as unsent from `based_on`, unless it is already: the earliest base wins. Drops
/// its offer from `offered`, as an unsent name is never offered.
fn mark(
    unsent: &mut Vec<Unsent>,
    offered: &mut BTreeMap<String, CustomGame>,
    name: &str,
    based_on: Option<CustomGame>,
) {
    offered.remove(name);
    if !unsent.iter().any(|entry| entry.name == name) {
        unsent.push(Unsent::new(name.to_owned(), based_on));
    }
}

/// Removes and returns the offer of `shown`'s name when it is still `shown`.
fn take_offer(
    offered: &mut BTreeMap<String, CustomGame>,
    shown: &CustomGame,
) -> Option<CustomGame> {
    if offered.get(&shown.name) != Some(shown) {
        return None;
    }
    offered.remove(&shown.name)
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
