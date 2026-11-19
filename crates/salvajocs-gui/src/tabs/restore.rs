//! The Restore tab: the games in the repository with their snapshots from every host, the build
//! to restore into, the plan read from the snapshot's file list and the restore behind a safety
//! backup (ADR-0005, ADR-0010, ADR-0011, ADR-0020). Backups can also be deleted from the
//! repository (ADR-0015).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Response, RichText, TextEdit, Ui, Vec2};
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::layout::{Base, Context, FILES_DIR, GameVars, LinuxFolders, WinFolders};
use salvajocs_core::prefix;
use salvajocs_core::restore::{
    self, GameBackups, GameInfo, Listed, PRE_RESTORE_TAG, PrepareError, RestorePlan, RestoreReport,
    RunError, SafetyBackup, SkippedItem, Targets, Write, WriteState,
};
use salvajocs_core::retention::{self, Cleanup, DeleteError};
use salvajocs_core::scan::Environment;
use salvajocs_core::storage::SnapshotInfo;

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::theme::{self, icons};

mod batch;

use batch::Batch;

/// A UTC time as `YYYY-MM-DD HH:MM UTC`.
fn format_time(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's `civil_from_days`, for days since 1970-01-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

/// How a snapshot shows in the picker: time, host and the `pre-restore` label.
fn snapshot_label(snapshot: &SnapshotInfo) -> String {
    let mut label = format!("{}, {}", format_time(snapshot.time), snapshot.host);
    if snapshot.tags.iter().any(|tag| tag == PRE_RESTORE_TAG) {
        label += ", pre-restore";
    }
    label
}

/// The build the user restores into, preselected from `restore::default_targets` (ADR-0005).
struct TargetPick {
    /// The native Windows folders on a Windows host, where there is nothing to pick.
    windows: Option<Context>,
    /// The native Linux folders on a Linux host.
    linux: Option<LinuxFolders>,
    use_linux: bool,
    use_prefix: bool,
    /// The prefix path as typed or chosen.
    prefix: String,
    /// Whether `prefix` was typed and not applied yet, so half-typed paths are not planned.
    typing: bool,
    /// The prefix `default_targets` found with its path as text, reused while `prefix` still
    /// names it.
    found: Option<(String, Context)>,
}

impl TargetPick {
    fn new(targets: Targets) -> Self {
        let mut pick = Self {
            windows: None,
            use_linux: targets.linux.is_some(),
            linux: targets.linux,
            use_prefix: false,
            prefix: String::new(),
            typing: false,
            found: None,
        };
        match targets.win {
            Some(native @ Context::WindowsNative(_)) => pick.windows = Some(native),
            Some(found @ Context::WinePrefix { .. }) => {
                if let Context::WinePrefix { prefix, .. } = &found {
                    pick.prefix = prefix.to_string_lossy().into_owned();
                }
                pick.use_prefix = true;
                pick.found = Some((pick.prefix.clone(), found));
            }
            _ => {}
        }
        pick
    }

    /// Whether the pick names at least one target.
    fn ready(&self) -> bool {
        self.windows.is_some()
            || (self.use_linux && self.linux.is_some())
            || (self.use_prefix && !self.prefix.trim().is_empty())
    }

    /// The targets the pick names. Reads the prefix's Wine user from disk, so it runs in a job.
    fn targets(&self) -> impl FnOnce() -> anyhow::Result<Targets> + Send + use<> {
        let linux = self.linux.clone().filter(|_| self.use_linux);
        let (windows, found) = (self.windows.clone(), self.found.clone());
        let prefix = (self.use_prefix && self.windows.is_none()).then(|| self.prefix.clone());
        move || {
            let win = match (windows, prefix) {
                (Some(native), _) => Some(native),
                (None, None) => None,
                (None, Some(text)) => Some(match found {
                    // An untouched path keeps the found prefix, which may not be valid UTF-8.
                    Some((found_text, found)) if found_text == text => found,
                    _ => {
                        let prefix = PathBuf::from(text.trim());
                        let user = prefix::wine_user(&prefix)?;
                        let folders = WinFolders::for_prefix(&prefix, &user);
                        Context::WinePrefix { prefix, folders }
                    }
                }),
            };
            Ok(Targets { linux, win })
        }
    }

    /// Draws the build choice; returns whether it changed.
    fn ui(&mut self, ui: &mut Ui) -> bool {
        if self.windows.is_some() {
            ui.label("Restores into this Windows user's folders.");
            return false;
        }
        let mut changed = false;
        ui.label("Restore into:");
        let native = ui.add_enabled(
            self.linux.is_some(),
            egui::Checkbox::new(&mut self.use_linux, "Native Linux build"),
        );
        changed |= native.changed();
        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(
                    &mut self.use_prefix,
                    "Windows build in a Wine/Proton prefix",
                )
                .changed();
            let label = ui.label("Prefix");
            let field = TextEdit::singleline(&mut self.prefix).hint_text("/path/to/pfx");
            let field = ui.add(field).labelled_by(label.id);
            // Enter also takes the focus away, so either applies the typed path.
            if field.changed() {
                self.typing = true;
                changed = true;
            } else if field.lost_focus() && self.typing {
                self.typing = false;
                changed = true;
            }
        });
        ui.weak(
            "Saves of the other build are mapped onto the chosen one (cross-build restore); \
             with both chosen, each goes into its own.",
        );
        changed
    }
}

