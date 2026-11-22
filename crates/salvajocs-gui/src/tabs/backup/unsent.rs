//! This PC's custom games and Old backups rules not yet saved to the repository, which a
//! backup saves first, asking about the ones another PC changed since (ADR-0029).

use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, Weak};

use eframe::egui::{RichText, Ui};
use salvajocs_core::config::AppConfig;
use salvajocs_core::custom_games::{self, Conflict, CustomGameError};
use salvajocs_core::manifest::{CustomGame, Integration};
use salvajocs_core::retention::{self, Retention, RetentionError, Unit};
use salvajocs_core::storage::Storage;

use super::chain;
use crate::state::{SharedStorage, State, lock_storage};
use crate::theme;

/// What a backup saved of this PC's unsent custom games and rules (ADR-0029).
#[derive(Default)]
pub(super) struct Shared {
    /// The custom games the repository now holds as sent ([`custom_games::Sent::sent`]).
    pub(super) games: Vec<(String, Option<CustomGame>)>,
    /// How many custom games were written.
    pub(super) written: usize,
    /// The rules the repository now holds as sent; `None` when none were sent.
    pub(super) rules: Option<Retention>,
    /// The rules were to be sent and could not be.
    pub(super) rules_failed: bool,
    /// What was not saved, as sentences; it stays unsent.
    pub(super) errors: Vec<String>,
    /// Problems that lost no change.
    pub(super) warnings: Vec<String>,
}

/// What the check before a backup found changed on another PC since this PC's unsent changes
/// started (ADR-0029).
pub(super) struct Checked {
    pub(super) games: Vec<Conflict>,
    /// The repository's rules, when they conflict with this PC's unsent ones.
    pub(super) rules: Option<Retention>,
}

/// Which version of a conflict the user keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keep {
    Here,
    Repository,
}

/// The question of which version to keep of each change made on another PC, before a backup.
pub(super) struct Asking {
    /// The repository that was checked.
    pub(super) source: Weak<Mutex<Storage>>,
    pub(super) games: Vec<(Conflict, Option<Keep>)>,
    /// The repository's rules and the choice.
    pub(super) rules: Option<(Retention, Option<Keep>)>,
}

impl Asking {
    /// Whether every conflict has a choice.
    pub(super) fn answered(&self) -> bool {
        self.games.iter().all(|(_, keep)| keep.is_some())
            && (self.rules.iter()).all(|(_, keep)| keep.is_some())
    }

    /// Puts the choices into `config`. Keeping this PC's version bases its unsent change on the
    /// repository's, so the backup writes it; keeping the repository's puts that version here
    /// and drops the change.
    pub(super) fn apply(self, config: &mut AppConfig) {
        for (conflict, keep) in self.games {
            let Some(index) =
                (config.custom_games_unsent.iter()).position(|entry| entry.name == conflict.name)
            else {
                continue;
            };
            match keep {
                Some(Keep::Here) => config.custom_games_unsent[index].based_on = conflict.newest,
                Some(Keep::Repository) => {
                    config.custom_games_unsent.remove(index);
                    let games = &mut config.custom_games;
                    let at = games.iter().position(|game| game.name == conflict.name);
                    match (conflict.newest, at) {
                        (Some(mut newest), Some(at)) => {
                            newest.wine_prefix = games[at].wine_prefix.take();
                            games[at] = newest;
                        }
                        (Some(newest), None) => games.push(newest),
                        (None, Some(at)) => {
                            games.remove(at);
                        }
                        (None, None) => {}
                    }
                }
                None => {}
            }
        }
        if let Some((stored, keep)) = self.rules {
            match keep {
                Some(Keep::Here) => {
                    if let Some(unsent) = &mut config.retention_unsent {
                        unsent.based_on = stored;
                    }
                }
                Some(Keep::Repository) => config.retention_unsent = None,
                None => {}
            }
        }
    }
}

/// Whether `config` holds custom games or rules changed here and not yet saved to the
/// repository (ADR-0029).
pub(super) fn has_unsent(config: Option<&AppConfig>) -> bool {
    config.is_some_and(|config| {
        !config.custom_games_unsent.is_empty() || config.retention_unsent.is_some()
    })
}

