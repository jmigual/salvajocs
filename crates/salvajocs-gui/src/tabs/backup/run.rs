use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use eframe::egui;
use salvajocs_core::backup::{self, ByState, Stored};
use salvajocs_core::changes::Mapper;
use salvajocs_core::config::AppConfig;
use salvajocs_core::custom_games;
use salvajocs_core::manifest::Manifest;
use salvajocs_core::retention::{self, Cleanup, Retention};
use salvajocs_core::scan::{self, Environment, GameScan, ScanReport};
use salvajocs_core::selection::{self, Pending};
use salvajocs_core::slug::slug;
use salvajocs_core::sync::{self, SyncError};

use super::choices::{self, AfterBackup, Games};
use super::results::{GameResult, Outcome, summarize};
use super::unsent::{
    Answer, Checked, Shared, conflict_lines, conflict_ui, has_unsent, rule_lines, send_unsent,
};
use super::{Backed, BackupTab, Cleaned, Progress, SaveProblems, lock, scan_progress};
use crate::job::Job;
use crate::state::{SharedStorage, State, chain, lock_storage};
use crate::theme::{self, icons};

impl BackupTab {
    /// The dialog asking which version to keep of each change made on another PC (this PC's
    /// version from the config, the repository's from the check). **Back up** puts the choices
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
    /// It also waits for the comparison, as the games it leaves out are found from it.
    pub(super) fn start_requested(&mut self, ctx: &egui::Context, state: &State) {
        if self.choices.busy() || self.running.is_some() || self.previewing.is_some() {
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
    /// the pending selection changes, and reads every game's settings again, so another machine's
    /// newer choices count. Then it scans again, so a save made since the last scan is included,
    /// and filters each game by its settings. Core stores a [`BackupTab::held_back`] game only if
    /// it no longer waits, and its "not backed up" result is not shown. Last, it removes old
    /// backups by the repository's retention policy. With no game chosen it only saves the unsent
    /// changes.
    pub(super) fn start_backup(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let chosen = self.chosen();
        let held_back = self.held_back();
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
        let back_up_anyway = std::mem::take(&mut self.back_up_anyway);
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
            let environment = environment.unwrap_or_else(Environment::detect);
            let (reloaded, report, games) = to_stage(
                choices::load(&storage),
                pending.as_ref(),
                &settings,
                (&manifest, &config, &environment),
                |_| true,
                scanned,
            );
            if let Err(err) = &reloaded {
                saving.warnings.push(format!(
                    "What to back up could not be read again, so the choices shown are used: \
                     {err:#}"
                ));
            }
            // Counted as the button counts them: a held-back game only once its result shows.
            let held = (games.iter()).filter(|game| held_back.contains(&slug(&game.title)));
            lock(&progress).total = games.len() - held.count();
            let mapper = Mapper::new(&manifest, &config, &environment);
            let scope = config.secret_scope();
            let backed_up = back_up(
                &storage,
                &games,
                &stage_root,
                &mapper,
                (&config_dir, &scope),
                &back_up_anyway,
                |result| {
                    let mut progress = lock(&progress);
                    // The list showed it unticked, so "not backed up" is no news.
                    if !hidden(&result, &held_back) {
                        if held_back.contains(&slug(&result.title)) {
                            progress.total += 1;
                        }
                        progress.done.push(result);
                    }
                    drop(progress);
                    waker.request_repaint();
                },
            );
            // A failed game or base record still cleans up; a run that stopped early does not.
            let (error, cleanup) = match backed_up {
                Ok((made, unrecorded)) => {
                    if let Some(err) = unrecorded {
                        saving.warnings.push(format!(
                            "This PC's sync record (sync-base.json) was not updated, so the next \
                             Back up may ask about the games backed up now: {}",
                            chain(&err)
                        ));
                    }
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

/// Whether the backup's `result` is left out of the results: a game of `held_back`, which the list
/// showed waiting for a choice, unticked, that core did not back up for the same reason.
fn hidden(result: &GameResult, held_back: &BTreeSet<String>) -> bool {
    let waited = matches!(
        result.outcome,
        Outcome::Behind { .. } | Outcome::ChangedOnBoth { .. } | Outcome::UnknownBase(_)
    );
    waited && held_back.contains(&slug(&result.title))
}

/// Backs up `games` into `storage` by each one's state on this PC, from its newest backup read
/// through `mapper` and its base in the `sync-base.json` of `config_dir` under the repository's
/// `scope`, calling `progress` with each game's result as it is done, then records the new bases there. A game
/// whose slug is in `back_up_anyway` is backed up even when behind, changed on both PCs or of
/// unknown base. Returns the ids of the new snapshots, and why the bases were not recorded when
/// they were not.
pub(super) fn back_up(
    storage: &SharedStorage,
    games: &[GameScan],
    stage_root: &Path,
    mapper: &Mapper,
    (config_dir, scope): (&Path, &str),
    back_up_anyway: &BTreeSet<String>,
    mut progress: impl FnMut(GameResult),
) -> anyhow::Result<(Vec<String>, Option<SyncError>)> {
    let mut storage = lock_storage(storage)?;
    let host = storage.hostname()?;
    // Read under the storage lock, which every job that records a base holds.
    let bases = sync::load(config_dir);
    let by_state = ByState {
        mapper,
        bases: &bases,
        scope,
        back_up_anyway,
    };
    let backups = backup::backup_games(
        &mut storage,
        games,
        stage_root,
        &[],
        Some(&by_state),
        |backup| progress(summarize(backup, back_up_anyway, &host)),
    )?;
    let bases =
        (backups.iter()).filter_map(|backup| Some((backup.slug.clone(), backup.base.clone()?)));
    let unrecorded = sync::record(config_dir, scope, bases).err();
    let made = backups
        .into_iter()
        .filter_map(|backup| match backup.result {
            Ok(Stored::New(snapshot)) => Some(snapshot.id),
            _ => None,
        });
    Ok((made.collect(), unrecorded))
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

/// What a Back up stages now (ADR-0014): the scan of the games whose title `keep` accepts, each
/// filtered by its settings. They are `read`, the repository's settings read again, with
/// `pending`, this PC's unsaved changes, replayed for the repository of `config`; when `read`
/// failed, `fallback`. Returns the replayed settings or why they could not be read, the scan,
/// and its games as [`chosen_files`] filters them.
pub(crate) fn to_stage(
    read: anyhow::Result<Games>,
    pending: Option<&Pending>,
    fallback: &Games,
    (manifest, config, environment): (&Manifest, &AppConfig, &Environment),
    keep: impl Fn(&str) -> bool,
    progress: impl FnMut(usize, usize),
) -> (anyhow::Result<Games>, ScanReport, Vec<GameScan>) {
    let reloaded = read.map(|games| choices::replay(games, pending, &config.secret_scope()));
    let report = scan::scan_where(manifest, config, environment, keep, progress);
    let games = chosen_files(&report, reloaded.as_ref().unwrap_or(fallback));
    (reloaded, report, games)
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