/// A snapshot's listing and what `restore::prepare` needs besides the user's choices.
struct Fetched {
    /// The repository the snapshot came from, which the safety backup goes into.
    storage: SharedStorage,
    title: String,
    listed: Arc<Listed>,
    targets: Targets,
    vars: GameVars,
    info: GameInfo,
}

impl Fetched {
    fn prepare(&self, choices: &[(SnapshotRoot, Choice)]) -> Result<RestorePlan, PrepareError> {
        restore::prepare(
            &self.listed.staged,
            &self.targets,
            &self.vars,
            &self.info,
            choices,
        )
    }

    /// `plan` with how each write compares with its target. Reads the targets, so it runs in
    /// a job.
    fn labelled(
        &self,
        plan: Result<RestorePlan, PrepareError>,
    ) -> Result<(RestorePlan, Vec<Label>), PrepareError> {
        plan.map(|plan| {
            let labels = (plan.writes.iter())
                .map(|write| Label::of(write, &self.listed))
                .collect();
            (plan, labels)
        })
    }
}

/// How one write of the plan compares with its target (`restore::compare_writes`).
#[derive(Debug)]
enum Label {
    /// What the write does to its target.
    State(WriteState),
    /// Why the restore refuses to write it.
    WillFail(String),
    /// Why the safety backup cannot read its target, which fails the whole restore.
    Blocks(String),
}

impl Label {
    /// How `write` compares with its target, against the listing `listed` it was planned from.
    fn of(write: &Write, listed: &Listed) -> Self {
        let plan = RestorePlan::new(vec![write.clone()], None, Vec::new(), false, Vec::new());
        match restore::compare_writes(&plan, listed) {
            Ok(mut states) => match states.pop().expect("one state per write") {
                Ok(state) => Label::State(state),
                Err(err) => Label::WillFail(format!("{:#}", anyhow!(err))),
            },
            Err(err) => Label::Blocks(format!("{:#}", anyhow!(err))),
        }
    }
}

/// What a plan job hands back.
struct Prepared {
    fetched: Arc<Fetched>,
    /// The dialog's choices this plan was prepared with; empty for the first try.
    choices: Vec<(SnapshotRoot, Choice)>,
    plan: Result<(RestorePlan, Vec<Label>), PrepareError>,
}

/// The plan shown before anything is written.
struct Preview {
    fetched: Arc<Fetched>,
    plan: RestorePlan,
    /// How each write compares with its target.
    labels: Vec<Label>,
    /// The override text of each write.
    overrides: Vec<String>,
    /// The write whose override was refused, with why.
    override_error: Option<(usize, String)>,
}

/// What a finished restore shows.
struct Restored {
    title: String,
    safety_backup: String,
    /// One line per write and for the registry, with whether it failed.
    lines: Vec<(String, bool)>,
}

impl Restored {
    /// What `run` did for the game `title` with `plan`, as lines to show.
    fn new(title: String, plan: &RestorePlan, report: RestoreReport) -> Self {
        let safety_backup = match report.safety_backup {
            SafetyBackup::Created(snapshot) => format!(
                "Safety backup: snapshot {} created, labelled pre-restore",
                snapshot.id.get(..8).unwrap_or(&snapshot.id)
            ),
            SafetyBackup::Unchanged => {
                "Safety backup: the repository already holds the replaced files".to_owned()
            }
            SafetyBackup::NothingExisted => {
                "Safety backup: none needed, nothing existed at the targets".to_owned()
            }
            _ => "Safety backup: unknown result".to_owned(),
        };
        let mut lines: Vec<_> = (plan.writes.iter().zip(report.files))
            .map(|(write, result)| match result {
                Ok(()) => (format!("{}: restored", write.target.display()), false),
                Err(err) => (
                    format!("{}: failed: {:#}", write.target.display(), anyhow!(err)),
                    true,
                ),
            })
            .collect();
        lines.extend(report.registry.map(|result| match result {
            Ok(()) => ("Registry: restored".to_owned(), false),
            Err(err) => (format!("Registry: failed: {:#}", anyhow!(err)), true),
        }));
        Self {
            title,
            safety_backup,
            lines,
        }
    }