/// Whether `source` is the repository open now.
pub(super) fn is_session(source: &Weak<Mutex<Storage>>, state: &State) -> bool {
    (state.session.as_ref())
        .is_some_and(|session| ptr::eq(source.as_ptr(), Arc::as_ptr(&session.storage)))
}

/// The lines a version of a custom game shows in the conflict dialog and in the Custom games
/// tab's offers; `None` is a removal.
pub(crate) fn game_lines(game: Option<&CustomGame>) -> Vec<String> {
    let Some(game) = game else {
        return vec!["Removed".to_owned()];
    };
    let integration = match game.integration {
        Integration::Override => "overrides the manifest entry",
        Integration::Extend => "extends the manifest entry",
    };
    let side = |path: &Option<String>| path.clone().unwrap_or_else(|| "none".to_owned());
    let mut lines = vec![format!("Integration: {integration}")];
    lines.extend(
        (game.files.iter())
            .map(|file| format!("File: {} / {}", side(&file.linux), side(&file.windows))),
    );
    // The form hides these and keeps them, yet they declare what a restore may write.
    lines.extend((game.entry.files.keys()).map(|file| format!("Old-style file: {file}")));
    lines.extend((game.entry.registry.keys()).map(|key| format!("Registry key: {key}")));
    lines.extend((game.entry.install_dir.keys()).map(|dir| format!("Install folder: {dir}")));
    lines
        .extend((game.pairs.iter()).map(|pair| format!("Pair: {} / {}", pair.linux, pair.windows)));
    lines.extend((game.skips.iter()).map(|skip| format!("Skipped folder: {skip}")));
    // The Steam id picks the Proton prefix a restore writes into.
    if let Some(id) = game.entry.steam.as_ref().and_then(|steam| steam.id) {
        lines.push(format!("Steam id: {id}"));
    }
    lines
}

/// The lines a version of the Old backups rules shows in the conflict dialog.
pub(super) fn rule_lines(policy: &Retention) -> Vec<String> {
    if policy.is_off() {
        return vec!["All backups are kept".to_owned()];
    }
    let mut lines = Vec::new();
    if let Some(last) = policy.keep_last {
        lines.push(format!("Keep the last {last} backups of each game"));
    }
    if let Some(within) = policy.keep_within {
        let unit = match within.unit {
            Unit::Days => "days",
            Unit::Weeks => "weeks",
            Unit::Months => "months",
            Unit::Years => "years",
            _ => "units",
        };
        lines.push(format!(
            "Keep every backup newer than {} {unit}",
            within.amount
        ));
    }
    lines
}

/// The lines of this PC's version `here` and the repository's `theirs` of a conflicting game, the
/// latter without a Wine prefix. A difference the lines do not show gets a line of its own.
pub(crate) fn conflict_lines(
    here: Option<&CustomGame>,
    theirs: Option<&CustomGame>,
) -> (Vec<String>, Vec<String>) {
    let (mut here_lines, mut theirs_lines) = (game_lines(here), game_lines(theirs));
    let shared = here.map(|game| {
        let mut game = game.clone();
        game.wine_prefix = None;
        game
    });
    if here_lines == theirs_lines && shared.as_ref() != theirs {
        let other = "Other differences, in rules this list does not show".to_owned();
        here_lines.push(other.clone());
        theirs_lines.push(other);
    }
    (here_lines, theirs_lines)
}

/// One conflict of the dialog: `title`, this PC's version `here` and the repository's `theirs`
/// side by side, a line only one side has in bold, and the choice `keep`.
pub(super) fn conflict_ui(
    ui: &mut Ui,
    title: &str,
    here: &[String],
    theirs: &[String],
    keep: &mut Option<Keep>,
) {
    ui.add_space(8.0);
    ui.label(theme::semibold(title));
    versions_ui(ui, here, theirs);
    ui.horizontal(|ui| {
        ui.radio_value(keep, Some(Keep::Here), "Keep this PC's");
        ui.radio_value(keep, Some(Keep::Repository), "Keep the repository's");
    });
}

