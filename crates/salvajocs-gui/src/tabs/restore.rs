//! The Restore tab: the games in the repository with their snapshots from every host, the build
//! to restore into, the plan read from the snapshot's file list and the restore behind a safety
//! backup (ADR-0005, ADR-0010, ADR-0011, ADR-0020). Backups can also be deleted from the
//! repository (ADR-0015).

use std::collections::BTreeSet;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Ui};
use salvajocs_core::config::ListSort;
use salvajocs_core::restore::{GameBackups, Listed};
use salvajocs_core::retention::Cleanup;
use salvajocs_core::stats::BackupStats;

use crate::job::Job;
use crate::state::{SharedStorage, State};
use crate::tabs::count;
use crate::tabs::details::Details;
use crate::tabs::mapping::Dialog;
use crate::tabs::sort::Line;
use crate::tabs::space::{Polled, Space};
use crate::theme::{self, icons};

mod batch;
mod delete;
mod dirs;
mod keys;
mod pick;
mod plan;
mod run;

use batch::Batch;
use delete::{Deletion, describe_delete_error};
use pick::{Detected, TargetPick};
use plan::{Fetched, Prepared, Preview};
use run::{Restored, describe_run_error};

/// The width of the games sidebar at rest, and the least it can be dragged to.
const SIDEBAR_WIDTH: f32 = 240.0;

/// The width the plan beside the games sidebar keeps, which the sidebar cannot be dragged into.
const PLAN_WIDTH: f32 = 600.0;

/// The state of the Restore tab.
#[derive(Default)]
pub struct RestoreTab {
    /// The repository the list came from.
    repository: Option<SharedStorage>,
    listing: Option<Job<Vec<GameBackups>>>,
    games: Vec<GameBackups>,
    /// The backup figures of each of `games`, in its order, built with `order` (ADR-0027).
    figures: Vec<BackupStats>,
    /// Works out the space of each listed game's backups.
    space: Option<Space>,
    /// Counts the listings and the space runs that ended, which `order` is built from.
    list_changes: u64,
    /// The indices of `games` in the order shown.
    order: Vec<usize>,
    /// The sort and `list_changes` that `order` was built for.
    sorted: Option<(ListSort, u64)>,
    /// The short line of figures under each of `games`, in its order; `None` until drawn.
    lines: Vec<Option<Line>>,
    /// A restore, batch restore or delete ended since [`RestoreTab::take_written`] last ran.
    written: bool,
    /// A Back up checks, asks, waits to start or runs; it holds the repository's lock, so the
    /// tab's controls wait for it.
    backing_up: bool,
    /// The slug of the chosen game.
    selected: Option<String>,
    /// The slug of the game chosen before a restore or delete, which the listing it started
    /// chooses again if it still lists it.
    reselect: Option<String>,
    /// The slugs of the games ticked for a batch restore.
    checked: BTreeSet<String>,
    /// The chosen snapshot's index in the game's `snapshots`. It starts as the game's default, so
    /// `None` when every snapshot is a safety backup, which is never preselected (ADR-0010).
    snapshot: Option<usize>,
    detecting: Option<Job<Detected>>,
    pick: Option<TargetPick>,
    picking: Option<Job<Option<PathBuf>>>,
    /// The plan or dialog confirm job, with the [`RestoreTab::generation`] it started in.
    preparing: Option<(u64, Job<Prepared>)>,
    /// The generation the last plan job started in, so a failed plan does not start again.
    attempted: Option<u64>,
    /// The last snapshot listing read, so a change of targets reads nothing.
    last_read: Option<Arc<Listed>>,
    mapping: Option<(Arc<Fetched>, Dialog)>,
    /// Whether the mapping dialog waits behind its "Choose folders…" button.
    dialog_hidden: bool,
    preview: Option<Preview>,
    /// The open details of a file the plan overwrites.
    details: Option<Details>,
    restoring: Option<Job<Restored>>,
    restored: Option<Restored>,
    confirm_delete: Option<Deletion>,
    deleting: Option<Job<Cleanup>>,
    /// How many backups the last delete removed.
    removed: Option<usize>,
    error: Option<String>,
    batch: Batch,
    /// Counts [`RestoreTab::forget_plan`] calls, so a job's result for a closed dialog or
    /// changed targets is dropped.
    generation: u64,
}