    /// Draws the safety backup line and a line per write and for the registry.
    fn lines_ui(&self, ui: &mut Ui) {
        ui.label(&self.safety_backup);
        let (accent, error_color) = (theme::palette(ui).accent, ui.visuals().error_fg_color);
        for (line, failed) in &self.lines {
            if *failed {
                theme::status(ui, icons::X_CIRCLE, error_color, line);
            } else {
                theme::icon_label(ui, icons::CHECK_CIRCLE, accent, line);
            }
        }
    }
}

/// Backups of one game waiting in the delete confirm dialog.
struct Deletion {
    title: String,
    snapshots: Vec<SnapshotInfo>,
    /// Whether these are all of the game's backups.
    all: bool,
    /// Whether Cancel still has to take the focus, once when the dialog opens.
    focus: bool,
}

/// The state of the Restore tab.
#[derive(Default)]
pub struct RestoreTab {
    /// The repository the list came from.
    repository: Option<SharedStorage>,
    listing: Option<Job<Vec<GameBackups>>>,
    games: Vec<GameBackups>,
    /// The slug of the chosen game.
    selected: Option<String>,
    /// The slugs of the games ticked for a batch restore.
    checked: BTreeSet<String>,
    /// The chosen snapshot's index in the game's `snapshots`. It starts as the game's default, so
    /// `None` when every snapshot is a safety backup, which is never preselected (ADR-0010).
    snapshot: Option<usize>,
    detecting: Option<Job<Targets>>,
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
            egui::Panel::left("restore_games")
                .exact_size(240.0)
                .resizable(false)
                .frame(frame)
                .show(ui, |ui| {
                    ui.add_enabled_ui(idle, |ui| self.games_ui(ui, state));
                });
            theme::page(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| self.game_ui(ui, manifest_loading));
                if busy {
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
    }

    /// Drops the listed backups, the chosen and ticked games and a delete waiting for its
    /// confirm, as after a cleanup that may have forgotten one of them; "Load backups" lists them
    /// again. A running plan job or restore is kept.
    pub(crate) fn drop_listing(&mut self) {
        self.confirm_delete = None;
        self.repository = None;
        self.listing = None;
        self.games.clear();
        self.selected = None;
        self.checked.clear();
        self.snapshot = None;
    }

