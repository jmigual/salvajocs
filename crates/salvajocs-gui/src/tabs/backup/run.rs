use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use eframe::egui;
use salvajocs_core::backup::{self, GameBackup, Stored};
use salvajocs_core::changes::Mapper;
use salvajocs_core::config::AppConfig;
use salvajocs_core::custom_games;
use salvajocs_core::retention::{self, Cleanup, Retention};
use salvajocs_core::scan::{self, Environment, GameScan, ScanReport};
use salvajocs_core::selection::{self, Pending};

use super::choices::{self, AfterBackup, Games};
use super::results::summarize;
use super::unsent::{
    Answer, Checked, Shared, conflict_lines, conflict_ui, has_unsent, rule_lines, send_unsent,
};
use super::{Backed, BackupTab, Cleaned, Progress, SaveProblems, lock, scan_progress};
use crate::job::Job;
use crate::state::{SharedStorage, State, chain, lock_storage};
use crate::theme::{self, icons};

impl BackupTab {
    /// The dialog asking which version to keep of each change made on another PC: this PC's
    /// version from the config, the repository's from the check. **Back up** puts the choices
    /// into the config and starts the backup; **Cancel** drops it and writes nothing.
    pub(super) fn asking_ui(&mut self, ctx: &egui::Context, state: &mut State) {
        let (Some(asking), Some(config)) = (&mut self.asking, &mut state.config) else {
            return;
        };
        let (answer, dismissed) = theme::modal_dismissible(ctx, "backup_conflicts", 720.0, |ui| {
            ui.heading("Changed on another PC");
            ui.label(
                "Another PC changed these in the repository since this PC's changes started. \
                 Choose which version to keep; the backup then saves this PC's choices.",
            );
            for (index, (conflict, keep)) in asking.games.iter_mut().enumerate() {
                let here = (config.custom_games.iter()).find(|game| game.name == conflict.name);
                let (here, theirs) = conflict_lines(here, conflict.newest.as_ref());
                ui.push_id(index, |ui| {
                    conflict_ui(ui, &conflict.name, &here, &theirs, keep)
                });
            }
            if let (Some((stored, keep)), Some(unsent)) =
                (&mut asking.rules, &config.retention_unsent)
            {
                let (here, theirs) = (rule_lines(&unsent.policy), rule_lines(stored));
                ui.push_id("rules", |ui| {
                    conflict_ui(ui, "Old backups rules", &here, &theirs, keep);
                });
            }
            ui.add_space(8.0);
            // After the blocks, so the choice made in this frame counts.
            let answered = asking.answered();
            theme::row_end(ui, |ui| {
                if ui.button("Cancel").clicked() {
                    return Some(Answer::Cancel);
                }
                let back_up = theme::primary(ui, answered, Some(icons::FLOPPY_DISK), "Back up");
                back_up.clicked().then_some(Answer::BackUp)
            })
        });
        let answer = answer.or(dismissed.then_some(Answer::Cancel));
        match answer {
            Some(Answer::BackUp) => {
                if let Some(asking) = self.asking.take() {
                    self.cleared = Some(asking.source.clone());
                    self.rules_changed |= asking.rules.is_some();
                    asking.apply(config);
                }
                state.save_config(ctx);
                self.start_requested(ctx, state);
            }
            Some(Answer::Cancel) => self.asking = None,
            None => {}
        }
    }

    /// Starts the requested backup once no pending file write runs, so the backup saves every
    /// change and is the file's only writer. With unsent custom games or rules, the repository
    /// is checked first for changes made on another PC to them, and the backup starts once the
    /// check passed or its question is answered, also only with no pending file write running.
    pub(super) fn start_requested(&mut self, ctx: &egui::Context, state: &State) {
        if self.choices.busy() || self.running.is_some() {
            return;
        }
        if self.backup_requested {
            self.backup_requested = false;
            if has_unsent(state.config.as_ref()) {
                self.start_check(ctx, state);
            } else {
                self.start_backup(ctx, state);
            }
        } else if self.cleared.take().is_some() {
            self.start_backup(ctx, state);
        }
    }