impl RestoreTab {
    /// Draws the tab. Listing, fetching, planning and restoring run in jobs; the mapping dialog's
    /// choices are saved to the config. `manifest_loading` is whether the manifest job runs.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        self.poll(ui.ctx(), state, manifest_loading);
        let busy = self.busy();
        // `default_targets` looks the game's Steam id up in the manifest. An open batch keeps the
        // other controls out of reach until it runs or is cancelled.
        let idle = !busy && !manifest_loading && !self.batch.open();
        if state.config.is_none() {
            theme::page(ui, |ui| ui.label("Choose a repository first."));
        } else if self.untouched() {
            theme::page(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| self.empty_ui(ui, state, manifest_loading));
                self.error_ui(ui);
            });
        } else {
            let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(12);
            // The sidebar never takes the room the plan's rows need.
            let widest = (ui.available_width() - PLAN_WIDTH).max(SIDEBAR_WIDTH);
            egui::Panel::left("restore_games")
                .default_size(SIDEBAR_WIDTH)
                .size_range(SIDEBAR_WIDTH..=widest)
                .resizable(true)
                .frame(frame)
                .show(ui, |ui| self.games_ui(ui, state, idle));
            theme::page(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| self.game_ui(ui, state, manifest_loading));
                if busy || self.detecting.is_some() {
                    ui.spinner();
                }
                self.error_ui(ui);
                self.removed_ui(ui);
                if self.batch.open() {
                    self.batch_ui(ui, state);
                } else {
                    self.preview_ui(ui, state);
                }
                self.restored_ui(ui);
                self.batch_restored_ui(ui);
            });
        }
        self.mapping_ui(ui.ctx(), state);
        self.batch_mapping_ui(ui.ctx(), state);
        self.delete_ui(ui.ctx());
        if (self.details.as_mut()).is_some_and(|details| details.ui(ui.ctx())) {
            self.details = None;
        }
    }

    /// Drops the listed backups, the plan, the chosen and ticked games, the batch confirm and a
    /// delete waiting for its confirm, as after a cleanup that may have forgotten a snapshot;
    /// "Load backups" lists them again. A running plan job's result is dropped; a running restore
    /// is kept.
    pub(crate) fn drop_listing(&mut self) {
        self.forget_plan();
        self.batch.close();
        self.confirm_delete = None;
        self.repository = None;
        self.listing = None;
        self.games.clear();
        self.space = None;
        self.list_changes += 1;
        self.selected = None;
        self.reselect = None;
        self.checked.clear();
        self.snapshot = None;
    }

    /// Whether a restore, batch restore or delete ended since the last call, so other views of
    /// the repository may be out of date. A flag rather than `poll`'s result, since `ui` polls
    /// too and would swallow it.
    pub(crate) fn take_written(&mut self) -> bool {
        mem::take(&mut self.written)
    }

    /// Tells the tab whether a Back up runs, which keeps its controls disabled until it ends.
    pub(crate) fn set_backing_up(&mut self, backing_up: bool) {
        self.backing_up = backing_up;
    }

    /// Whether a listing, restore or other job of this tab is running. A plan job only reads,
    /// so it keeps nothing waiting; with the mapping dialog open it is the dialog's confirm.
    pub(crate) fn busy(&self) -> bool {
        self.listing.is_some()
            || self.picking.is_some()
            || (self.preparing.is_some() && self.mapping.is_some())
            || self.restoring.is_some()
            || self.deleting.is_some()
            || self.batch.busy()
    }

    /// Whether a restore, batch restore or delete runs, which closing the app would cut short.
    pub(crate) fn writing(&self) -> bool {
        self.restoring.is_some() || self.deleting.is_some() || self.batch.writing()
    }

    /// Whether the tab has nothing to show yet: no list loaded or loading, and no plan or report.
    fn untouched(&self) -> bool {
        self.repository.is_none()
            && !self.busy()
            && self.preview.is_none()
            && self.mapping.is_none()
            && self.restored.is_none()
            && self.batch.untouched()
    }

    fn error_ui(&self, ui: &mut Ui) {
        if let Some(error) = &self.error {
            let color = ui.visuals().error_fg_color;
            theme::status(ui, icons::X_CIRCLE, color, error);
        }
    }

    fn removed_ui(&self, ui: &mut Ui) {
        if let Some(removed) = self.removed {
            let accent = theme::palette(ui).accent;
            let text = format!("Removed {}", count(removed, "backup"));
            theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
        }
    }

    /// Drops the plan and the dialog, which no longer match the chosen game or targets, and the
    /// result of a plan job or dialog confirm still running.
    fn forget_plan(&mut self) {
        self.preview = None;
        self.details = None;
        self.mapping = None;
        self.generation += 1;
    }

    /// Collects the tab's finished jobs, after dropping what belongs to a repository Settings
    /// replaced, then starts the plan of the current choice when it has none. The app calls it
    /// every frame, whichever tab is in front, so a finished job does not stay running.
    /// `manifest_loading` is whether the manifest job runs.
    pub(crate) fn poll(&mut self, ctx: &egui::Context, state: &mut State, manifest_loading: bool) {
        // The list, plan and dialog of a repository Settings replaced are dropped. A running
        // restore is kept, so its report shows and no new restore clears the work dir it
        // downloads into; the generation bump drops a running plan job's result.
        let current = state.session.as_ref().map(|session| &session.storage);
        if (self.repository.as_ref())
            .is_some_and(|listed| !current.is_some_and(|current| Arc::ptr_eq(listed, current)))
        {
            self.drop_listing();
            self.last_read = None;
            self.detecting = None;
            self.pick = None;
            self.removed = None;
            self.error = None;
        }
        self.poll_jobs(ctx, state);
        self.poll_batch(ctx, state);
        if let Some(space) = &mut self.space {
            match space.poll() {
                Polled::Arrived => self.lines.fill(None),
                // The order changes once, when every figure is in.
                Polled::Finished => self.list_changes += 1,
                Polled::Nothing => {}
            }
        }
        if let Some(details) = &mut self.details {
            details.poll();
        }
        self.auto_plan(ctx, state, manifest_loading);
    }

    /// Starts the plan of the chosen game, snapshot and targets once they are all set, unless
    /// this choice was already tried. One plan job runs at a time; a result for an older choice
    /// is dropped and the next frame starts the current one.
    fn auto_plan(&mut self, ctx: &egui::Context, state: &State, manifest_loading: bool) {
        let ready = !manifest_loading
            && state.session.is_some()
            && !self.batch.open()
            && self.detecting.is_none()
            && self.preparing.is_none()
            && self.attempted != Some(self.generation)
            && (self.pick.as_ref()).is_some_and(|pick| pick.ready() && !pick.typing);
        let selected = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref());
        let chosen = selected.and_then(|game| {
            let snapshot = game.snapshots.get(self.snapshot?)?;
            Some((game.title.clone(), snapshot.clone()))
        });
        if let (true, Some((title, snapshot))) = (ready, chosen) {
            self.attempted = Some(self.generation);
            self.start_plan(ctx, state, title, snapshot);
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context, state: &mut State) {
        if let Some(job) = &mut self.listing
            && let Some(result) = job.take_result()
        {
            self.listing = None;
            match result {
                Ok(games) => {
                    // A delete may have taken a ticked game's last regular backup, or the game.
                    self.checked.retain(|slug| {
                        (games.iter()).any(|game| game.slug == *slug && game.default.is_some())
                    });
                    // The new list may order the snapshots differently.
                    let chosen = self.chosen().map(|snapshot| snapshot.id.clone());
                    self.games = games;
                    if let Some(chosen) = chosen {
                        self.snapshot = (self.games.iter())
                            .find(|game| Some(&game.slug) == self.selected.as_ref())
                            .and_then(|game| game.snapshots.iter().position(|s| s.id == chosen));
                    }
                    self.start_space(ctx, state);
                    let chosen = self.reselect.take();
                    let game = (self.games.iter()).find(|game| Some(&game.slug) == chosen.as_ref());
                    if let Some(game) = game.cloned() {
                        self.select(ctx, state, &game);
                    }
                }
                Err(err) => {
                    self.reselect = None;
                    self.error = Some(format!("The backups cannot be listed: {err:#}"));
                }
            }
        }
        if let Some(job) = &mut self.detecting
            && let Some(result) = job.take_result()
        {
            self.detecting = None;
            match result {
                // A result for a snapshot no longer chosen is dropped.
                Ok(detected) if detected.snapshot != self.chosen().map(|s| s.id.clone()) => {}
                Ok(detected) => {
                    let stored = (detected.listed.as_ref())
                        .and_then(|listed| listed.staged.meta.prefix.clone());
                    if let Some(listed) = detected.listed {
                        self.last_read = Some(listed);
                    }
                    match &mut self.pick {
                        // Edited while this ran: what the default said is about the old snapshot.
                        Some(pick) if pick.edited => {
                            pick.reason = None;
                            pick.missing = None;
                            pick.ambiguous.clear();
                            pick.stored_several.clear();
                        }
                        _ => {
                            let missing = stored.filter(|_| detected.defaults.stored_missing);
                            self.pick = Some(TargetPick::new(detected.defaults, missing));
                        }
                    }
                }
                Err(err) => self.error = Some(format!("{err:#}")),
            }
        }
        if let Some(job) = &mut self.picking
            && let Some(result) = job.take_result()
        {
            self.picking = None;
            if let (Ok(Some(path)), Some(pick)) = (result, &mut self.pick) {
                pick.prefix = path.to_string_lossy().into_owned();
                pick.use_prefix = true;
                pick.edited = true;
                self.forget_plan();
            }
        }
        if let Some((generation, job)) = &mut self.preparing
            && let Some(result) = job.take_result()
        {
            let current = *generation == self.generation;
            self.preparing = None;
            if let Ok(prepared) = &result {
                self.last_read = Some(prepared.fetched.listed.clone());
            }
            match result {
                // A plan for a closed dialog or other targets is neither shown nor saved.
                _ if !current => {}
                Ok(prepared) => self.prepared(prepared, ctx, state),
                Err(err) => self.error = Some(format!("The plan failed: {err:#}")),
            }
        }
        if let Some(job) = &mut self.restoring
            && let Some(result) = job.take_result()
        {
            self.restoring = None;
            self.written = true;
            match result {
                Ok(restored) => {
                    self.restored = Some(restored);
                    // The list gains the safety backup.
                    self.forget_plan();
                    self.list_keeping_choice(ctx, state);
                }
                // Nothing was written, so the plan stays for another try.
                Err(err) => self.error = Some(describe_run_error(&err)),
            }
        }
        if let Some(job) = &mut self.deleting
            && let Some(result) = job.take_result()
        {
            self.deleting = None;
            self.written = true;
            match result {
                Ok(done) if done.append_only_kept > 0 => {
                    self.error =
                        Some("The repository is append-only, so nothing was removed".to_owned());
                }
                Ok(done) => self.removed = Some(done.forgotten),
                Err(err) => self.error = Some(describe_delete_error(&err)),
            }
            // A plan may be of a deleted snapshot, and the list may have lost some.
            self.forget_plan();
            self.list_keeping_choice(ctx, state);
        }
    }
}

#[cfg(test)]
mod tests;