    /// Whether a listing, restore or other job of this tab is running. A plan job only reads,
    /// so it keeps nothing waiting; with the mapping dialog open it is the dialog's confirm.
    pub(crate) fn busy(&self) -> bool {
        self.listing.is_some()
            || self.detecting.is_some()
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
            self.forget_plan();
            self.drop_listing();
            self.batch.close();
            self.last_read = None;
            self.detecting = None;
            self.pick = None;
            self.removed = None;
            self.error = None;
        }
        self.poll_jobs(ctx, state);
        self.poll_batch(ctx, state);
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
                    self.games = games;
                }
                Err(err) => self.error = Some(format!("The backups cannot be listed: {err:#}")),
            }
        }
        if let Some(job) = &mut self.detecting
            && let Some(result) = job.take_result()
        {
            self.detecting = None;
            match result {
                Ok(targets) => self.pick = Some(TargetPick::new(targets)),
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
            match result {
                Ok(restored) => {
                    self.restored = Some(restored);
                    // The list gains the safety backup.
                    self.forget_plan();
                    self.start_listing(ctx, state);
                }
                // Nothing was written, so the plan stays for another try.
                Err(err) => self.error = Some(describe_run_error(&err)),
            }
        }
        if let Some(job) = &mut self.deleting
            && let Some(result) = job.take_result()
        {
            self.deleting = None;
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
            self.start_listing(ctx, state);
        }
    }

    /// Shows the plan, or the mapping dialog while a root needs a choice (ADR-0011). Choices
    /// that produced a plan are saved on the game's custom entry.
    fn prepared(&mut self, prepared: Prepared, ctx: &egui::Context, state: &mut State) {
        let Prepared {
            fetched,
            choices,
            plan,
        } = prepared;
        match plan {
            Ok((plan, labels)) => {
                if let (false, Some(config)) = (choices.is_empty(), &mut state.config) {
                    crossbuild::apply_choices(config, &fetched.title, &choices);
                    state.save_config(ctx);
                }
                self.mapping = None;
                self.preview = Some(Preview {
                    overrides: vec![String::new(); plan.writes.len()],
                    fetched,
                    plan,
                    labels,
                    override_error: None,
                });
            }
            Err(PrepareError::NeedsMapping(entries)) => {
                self.mapping = Some((fetched, Dialog::new(entries)));
                self.dialog_hidden = true;
            }
            Err(PrepareError::Unclassifiable { root, template }) => match &mut self.mapping {
                Some((_, dialog)) => dialog.reject(&root, &template),
                None => {
                    self.error = Some(format!(
                        "The saved target {template} is not a placeholder template of this OS"
                    ));
                }
            },
            Err(err) => self.error = Some(format!("The plan failed: {err}")),
        }
    }

    fn start_listing(&mut self, ctx: &egui::Context, state: &State) {
        let Some(session) = &state.session else {
            return;
        };
        let storage = session.storage.clone();
        self.repository = Some(storage.clone());
        self.selected = None;
        self.pick = None;
        self.listing = Some(Job::spawn(ctx, move || {
            Ok(restore::list(&*lock_storage(&storage)?)?)
        }));
    }

    /// Before the first listing: what the tab is for and the listing as its one action.
    fn empty_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.weak("See the games in your repository and their snapshots.");
            ui.add_space(8.0);
            let session = state.session.is_some();
            if theme::primary(ui, session, None, "Load backups").clicked() {
                self.load(ui.ctx(), state);
            }
            if !session {
                ui.weak("The repository is not open.");
            }
            if manifest_loading {
                ui.weak("The game manifest is loading.");
            }
        });
    }

    fn load(&mut self, ctx: &egui::Context, state: &State) {
        self.error = None;
        self.removed = None;
        self.forget_plan();
        self.start_listing(ctx, state);
    }

    /// The sidebar: the listing button and the games of the repository.
    fn games_ui(&mut self, ui: &mut Ui, state: &State) {
        let load = egui::Button::new("Load backups").min_size(Vec2::new(ui.available_width(), 0.0));
        if ui.add_enabled(state.session.is_some(), load).clicked() {
            self.load(ui.ctx(), state);
        }
        if state.session.is_none() {
            ui.weak("The repository is not open.");
        }
        if self.games.is_empty() {
            return;
        }
        self.select_all_ui(ui);
        let mut chosen = None;
        theme::group(ui, |ui| {
            theme::pill_list(ui);
            for game in &self.games {
                ui.horizontal(|ui| {
                    batch::include_ui(ui, &mut self.checked, game);
                    let selected = self.selected.as_ref() == Some(&game.slug);
                    let row = egui::Button::selectable(selected, &game.title)
                        .min_size(Vec2::new(ui.available_width(), 0.0));
                    if ui.add(row).clicked() && !selected {
                        chosen = Some(game.clone());
                    }
                });
            }
        });
        if let Some(game) = chosen {
            self.choose(ui.ctx(), state, &game);
        }
        self.batch_button_ui(ui, state);
    }

    /// The chosen game: its snapshot and the build to restore into.
    fn game_ui(&mut self, ui: &mut Ui, manifest_loading: bool) {
        let listed = self.repository.is_some() && self.listing.is_none();
        if listed && self.games.is_empty() && self.error.is_none() {
            ui.weak("The repository has no backups yet. Back up your games in the Backup tab.");
        }
        let selected = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref());
        let Some(game) = selected.cloned() else {
            return;
        };
        ui.heading(&game.title);
        let before = self.snapshot;
        ui.horizontal(|ui| {
            let label = ui.label("Snapshot");
            egui::ComboBox::from_id_salt("restore_snapshot")
                .selected_text(
                    (self.snapshot.and_then(|index| game.snapshots.get(index)))
                        .map_or_else(|| "Choose a snapshot".to_owned(), snapshot_label),
                )
                .show_ui(ui, |ui| {
                    theme::pill_list(ui);
                    for (index, snapshot) in game.snapshots.iter().enumerate() {
                        let label = snapshot_label(snapshot);
                        ui.selectable_value(&mut self.snapshot, Some(index), label);
                    }
                })
                .response
                .labelled_by(label.id);
            let chosen = self.snapshot.and_then(|index| game.snapshots.get(index));
            let delete = ui.add_enabled(chosen.is_some(), egui::Button::new("Delete snapshot"));
            if delete.clicked()
                && let Some(snapshot) = chosen
            {
                self.confirm_delete = Some(Deletion {
                    title: game.title.clone(),
                    snapshots: vec![snapshot.clone()],
                    all: game.snapshots.len() == 1,
                    focus: true,
                });
            }
        });
        let mut changed = self.snapshot != before;
        if let Some(pick) = &mut self.pick {
            changed |= pick.ui(ui);
            if pick.windows.is_none() && ui.button("Browse").clicked() {
                self.picking = Some(Job::spawn(ui.ctx(), || {
                    Ok(rfd::FileDialog::new().pick_folder())
                }));
            }
        }
        if changed {
            self.forget_plan();
        }
        if manifest_loading {
            ui.weak("The game manifest is loading.");
        }
        ui.add_space(16.0);
        if ui
            .button(format!("Delete all backups of {}", game.title))
            .clicked()
        {
            self.confirm_delete = Some(Deletion {
                title: game.title.clone(),
                snapshots: game.snapshots.clone(),
                all: true,
                focus: true,
            });
        }
    }

    /// Selects `game` with its default snapshot and looks up its default targets.
    fn choose(&mut self, ctx: &egui::Context, state: &State, game: &GameBackups) {
        let Some(config) = &state.config else {
            return;
        };
        self.selected = Some(game.slug.clone());
        self.snapshot = game.default;
        self.pick = None;
        self.error = None;
        self.removed = None;
        self.forget_plan();
        let (title, manifest, config) = (
            game.title.clone(),
            state.manifest_or_empty(),
            config.clone(),
        );
        let environment = state.env.environment.clone();
        self.detecting = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            Ok(restore::default_targets(
                &title,
                &manifest,
                &config,
                &environment,
            ))
        }));
    }

    /// Reads `snapshot`'s listing, or takes the last one read when it is the same snapshot, and
    /// plans its restore into the picked targets. It downloads nothing: the restore downloads
    /// into a work dir under the cache dir (ADR-0020).
    fn start_plan(
        &mut self,
        ctx: &egui::Context,
        state: &State,
        title: String,
        snapshot: SnapshotInfo,
    ) {
        let (Some(session), Some(config), Some(pick)) = (&state.session, &state.config, &self.pick)
        else {
            return;
        };
        let targets = pick.targets();
        self.error = None;
        let (storage, manifest, config) = (
            session.storage.clone(),
            state.manifest_or_empty(),
            config.clone(),
        );
        let environment = state.env.environment.clone();
        let work_dir = own_dir(&state.env.cache_dir, "restore-work");
        let last_read = (self.last_read.clone()).filter(|listed| listed.snapshot.id == snapshot.id);
        let job = Job::spawn(ctx, move || {
            let targets = targets()?;
            let listed = match last_read {
                Some(listed) => listed,
                None => Arc::new(restore::read(
                    &mut *lock_storage(&storage)?,
                    &snapshot,
                    &work_dir,
                )?),
            };
            let info = restore::game_info(&title, &manifest, &config.custom_games);
            let environment = environment.unwrap_or_else(Environment::detect);
            let fetched = Fetched {
                storage,
                vars: restore::game_vars(&title, info.entry.as_ref(), &config, &environment),
                title,
                listed,
                targets,
                info,
            };
            Ok(Prepared {
                plan: fetched.labelled(fetched.prepare(&[])),
                fetched: Arc::new(fetched),
                choices: Vec::new(),
            })
        });
        self.preparing = Some((self.generation, job));
    }

    fn mapping_ui(&mut self, ctx: &egui::Context, state: &State) {
        let (busy, error) = (self.busy(), self.error.clone());
        let Some((fetched, dialog)) = (self.mapping.as_mut()).filter(|_| !self.dialog_hidden)
        else {
            return;
        };
        let action = theme::modal(ctx, "restore_mapping", 720.0, |ui| {
            let offer_prefix = fetched.targets.win.is_none();
            dialog.ui(
                ui,
                &fetched.title,
                busy,
                offer_prefix,
                false,
                error.as_deref(),
            )
        });
        match action {
            Some(DialogAction::Confirm(choices)) if state.session.is_some() => {
                let fetched = fetched.clone();
                let job = Job::spawn(ctx, move || {
                    Ok(Prepared {
                        plan: fetched.labelled(fetched.prepare(&choices)),
                        fetched,
                        choices,
                    })
                });
                self.preparing = Some((self.generation, job));
            }
            Some(DialogAction::UsePrefix) => {
                self.forget_plan();
                if let Some(pick) = &mut self.pick {
                    pick.use_linux = false;
                    pick.use_prefix = true;
                }
            }
            Some(DialogAction::Cancel) => self.dialog_hidden = true,
            Some(DialogAction::Confirm(_) | DialogAction::SkipGame) | None => {}
        }
    }

    /// The confirm dialog of a delete; Delete starts it in the listed repository.
    fn delete_ui(&mut self, ctx: &egui::Context) {
        let Some(deletion) = &mut self.confirm_delete else {
            return;
        };
        let (mut cancel, mut delete) = (false, false);
        theme::modal(ctx, "restore_delete", 560.0, |ui| {
            let backups = count(deletion.snapshots.len(), "backup");
            ui.heading(format!("Delete {backups} of {}?", deletion.title));
            for snapshot in &deletion.snapshots {
                ui.label(snapshot_label(snapshot));
            }
            ui.label(
                "This removes them from the repository for every computer. It cannot be undone.",
            );
            if deletion.all {
                ui.weak("Your file choices for this game are kept.");
            }
            let error = ui.visuals().error_fg_color;
            theme::row_end(ui, |ui| {
                let button = ui.button("Cancel");
                // `row_end` first runs this in a sizing pass, whose widgets are not the real ones.
                if !ui.is_sizing_pass() && mem::take(&mut deletion.focus) {
                    button.request_focus();
                }
                cancel |= button.clicked();
                delete |= ui
                    .button(RichText::new("Delete").color(error).strong())
                    .clicked();
            });
        });
        if delete {
            self.start_delete(ctx);
        } else if cancel {
            self.confirm_delete = None;
        }
    }

    /// Deletes the backups of the confirm dialog in a job holding the storage lock.
    fn start_delete(&mut self, ctx: &egui::Context) {
        let (Some(deletion), Some(storage)) = (self.confirm_delete.take(), &self.repository) else {
            return;
        };
        let storage = storage.clone();
        let ids: Vec<String> = deletion.snapshots.into_iter().map(|s| s.id).collect();
        self.error = None;
        self.removed = None;
        self.deleting = Some(Job::spawn(ctx, move || {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            Ok(retention::delete(&mut *lock_storage(&storage)?, &ids)?)
        }));
    }

    fn preview_ui(&mut self, ui: &mut Ui, state: &State) {
        let busy = self.busy();
        let Some(preview) = &mut self.preview else {
            let mut choose = false;
            let mut retry = false;
            theme::row(
                ui,
                |ui| self.status_ui(ui, &mut choose, &mut retry),
                |ui| restore_button(ui, false),
            );
            if choose {
                self.dialog_hidden = false;
            }
            if retry {
                self.error = None;
                self.forget_plan();
            }
            return;
        };
        ui.add_space(16.0);
        ui.heading("Plan");
        let error_color = ui.visuals().error_fg_color;
        let muted = ui.visuals().weak_text_color();
        warnings_ui(ui, &preview.plan);
        let compared: Vec<_> = (preview.labels.iter())
            .filter_map(|label| match label {
                Label::State(state) => Some(*state),
                Label::WillFail(_) | Label::Blocks(_) => None,
            })
            .collect();
        let writes = preview.plan.writes.len();
        theme::row(
            ui,
            |ui| {
                ui.label(format!("Files to write ({writes})"));
            },
            |ui| batch::state_chips(ui, &compared),
        );
        let blocking: Vec<_> = (preview.plan.writes.iter().zip(&preview.labels))
            .filter(|(_, label)| matches!(label, Label::Blocks(_)))
            .map(|(write, _)| write.target.display().to_string())
            .collect();
        if !blocking.is_empty() {
            let text = "Nothing will be restored: the safety backup cannot read these files:";
            theme::status(ui, icons::X_CIRCLE, error_color, text);
            for path in &blocking {
                ui.weak(path);
            }
        }
        let files = preview.fetched.listed.staged.root.join(FILES_DIR);
        let mut overridden = None;
        theme::group(ui, |ui| {
            for (index, write) in preview.plan.writes.iter().enumerate() {
                if index > 0 {
                    theme::row_separator(ui);
                }
                let number = index + 1;
                // `<Base>/<tail>` with `/` on every OS, as the snapshot stores it.
                let source = write.source.strip_prefix(&files).unwrap_or(&write.source);
                let source = (source.iter().map(|part| part.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join("/");
                let flag = match &write.target_file {
                    None => " (overridden for this restore)",
                    Some((Base::Absolute | Base::WinAbsolute, _)) => " (absolute path)",
                    Some(_) => "",
                };
                ui.horizontal(|ui| {
                    ui.label(source);
                    let state = match &preview.labels[index] {
                        Label::State(state) => Ok(*state),
                        Label::WillFail(reason) | Label::Blocks(reason) => Err(reason.as_str()),
                    };
                    if let Some((badge, hint)) = batch::write_badge(state) {
                        theme::labeled_mark(ui, badge.glyph(), badge.color(ui), &hint);
                    }
                });
                let target = format!("{}{flag}", write.target.display());
                theme::icon_label(ui, icons::ARROW_RIGHT, muted, target);
                ui.horizontal(|ui| {
                    let label = ui.weak(format!("Override target {number}"));
                    let field = TextEdit::singleline(&mut preview.overrides[index])
                        .hint_text("an absolute path, for this restore only");
                    ui.add(field).labelled_by(label.id);
                    let button = egui::Button::new(format!("Override {number}"));
                    if ui.add_enabled(!busy, button).clicked() {
                        overridden = Some(index);
                    }
                });
                if let Some((_, error)) =
                    (preview.override_error.as_ref()).filter(|(i, _)| *i == index)
                {
                    theme::status(ui, icons::X_CIRCLE, error_color, error);
                }
            }
        });
        if let Some(index) = overridden {
            let target = PathBuf::from(preview.overrides[index].trim());
            preview.override_error = (preview.plan.override_target(index, target))
                .err()
                .map(|err| (index, err.to_string()));
            // One target is a stat and at most one hash, unlike the whole plan's in its job.
            if preview.override_error.is_none() {
                let write = &preview.plan.writes[index];
                preview.labels[index] = Label::of(write, &preview.fetched.listed);
            }
        }
        if let Some((export, ctx)) = &preview.plan.registry {
            ui.label(format!(
                "Registry: {} keys into {}",
                export.keys.len(),
                describe_context(ctx)
            ));
        }
        skipped_ui(ui, &preview.plan);
        // Typed text that is not the write's target would be silently ignored by the restore.
        let unapplied =
            (preview.overrides.iter().zip(&preview.plan.writes)).position(|(text, write)| {
                !text.trim().is_empty() && Path::new(text.trim()) != write.target
            });
        if let Some(index) = unapplied {
            let number = index + 1;
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!(
                    "Override target {number} is not applied: click Override {number} or clear it."
                ),
            );
        }
        let blocked = !blocking.is_empty();
        let ready = !busy && state.session.is_some() && unapplied.is_none() && !blocked;
        let restoring = self.restoring.is_some();
        let mut retry = false;
        let start = |ui: &mut Ui| {
            if restoring {
                ui.weak("Downloading and restoring…");
            } else if blocked {
                ui.weak("Make those files readable, then check again.");
                retry |= ui.button("Try again").clicked();
            }
        };
        if theme::row(ui, start, |ui| restore_button(ui, ready)).clicked() {
            self.start_restore(ui.ctx(), state);
        }
        if retry {
            self.forget_plan();
        }
    }

    /// Why there is no plan to restore yet, next to the disabled "Restore files": what to choose
    /// next, the running plan job, the "Choose folders…" button of a plan that needs mapping, or
    /// "Try again" after a failed plan, whose error shows above. Sets `choose` or `retry` when
    /// their button is clicked.
    fn status_ui(&self, ui: &mut Ui, choose: &mut bool, retry: &mut bool) {
        if self.selected.is_none() || self.detecting.is_some() || self.batch.open() {
            return;
        }
        if let Some((_, dialog)) = &self.mapping {
            let unknown = dialog.unknown_roots();
            let folders = count(unknown, "folder");
            ui.weak(format!(
                "This backup has {folders} from the other build without a place here."
            ));
            *choose |= ui.button("Choose folders…").clicked();
        } else if self.preparing.is_some() {
            ui.spinner();
            ui.weak("Reading the backup's file list…");
        } else if self.snapshot.is_none() {
            ui.weak("Choose a snapshot.");
        } else if self.pick.as_ref().is_some_and(|pick| pick.typing) {
            ui.weak("Press Enter to use this prefix.");
        } else if !self.pick.as_ref().is_some_and(TargetPick::ready) {
            ui.weak("Choose where to restore.");
        } else if self.attempted == Some(self.generation) {
            *retry |= ui.button("Try again").clicked();
        }
    }

    /// Downloads the planned snapshot into its cleared work dir, then runs the plan behind a
    /// safety backup staged in its own dir under the cache dir. A failed download writes
    /// nothing, not even the safety backup.
    fn start_restore(&mut self, ctx: &egui::Context, state: &State) {
        let Some(preview) = &self.preview else {
            return;
        };
        self.error = None;
        self.restored = None;
        self.batch.clear_result();
        let (fetched, plan) = (preview.fetched.clone(), preview.plan.clone());
        let stage_root = own_dir(&state.env.cache_dir, "restore-stage");
        let wineserver_running = state.env.wineserver_running;
        self.restoring = Some(Job::spawn(ctx, move || {
            let staged = &fetched.listed.staged;
            remove_dir(&staged.root)
                .with_context(|| format!("cannot clear {}", staged.root.display()))?;
            let mut storage = lock_storage(&fetched.storage)?;
            restore::download(&mut storage, &fetched.listed)
                .context("the backup cannot be fetched")?;
            let title = fetched.title.clone();
            let running = wineserver_running();
            let report = restore::run(&mut storage, &title, &plan, &stage_root, running)?;
            Ok(Restored::new(title, &plan, report))
        }));
    }

    fn restored_ui(&self, ui: &mut Ui) {
        let Some(restored) = &self.restored else {
            return;
        };
        ui.add_space(16.0);
        ui.heading(format!("Restored {}", restored.title));
        restored.lines_ui(ui);
    }
}