    /// Reads the repository's copies of this PC's unsent custom games and rules in a job, for
    /// the conflicts the backup asks about first; writes nothing.
    fn start_check(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let storage = session.storage.clone();
        let games = config.custom_games.clone();
        let (unsent, rules) = (config.custom_games_unsent.clone(), config.retention_unsent);
        self.run_error = None;
        let job = Job::spawn(ctx, move || {
            let mut storage = lock_storage(&storage)?;
            let games = custom_games::conflicts(&mut storage, &games, &unsent)?;
            // Rules that cannot be read are the backup's to report when it saves them.
            let rules = rules.and_then(|unsent| {
                let stored = retention::load(&mut storage).ok()?;
                unsent.conflicts(&stored).then_some(stored)
            });
            Ok(Checked { games, rules })
        });
        self.checking = Some((Arc::downgrade(&session.storage), job));
    }

    /// Backs up the chosen games in a job, with the staging mirrors under the cache dir
    /// (ADR-0004). The job first saves this PC's unsent custom games and rules (ADR-0029), then
    /// the pending selection changes, and reads every game's settings again, so another
    /// machine's newer choices count. Then it scans again, so a save made since the last scan
    /// is included, and filters each game by its settings. Last, it removes old backups by the
    /// repository's retention policy. With no game chosen it only saves the unsent changes.
    pub(super) fn start_backup(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let chosen = self.chosen();
        let selection = match chosen {
            0 => None,
            _ => match self.choices.for_backup() {
                Some(selection) => Some(selection),
                None => return,
            },
        };
        let progress = Arc::new(Mutex::new(Progress {
            total: chosen,
            done: Vec::new(),
            cleaning: false,
            sending: has_unsent(Some(config)),
        }));
        self.progress = progress.clone();
        self.checked = Arc::default();
        let scanned = scan_progress(&self.checked, ctx);
        self.run_error = None;
        self.saving = SaveProblems::default();
        self.cleanup = None;
        self.shared = Shared::default();
        let (manifest, config) = (state.manifest_or_empty(), config.clone());
        let (storage, stage_root, waker) = (
            session.storage.clone(),
            state.env.cache_dir.join("stage"),
            ctx.clone(),
        );
        let config_dir = state.env.config_dir.clone();
        let environment = state.env.environment.clone();
        self.running = Some(Job::spawn(ctx, move || {
            let source = Arc::downgrade(&storage);
            // First, so the scan and the cleanup go by this PC's custom games and rules.
            let shared = send_unsent(&storage, &config, &stage_root);
            lock(&progress).sending = false;
            let Some((mut pending, settings)) = selection else {
                return Ok(Backed {
                    source,
                    shared,
                    report: None,
                    error: None,
                    saving: SaveProblems::default(),
                    after: None,
                    cleanup: None,
                });
            };
            let (mut saving, written) = match &mut pending {
                Some(pending) => save_selection(
                    &storage,
                    pending,
                    &config,
                    &settings,
                    &stage_root,
                    &config_dir,
                ),
                None => (SaveProblems::default(), None),
            };
            let reloaded = choices::load(&storage)
                .map(|games| choices::replay(games, pending.as_ref(), &config.secret_scope()));
            let filter = match &reloaded {
                Ok(games) => games,
                Err(err) => {
                    saving.warnings.push(format!(
                        "What to back up could not be read again, so the choices shown are \
                         used: {err:#}"
                    ));
                    &settings
                }
            };
            let environment = environment.unwrap_or_else(Environment::detect);
            let report = scan::scan(&manifest, &config, &environment, scanned);
            let games = chosen_files(&report, filter);
            lock(&progress).total = games.len();
            let mapper = Mapper::new(&manifest, &config, &environment);
            let backed_up = back_up(&storage, &games, &stage_root, &mapper, |backup| {
                lock(&progress).done.push(summarize(backup));
                waker.request_repaint();
            });
            // A failed game still cleans up; a run that stopped early does not.
            let (error, cleanup) = match backed_up {
                Ok(made) => {
                    lock(&progress).cleaning = true;
                    waker.request_repaint();
                    (None, Some(clean_up(&storage, &made, shared.rules_failed)))
                }
                Err(err) => (Some(format!("{err:#}")), None),
            };
            Ok(Backed {
                source,
                shared,
                report: Some(report),
                error,
                saving,
                cleanup,
                after: Some(AfterBackup {
                    source: Arc::downgrade(&storage),
                    pending,
                    written,
                    games: reloaded.ok(),
                }),
            })
        }));
    }
}