/// This PC's version `here` and the repository's `theirs` of a custom game side by side, a line
/// only one side has in bold.
pub(crate) fn versions_ui(ui: &mut Ui, here: &[String], theirs: &[String]) {
    ui.columns(2, |columns| {
        let sides = [("This PC", here, theirs), ("The repository", theirs, here)];
        for (ui, (heading, lines, other)) in columns.iter_mut().zip(sides) {
            ui.weak(heading);
            for line in lines {
                if other.contains(line) {
                    ui.label(line);
                } else {
                    ui.label(RichText::new(line).strong())
                        .on_hover_text("Only this version has it");
                }
            }
        }
    });
}

/// What the conflict dialog's user did.
pub(super) enum Answer {
    BackUp,
    Cancel,
}

/// Saves this PC's unsent custom games (`custom_games::send`) and Old backups rules from
/// `config` to the repository, staged below `stage_root` (ADR-0029). What is not saved stays
/// unsent and is reported.
pub(super) fn send_unsent(
    storage: &SharedStorage,
    config: &AppConfig,
    stage_root: &Path,
) -> Shared {
    let mut shared = Shared::default();
    if !has_unsent(Some(config)) {
        return shared;
    }
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            shared.rules_failed = config.retention_unsent.is_some();
            shared.errors.push(format!(
                "The custom games and rules were not saved to the repository: {err:#}"
            ));
            return shared;
        }
    };
    let unsent = &config.custom_games_unsent;
    if !unsent.is_empty() {
        let sent = custom_games::send(&mut storage, &config.custom_games, unsent, stage_root);
        shared.written = sent.written.len();
        for (name, err) in sent.failed {
            shared.errors.push(match err {
                CustomGameError::Changed(_) => format!(
                    "{name} was not saved to the repository: another PC changed it again; the \
                     next Back up asks about it"
                ),
                CustomGameError::NewerFormat(_) => format!(
                    "{name} was not saved to the repository: its copy there is from a newer \
                     salvajocs"
                ),
                err => format!(
                    "{name} was not saved to the repository: {}; it is tried again at the next \
                     Back up",
                    chain(&err)
                ),
            });
        }
        for name in sent.held_back {
            shared.errors.push(format!(
                "{name} was not removed from the repository, as another custom game was not \
                 saved; it is tried again at the next Back up"
            ));
        }
        for (name, err) in sent.forget_errors {
            shared.warnings.push(format!(
                "The custom game {name} was saved, but its older copies could not be removed: {}",
                chain(&err)
            ));
        }
        shared.games = sent.sent;
    }
    if let Some(unsent) = config.retention_unsent {
        // Rules another PC changed since the check are asked about at the next Back up.
        let saved = match retention::load(&mut storage) {
            Ok(stored) if unsent.conflicts(&stored) => Err(None),
            Err(err @ (RetentionError::NewerFormat(_) | RetentionError::Parse(_))) => {
                Err(Some(err))
            }
            _ => retention::save(&mut storage, &unsent.policy, stage_root).map_err(Some),
        };
        match saved {
            Ok(saved) => {
                shared.rules = Some(unsent.policy);
                if let Some(err) = saved.forget_error {
                    shared.warnings.push(format!(
                        "The Old backups rules were saved, but their older copies could not be \
                         removed: {}",
                        chain(&err)
                    ));
                }
            }
            Err(err) => {
                shared.rules_failed = true;
                shared.errors.push(match err {
                    None => "The Old backups rules were not saved to the repository: another PC \
                             changed them again; the next Back up asks about them"
                        .to_owned(),
                    Some(RetentionError::NewerFormat(_) | RetentionError::Parse(_)) => {
                        "The Old backups rules were not saved to the repository: the rules there \
                         are from a newer salvajocs or cannot be read"
                            .to_owned()
                    }
                    Some(err) => format!(
                        "The Old backups rules were not saved to the repository: {}; they are \
                         tried again at the next Back up",
                        chain(&err)
                    ),
                });
            }
        }
    }
    shared
}