/// The cross-build warning and the notes of `plan`.
fn warnings_ui(ui: &mut Ui, plan: &RestorePlan) {
    let warn = ui.visuals().warn_fg_color;
    if plan.cross_build {
        theme::status(
            ui,
            icons::WARNING,
            warn,
            "Cross-build restore: the other build may use a different save format. The safety \
             backup lets you undo it.",
        );
    }
    for note in &plan.notes {
        theme::status(ui, icons::WARNING, warn, format!("Note: {note}"));
    }
}

/// The items `plan` does not restore, with why.
fn skipped_ui(ui: &mut Ui, plan: &RestorePlan) {
    if plan.skipped.is_empty() {
        return;
    }
    ui.label(format!("Skipped ({})", plan.skipped.len()));
    for skipped in &plan.skipped {
        let item = match &skipped.item {
            SkippedItem::File(base, tail) => format!("{}/{tail}", base.dir_name()),
            SkippedItem::Registry => "registry".to_owned(),
            SkippedItem::RegistryKey(key) => key.clone(),
            _ => "an item".to_owned(),
        };
        ui.weak(format!("{item}: {}", skipped.reason));
    }
}

/// The primary button that runs the previewed plan.
fn restore_button(ui: &mut Ui, enabled: bool) -> Response {
    let glyph = Some(icons::CLOCK_COUNTER_CLOCKWISE);
    theme::primary(ui, enabled, glyph, "Restore files")
}