/// Backs up `games` into `storage`, comparing each with its newest backup through `mapper`, and
/// calling `progress` as each game is done. Returns the ids of the new snapshots.
pub(super) fn back_up(
    storage: &SharedStorage,
    games: &[GameScan],
    stage_root: &Path,
    mapper: &Mapper,
    progress: impl FnMut(&GameBackup),
) -> anyhow::Result<Vec<String>> {
    let mut storage = lock_storage(storage)?;
    let same_as_newest = Some(mapper);
    let backups = backup::backup_games(
        &mut storage,
        games,
        stage_root,
        &[],
        same_as_newest,
        progress,
    )?;
    let made = backups
        .into_iter()
        .filter_map(|backup| match backup.result {
            Ok(Stored::New(snapshot)) => Some(snapshot.id),
            _ => None,
        });
    Ok(made.collect())
}

/// Forgets and prunes by the repository's retention policy, never the snapshots `made`
/// (ADR-0015). A policy that cannot be read forgets nothing, but marked packs are still pruned;
/// so does a backup whose rules were not saved (`rules_failed`), since the stored ones may be
/// stricter than this PC's (ADR-0029).
pub(super) fn clean_up(storage: &SharedStorage, made: &[String], rules_failed: bool) -> Cleaned {
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            return Cleaned {
                rules_failed,
                policy_error: None,
                done: Cleanup::default(),
                error: Some(format!("{err:#}")),
            };
        }
    };
    let (policy, policy_error) = match rules_failed {
        true => (Retention::default(), None),
        false => match retention::load(&mut storage) {
            Ok(policy) => (policy, None),
            Err(err) => (Retention::default(), Some(chain(&err))),
        },
    };
    let keep: Vec<&str> = made.iter().map(String::as_str).collect();
    let (done, error) = match retention::apply(&mut storage, &policy, &keep, SystemTime::now()) {
        Ok(done) => (done, None),
        Err(err) => {
            let mut done = Cleanup::default();
            done.forgotten = err.forgotten;
            (done, Some(chain(&err.source)))
        }
    };
    Cleaned {
        rules_failed,
        policy_error,
        done,
        error,
    }
}

/// Saves this repository's pending changes of each game whose settings could be read; each
/// saved game's changes leave `pending` and the pending file in `config_dir`. Settings stage
/// below `stage_root` (ADR-0014). Returns the problems and, when the file was written, how the
/// last write went.
fn save_selection(
    storage: &SharedStorage,
    pending: &mut Pending,
    config: &AppConfig,
    settings: &Games,
    stage_root: &Path,
    config_dir: &Path,
) -> (SaveProblems, Option<Result<(), String>>) {
    let mut problems = SaveProblems::default();
    let scope = config.secret_scope();
    let games = pending
        .repositories
        .get(&scope)
        .cloned()
        .unwrap_or_default();
    if games.is_empty() {
        return (problems, None);
    }
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            problems.failed.push(format!("any game: {err:#}"));
            return (problems, None);
        }
    };
    let mut written = None;
    for (title, changes) in games {
        // Settings that could not be read are never written over.
        if matches!(settings.get(&title), Some(Err(_))) {
            continue;
        }
        match selection::save(&mut storage, &title, &changes, stage_root) {
            Ok(saved) => {
                if let Some(err) = saved.forget_error {
                    problems.warnings.push(format!(
                        "{title}: older copies of what to back up stay in the repository: {}",
                        chain(&err)
                    ));
                }
                pending.remove(config, &title);
                written = Some(pending.save(config_dir).map_err(|err| chain(&err)));
            }
            Err(err) => problems.failed.push(format!("{title}: {}", chain(&err))),
        }
    }
    (problems, written)
}

/// The games of `report` as `settings` filter them; a game skipped or with nothing left is
/// left out, and one whose settings cannot be read is backed up whole.
fn chosen_files(report: &ScanReport, settings: &Games) -> Vec<GameScan> {
    report
        .games
        .iter()
        .filter_map(|game| match settings.get(&game.title) {
            Some(Ok(settings)) => selection::apply(game, settings),
            _ => Some(game.clone()),
        })
        .collect()
}