/// What a failed restore job tells the user; nothing was written.
fn describe_run_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<RunError>() {
        Some(RunError::WineserverRunning) => "Nothing was restored: a Wine or Proton program is \
             running. Close the game and Wine first, then restore again."
            .to_owned(),
        _ => format!("Nothing was restored: {err:#}"),
    }
}

/// What a failed delete job tells the user, with how many backups it removed.
fn describe_delete_error(err: &anyhow::Error) -> String {
    match err.downcast_ref::<DeleteError>() {
        Some(DeleteError::Stopped(stopped)) if stopped.forgotten > 0 => format!(
            "Removed {}, then the delete failed: {}",
            count(stopped.forgotten, "backup"),
            stopped.source
        ),
        Some(DeleteError::Stopped(stopped)) => format!("Nothing was removed: {}", stopped.source),
        Some(DeleteError::NotAGameBackup(_)) => {
            "The list was out of date, so nothing was removed".to_owned()
        }
        None => format!("Nothing was removed: {err:#}"),
        Some(_) => format!("The delete failed: {err:#}"),
    }
}

fn describe_context(ctx: &Context) -> String {
    match ctx {
        Context::WinePrefix { prefix, .. } => format!("the prefix {}", prefix.display()),
        Context::WindowsNative(_) => "the Windows registry".to_owned(),
        Context::Linux(_) => "Linux".to_owned(),
        _ => "the target".to_owned(),
    }
}

/// `<cache_dir>/<name>/<process id>`, so two running apps never clear each other's files.
fn own_dir(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir.join(name).join(std::process::id().to_string())
}

/// Removes `dir` and everything below it; a missing dir is fine. A restored dir can be
/// read-only, which would block removing its entries, so every real dir is made writable first.
fn remove_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    make_writable(dir)?;
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Adds owner write and search to `path` and every dir below it, without following symlinks,
/// so nothing outside `path` changes.
#[cfg(unix)]
fn make_writable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        metadata => metadata?,
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    let mode = metadata.permissions().mode() | 0o700;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    for entry in fs::read_dir(path)? {
        make_writable(&entry?.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
