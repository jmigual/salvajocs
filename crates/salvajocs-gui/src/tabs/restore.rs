//! The Restore tab: the games in the repository with their snapshots from every host, the build
//! to restore into, the plan read from the snapshot's file list and the restore behind a safety
//! backup (ADR-0005, ADR-0010, ADR-0011, ADR-0020). Backups can also be deleted from the
//! repository (ADR-0015).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, anyhow};
use eframe::egui::{self, Response, RichText, Role, TextEdit, Ui, Vec2, WidgetInfo};
use salvajocs_core::config::ListSort;
use salvajocs_core::crossbuild::{Choice, SnapshotRoot};
use salvajocs_core::layout::{Context, GameVars, LinuxFolders, PrefixOrigin, WinFolders};
use salvajocs_core::prefix;
use salvajocs_core::restore::{
    self, DefaultTargets, GameBackups, GameInfo, Listed, PRE_RESTORE_TAG, PrefixReason,
    PrepareError, RestorePlan, RestoreReport, RunError, SafetyBackup, SkippedItem, Targets, Used,
    Write, WriteState,
};
use salvajocs_core::retention::{self, Cleanup, DeleteError};
use salvajocs_core::scan::Environment;
use salvajocs_core::selection::GameSettings;
use salvajocs_core::stats::{self, BackupStats};
use salvajocs_core::storage::SnapshotInfo;

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::tabs::details::Details;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::tabs::sort::{self, Line};
use crate::tabs::space::{Polled, Space};
use crate::tabs::tree::{Badge, Clicked, FileTree, Mode, Row, Slot};
use crate::theme::{self, icons};
use crate::widgets::named_button;

mod batch;
mod keys;

use batch::Batch;
use keys::KeyTree;

/// The width of the games sidebar at rest, and the least it can be dragged to.
const SIDEBAR_WIDTH: f32 = 240.0;

/// The width the plan beside the games sidebar keeps, which the sidebar cannot be dragged into.
const PLAN_WIDTH: f32 = 600.0;

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
    /// Whether the user changed the pick, which another snapshot's default then keeps.
    edited: bool,
    /// Why the default chose `found`, and its launcher's name for it (ADR-0023).
    reason: Option<PrefixReason>,
    origin: Option<PrefixOrigin>,
    /// The prefixes that all hold the backup's save folders, so none was chosen.
    ambiguous: Vec<PathBuf>,
    /// The prefixes the backup's prefix name fits when none holds its saves.
    stored_several: Vec<PathBuf>,
    /// The prefix the backup names when this machine does not have it.
    missing: Option<PrefixOrigin>,
}

impl TargetPick {
    /// The pick `defaults` preselect; `missing` is the prefix the snapshot names when this
    /// machine does not have it.
    fn new(defaults: DefaultTargets, missing: Option<PrefixOrigin>) -> Self {
        let targets = defaults.targets;
        let mut pick = Self {
            windows: None,
            use_linux: targets.linux.is_some(),
            linux: targets.linux,
            use_prefix: false,
            prefix: String::new(),
            typing: false,
            found: None,
            edited: false,
            reason: defaults.reason,
            origin: defaults.origin,
            ambiguous: defaults.ambiguous,
            stored_several: defaults.stored_several,
            missing,
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

    /// Why the default chose the prefix, while the field still shows it.
    fn default_reason(&self) -> Option<PrefixReason> {
        let shown = (self.found.as_ref()).is_some_and(|(text, _)| *text == self.prefix);
        self.reason.filter(|_| shown)
    }

    /// What the pick says about its prefix: why the default filled it in, a prefix the backup
    /// names that this machine lacks, the prefixes that all hold the saves, and those the
    /// backup's prefix name fits.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some(reason) = self.default_reason() {
            let text = match reason {
                PrefixReason::Backup => "Filled in from the backup",
                PrefixReason::Saves => "Filled in: the backup's save folders are in this prefix",
                PrefixReason::CustomGame => "Filled in from the custom game's Wine prefix",
                _ => "Filled in: the game's own prefix",
            };
            notes.push(match self.origin.as_ref().and_then(prefix_name) {
                Some(name) => format!("{text} ({name})"),
                None => text.to_owned(),
            });
        }
        if let Some(name) = self.missing.as_ref().and_then(prefix_name) {
            notes.push(format!(
                "The backup came from {name}, which this machine does not have or does not use."
            ));
        }
        let lists = [
            (&self.ambiguous, "The backup's save folders are in"),
            (&self.stored_several, "The backup's prefix name fits"),
        ];
        for (prefixes, text) in lists.into_iter().filter(|(list, _)| !list.is_empty()) {
            let paths: Vec<String> = (prefixes.iter())
                .map(|path| path.display().to_string())
                .collect();
            notes.push(format!(
                "{text} {} prefixes: {}. Type or browse to the one to use.",
                paths.len(),
                paths.join(", ")
            ));
        }
        notes
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
        for note in self.notes() {
            ui.weak(note);
        }
        ui.weak(
            "Saves of the other build are mapped onto the chosen one (cross-build restore); \
             with both chosen, each goes into its own.",
        );
        self.edited |= changed;
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
    /// The planned prefix as its launcher names it (ADR-0023).
    prefix_origin: Option<PrefixOrigin>,
    /// Why the default chose the planned prefix; `None` once the user picked another.
    reason: Option<PrefixReason>,
    vars: GameVars,
    info: GameInfo,
}

impl Fetched {
    fn prepare(&self, choices: &[(SnapshotRoot, Choice)]) -> Result<RestorePlan, PrepareError> {
        restore::prepare(&self.listed, &self.targets, &self.vars, &self.info, choices)
    }

    /// `plan` with how each write compares with its target, the tree of its writes and the tree
    /// of its registry keys. Reads the targets and which target folders exist, so it runs in a
    /// job.
    fn labelled(&self, plan: Result<RestorePlan, PrepareError>) -> Result<Labelled, PrepareError> {
        plan.map(|plan| {
            let labels = (plan.writes.iter())
                .map(|write| Label::of(write, &self.listed))
                .collect();
            let tree = WriteTree::new(&plan, &self.listed);
            let keys = KeyTree::of(&plan);
            Labelled {
                plan,
                labels,
                tree,
                keys,
            }
        })
    }
}

/// A plan with how each write compares with its target, its writes as a tree and its registry
/// keys as a tree.
struct Labelled {
    plan: RestorePlan,
    labels: Vec<Label>,
    tree: WriteTree,
    keys: KeyTree,
}

/// A plan's writes as a tree keyed by where they land as planned, with a checkbox on each file
/// and folder (ADR-0025). Its row positions are the write indices. An overridden write keeps its
/// row, so it stays where the user is.
struct WriteTree {
    tree: FileTree,
    /// Each write's planned `<Base>/<tail>` key, which its checkbox ticks; an override keeps it.
    keys: Vec<String>,
    /// What each write's controls are named after: its key, and its row number when another
    /// write lands on the same key, so screen readers tell them apart.
    names: Vec<String>,
}

impl WriteTree {
    /// The tree of `plan`'s writes, sized from the listing `listed`. Reads which target folders
    /// exist, so it runs in a job.
    fn new(plan: &RestorePlan, listed: &Listed) -> Self {
        let files: Vec<Row> = (plan.writes.iter())
            .map(|write| Row {
                key: target_key(write).expect("`restore::plan` gives every write its target's key"),
                size: listed.source(write).map_or(0, |entry| entry.size),
                real: Some(write.target.clone()),
                removed: false,
            })
            .collect();
        let mut shared = HashMap::<&str, usize>::new();
        for file in &files {
            *shared.entry(&file.key).or_default() += 1;
        }
        let names = (files.iter().enumerate())
            .map(|(index, file)| match shared[file.key.as_str()] {
                1 => file.key.clone(),
                _ => format!("{} ({})", file.key, index + 1),
            })
            .collect();
        let keys = files.iter().map(|file| file.key.clone()).collect();
        Self {
            tree: FileTree::new(files).existing_folders_only(),
            keys,
            names,
        }
    }

    /// Whether `selection` ticks each write, by write index.
    fn ticked(&self, selection: &GameSettings) -> Vec<bool> {
        (self.keys.iter())
            .map(|key| selection.path_included(key))
            .collect()
    }
}

/// The `<Base>/<tail>` key of `write`'s target, the absolute one once overridden; `None` when
/// the target has none.
fn target_key(write: &Write) -> Option<String> {
    let ((base, tail), _) = restore::staged_target(write)?;
    Some(format!("{}/{tail}", base.dir_name()))
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

    /// The icon of the write in the plan with its tooltip; `None` for an unchanged one.
    fn badge(&self) -> Option<(Badge, String)> {
        batch::write_badge(match self {
            Label::State(state) => Ok(*state),
            Label::WillFail(reason) | Label::Blocks(reason) => Err(reason.as_str()),
        })
    }
}

/// What a detection job hands back: the default targets of the chosen snapshot, and its listing
/// when it could be read.
struct Detected {
    /// The id of the snapshot detected for; `None` when none was chosen.
    snapshot: Option<String>,
    listed: Option<Arc<Listed>>,
    defaults: DefaultTargets,
}

/// What a plan job hands back.
struct Prepared {
    fetched: Arc<Fetched>,
    /// The dialog's choices this plan was prepared with; empty for the first try.
    choices: Vec<(SnapshotRoot, Choice)>,
    plan: Result<Labelled, PrepareError>,
}

/// The plan shown before anything is written.
struct Preview {
    fetched: Arc<Fetched>,
    plan: RestorePlan,
    /// The writes as planned, which clearing an override puts back.
    planned: Vec<Write>,
    /// How each write compares with its target.
    labels: Vec<Label>,
    tree: WriteTree,
    /// The registry keys with their ticks.
    keys: KeyTree,
    /// Which files the restore writes, by planned key, all ticked at first. Never saved; a new
    /// plan starts with a new one.
    selection: GameSettings,
    /// The text of each write whose override field is open.
    editing: BTreeMap<usize, String>,
    /// The write whose override was refused, with why.
    override_error: Option<(usize, String)>,
}

/// What a file row of the plan was clicked for.
enum RowAction {
    /// Its Override button: opens or closes its field.
    Toggle(usize),
    /// Its field's Apply button.
    Apply(usize),
    /// Its Clear button: back to the planned target.
    Clear(usize),
}

impl Preview {
    fn new(fetched: Arc<Fetched>, labelled: Labelled) -> Self {
        let Labelled {
            plan,
            labels,
            tree,
            keys,
        } = labelled;
        Self {
            fetched,
            planned: plan.writes.clone(),
            plan,
            labels,
            tree,
            keys,
            selection: GameSettings::default(),
            editing: BTreeMap::new(),
            override_error: None,
        }
    }

    /// The tree of the writes, with a checkbox on each file and folder. Each file's name and
    /// Override button show where it goes on hover; the button opens a field under it for a new
    /// target, and an overridden file shows its target with a Clear button. A file whose target
    /// differs also has a Details button. Applies what was clicked and returns the folder the
    /// user asked to open and the write whose details they asked for.
    fn files_ui(&mut self, ui: &mut Ui, busy: bool) -> (Option<PathBuf>, Option<usize>) {
        let Preview {
            fetched,
            plan,
            planned,
            labels,
            tree: WriteTree { tree, names, .. },
            selection,
            editing,
            override_error,
            ..
        } = self;
        let badge = |index: usize, _: &str| labels[index].badge().map(|(badge, _)| badge);
        let hint = |index: usize, _: &str, _| {
            (labels[index].badge())
                .map(|(_, hint)| hint)
                .unwrap_or_default()
        };
        let (error_color, muted) = (ui.visuals().error_fg_color, ui.visuals().weak_text_color());
        let mut action = None;
        let mut details = None;
        let mut row = |ui: &mut Ui, index: usize, _: &str, slot: Slot<'_>| {
            let (write, name) = (&plan.writes[index], &names[index]);
            let target = write.target.display().to_string();
            if let Slot::End(shown) = slot {
                shown.clone().on_hover_text(&target);
                if matches!(labels[index], Label::State(WriteState::Changed)) {
                    let button = ui.add_enabled(!busy, egui::Button::new("Details").small());
                    let label = format!("Details of {name}");
                    button.widget_info(|| WidgetInfo::labeled(Role::Button, !busy, &label));
                    if button.clicked() {
                        details = Some(index);
                    }
                }
                // Small, so the row keeps the height of the batch confirm's.
                let button = ui.add_enabled(!busy, egui::Button::new("Override").small());
                let label = format!("Override {name}");
                button.widget_info(|| WidgetInfo::labeled(Role::Button, !busy, &label));
                if button.on_hover_text(&target).clicked() {
                    action = Some(RowAction::Toggle(index));
                }
                return;
            }
            if write.target_file.is_none() {
                ui.horizontal(|ui| {
                    let text = format!("{target} (overridden for this restore)");
                    theme::icon_label(ui, icons::ARROW_RIGHT, muted, text);
                    let label = format!("Clear the override of {name}");
                    if named_button(ui, !busy, "Clear", &label).clicked() {
                        action = Some(RowAction::Clear(index));
                    }
                });
            }
            if let Some(text) = editing.get_mut(&index) {
                ui.horizontal(|ui| {
                    let label = ui.weak(format!("New target of {name}"));
                    let field = TextEdit::singleline(text)
                        .hint_text("an absolute path, for this restore only");
                    ui.add(field).labelled_by(label.id);
                    let label = format!("Apply the new target of {name}");
                    if named_button(ui, !busy, "Apply", &label).clicked() {
                        action = Some(RowAction::Apply(index));
                    }
                });
            }
            if let Some((_, error)) = (override_error.as_ref()).filter(|(i, _)| *i == index) {
                theme::status(ui, icons::X_CIRCLE, error_color, error);
            }
        };
        let included = |key: &str| selection.path_included(key);
        let mode = Mode::Select {
            included: &included,
            enabled: !busy,
        };
        let clicked = ui.push_id(("restore_plan", &fetched.title), |ui| {
            tree.ui_with_rows(ui, mode, &badge, &hint, &mut row)
        });
        // An action ends the field's error: a new try, a closed field or the planned target.
        if let Some(RowAction::Toggle(index) | RowAction::Apply(index) | RowAction::Clear(index)) =
            action
            && override_error.as_ref().is_some_and(|(i, _)| *i == index)
        {
            *override_error = None;
        }
        let changed = match action {
            Some(RowAction::Toggle(index)) => {
                if editing.remove(&index).is_none() {
                    editing.insert(index, String::new());
                }
                None
            }
            Some(RowAction::Apply(index)) => {
                let target = PathBuf::from(editing.get(&index).map_or("", |text| text.trim()));
                match plan.override_target(index, target) {
                    Ok(()) => {
                        editing.remove(&index);
                        Some(index)
                    }
                    Err(err) => {
                        *override_error = Some((index, err.to_string()));
                        None
                    }
                }
            }
            Some(RowAction::Clear(index)) => {
                plan.writes[index] = planned[index].clone();
                Some(index)
            }
            None => None,
        };
        // One target is a stat and at most one hash, unlike the whole plan's in its job.
        if let Some(index) = changed {
            labels[index] = Label::of(&plan.writes[index], &fetched.listed);
            tree.invalidate();
        }
        let opened = match clicked.inner {
            Some(Clicked::Open(path)) => Some(path),
            Some(Clicked::Toggle(key, included)) => {
                selection.set_path(&key, included);
                tree.invalidate();
                None
            }
            None => None,
        };
        (opened, details)
    }

    /// The registry line, counting the ticked keys, and the tree of the keys.
    fn registry_ui(&mut self, ui: &mut Ui, busy: bool) {
        let Some((_, ctx)) = &self.plan.registry else {
            return;
        };
        let (name, hover) = name_context(ctx, self.fetched.prefix_origin.as_ref());
        let keys = self.keys.ticked_count();
        hover_label(ui, format!("Registry: {keys} keys into {name}"), hover);
        self.keys.ui(ui, !busy);
    }
}

/// `plan` with only what is ticked (ADR-0025, ADR-0026): the writes whose planned key (`keys`,
/// by write index) `selection` includes, and the registry keys `registry` ticks with the keys
/// it unticks kept, or no registry when nothing is written. The restore backs up and writes
/// this one plan, so nothing unticked is written.
fn ticked_plan(
    plan: &RestorePlan,
    keys: &[String],
    registry: &KeyTree,
    selection: &GameSettings,
) -> RestorePlan {
    let mut ticked = plan.clone();
    let mut keys = keys.iter();
    (ticked.writes).retain(|_| keys.next().is_some_and(|key| selection.path_included(key)));
    if let Some((export, _)) = &mut ticked.registry {
        (*export, ticked.registry_kept) = registry.select(export);
    }
    // An empty export would still refuse the restore while a wineserver runs.
    if (ticked.registry.as_ref()).is_some_and(|(export, _)| export.keys.is_empty()) {
        ticked.registry = None;
        ticked.registry_kept.clear();
    }
    ticked
}

/// Whether `plan` has neither a file to write nor a registry key, so nothing can be ticked.
fn nothing_to_restore(plan: &RestorePlan) -> bool {
    plan.writes.is_empty() && plan.registry.is_none()
}

/// The targets of the ticked writes (`ticked`, by write index) the safety backup cannot read,
/// any of which keeps the whole restore from running.
fn blocking(plan: &RestorePlan, labels: &[Label], ticked: &[bool]) -> Vec<String> {
    (plan.writes.iter().zip(labels).zip(ticked))
        .filter(|((_, label), ticked)| **ticked && matches!(label, Label::Blocks(_)))
        .map(|((write, _), _)| write.target.display().to_string())
        .collect()
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
                }
                Err(err) => self.error = Some(format!("The backups cannot be listed: {err:#}")),
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
            Ok(labelled) => {
                if !choices.is_empty() {
                    state.apply_mapping_choices(ctx, &fetched.title, &choices);
                }
                self.mapping = None;
                self.preview = Some(Preview::new(fetched, labelled));
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

    /// Starts working out the space of the listed games' backups that the session has not yet.
    fn start_space(&mut self, ctx: &egui::Context, state: &State) {
        if let Some(session) = &state.session {
            let games = (self.games.iter())
                .map(|game| BackupStats::of(&game.snapshots).snapshot_ids)
                .collect();
            let space = Space::start(ctx, &session.storage, &session.space, games);
            self.space = Some(space);
        }
        self.list_changes += 1;
    }

    /// Builds the figures and the order of the games by `sort`, and drops their lines, when the
    /// sort, the listing or the space used changed since they were last built.
    fn sort_rows(&mut self, sort: ListSort) {
        if self.sorted == Some((sort, self.list_changes)) {
            return;
        }
        let space = self.space.as_ref();
        self.figures = (self.games.iter())
            .map(|game| {
                let mut figures = BackupStats::of(&game.snapshots);
                let space_used = space.and_then(|space| space.get(&figures.snapshot_ids));
                figures.space_used = space_used.and_then(Result::ok);
                figures
            })
            .collect();
        let rows: Vec<_> = (self.games.iter().zip(&self.figures))
            .map(|(game, figures)| (game.title.as_str(), Some(figures)))
            .collect();
        self.order = stats::order(&rows, sort);
        self.lines = vec![None; self.games.len()];
        self.sorted = Some((sort, self.list_changes));
    }

    fn start_listing(&mut self, ctx: &egui::Context, state: &State) {
        if self.read_list(ctx, state) {
            self.selected = None;
            self.pick = None;
        }
    }

    /// Lists the backups again after a Back up saved snapshots, keeping the chosen game and
    /// snapshot, the plan with its edits, the targets and the ticked games: a backup removes
    /// no snapshot. Does nothing before the first listing, and while a restore or delete runs,
    /// which list again when they end.
    pub(crate) fn relist(&mut self, ctx: &egui::Context, state: &State) {
        let writing = self.restoring.is_some() || self.deleting.is_some() || self.batch.writing();
        if self.repository.is_some() && !writing {
            self.read_list(ctx, state);
        }
    }

    /// Starts the listing job; returns whether the repository is open to list.
    fn read_list(&mut self, ctx: &egui::Context, state: &State) -> bool {
        let Some(session) = &state.session else {
            return false;
        };
        let storage = session.storage.clone();
        self.repository = Some(storage.clone());
        self.listing = Some(Job::spawn(ctx, move || {
            Ok(restore::list(&*lock_storage(&storage)?)?)
        }));
        true
    }

    /// Before the first listing: what the tab is for and the listing as its one action.
    fn empty_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            ui.weak("See the games in your repository and their snapshots.");
            ui.add_space(8.0);
            let session = state.session.is_some();
            let enabled = session && !self.backing_up;
            if theme::primary(ui, enabled, None, "Load backups").clicked() {
                self.load(ui.ctx(), state);
            }
            if !session {
                ui.weak("The repository is not open.");
            }
            if manifest_loading {
                ui.weak("The game manifest is loading.");
            }
            if self.backing_up {
                ui.weak("A backup is running.");
            }
        });
    }

    fn load(&mut self, ctx: &egui::Context, state: &State) {
        self.error = None;
        self.removed = None;
        self.forget_plan();
        self.start_listing(ctx, state);
    }

    /// The sidebar: the listing button, the Sort by row and the games of the repository in the
    /// config's order, each with a line of its backup figures. Only `idle` enables the controls:
    /// the list scrolls either way, which a disabled `Ui` would not allow.
    fn games_ui(&mut self, ui: &mut Ui, state: &mut State, idle: bool) {
        let sort = ui
            .add_enabled_ui(idle, |ui| self.sidebar_header_ui(ui, state))
            .inner;
        let Some(sort) = sort else {
            return;
        };
        // The batch button keeps its room under the list, so a long list cannot push it off.
        let reserve = self.batch_button_room(ui);
        let chosen = egui::ScrollArea::vertical()
            .id_salt("restore_games_list")
            .auto_shrink([false, true])
            .max_height((ui.available_height() - reserve).max(0.0))
            .show(ui, |ui| {
                ui.add_enabled_ui(idle, |ui| self.games_list_ui(ui, sort))
                    .inner
            })
            .inner;
        if let Some(game) = chosen {
            self.choose(ui.ctx(), state, &game);
        }
        ui.add_enabled_ui(idle, |ui| self.batch_button_ui(ui, state));
    }

    /// The listing button and, once there are games, the select-all row and the sort control.
    /// Returns the sort to list by, or `None` while there are no games.
    fn sidebar_header_ui(&mut self, ui: &mut Ui, state: &mut State) -> Option<ListSort> {
        let load = egui::Button::new("Load backups").min_size(Vec2::new(ui.available_width(), 0.0));
        let enabled = state.session.is_some() && !self.backing_up;
        if ui.add_enabled(enabled, load).clicked() {
            self.load(ui.ctx(), state);
        }
        if self.backing_up {
            ui.weak("A backup is running.");
        }
        if state.session.is_none() {
            ui.weak("The repository is not open.");
        }
        if self.games.is_empty() {
            return None;
        }
        self.select_all_ui(ui);
        let sort = (state.config.as_ref()).map_or(ListSort::default(), |c| c.restore_sort);
        if let Some(picked) = sort::sort_ui(ui, "restore_sort", sort)
            && let Some(config) = &mut state.config
        {
            config.restore_sort = picked;
            state.save_config(ui.ctx());
        }
        self.sort_rows(sort);
        Some(sort)
    }

    /// The boxed list of games; returns the one whose row was clicked.
    fn games_list_ui(&mut self, ui: &mut Ui, sort: ListSort) -> Option<GameBackups> {
        let mut chosen = None;
        let (figures, space, lines) = (&self.figures, &self.space, &mut self.lines);
        theme::group(ui, |ui| {
            for &index in &self.order {
                let game = &self.games[index];
                let selected = self.selected.as_ref() == Some(&game.slug);
                let line = lines[index].get_or_insert_with(|| {
                    let figures = &figures[index];
                    let space_used = space.as_ref().and_then(|s| s.get(&figures.snapshot_ids));
                    sort::restore_line(figures, space_used, sort.by)
                });
                ui.scope(|ui| {
                    // No gap and no row height, so the figures sit right under their title.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let title = ui.horizontal(|ui| {
                        batch::include_ui(ui, &mut self.checked, game);
                        // The pill list drops the outline of the buttons, which the checkbox
                        // keeps.
                        ui.scope(|ui| {
                            theme::pill_list(ui);
                            // The grow atom left-aligns the title in the row, which the title
                            // truncates to.
                            let title = (game.title.as_str(), egui::Atom::grow());
                            let row = egui::Button::selectable(selected, title)
                                .truncate()
                                .min_size(Vec2::new(ui.available_width(), 0.0));
                            let row = ui.add(row).on_hover_text(&game.title);
                            if row.clicked() && !selected {
                                chosen = Some(game.clone());
                            }
                        })
                        .response
                        .rect
                        .left()
                    });
                    ui.spacing_mut().interact_size.y = 0.0;
                    ui.horizontal(|ui| {
                        let indent = title.inner - ui.cursor().left();
                        ui.add_space(indent + ui.spacing().button_padding.x);
                        if line.compact_ui(ui).clicked() && !selected {
                            chosen = Some(game.clone());
                        }
                    });
                });
            }
        });
        chosen
    }

    /// The chosen game: its snapshot and the build to restore into.
    fn game_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
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
            let deletable = chosen.is_some() && !self.backing_up;
            let delete = ui.add_enabled(deletable, egui::Button::new("Delete snapshot"));
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
        let switched = self.snapshot != before;
        let mut changed = switched;
        if let Some(pick) = &mut self.pick {
            changed |= pick.ui(ui);
            if pick.windows.is_none() && ui.button("Browse").clicked() {
                let pick = state.env.pick_folder.clone();
                self.picking = Some(Job::spawn(ui.ctx(), move || Ok(pick())));
            }
        }
        if changed {
            self.forget_plan();
        }
        match &mut self.pick {
            // What the default said is about the other snapshot.
            Some(pick) if switched && pick.edited => {
                pick.reason = None;
                pick.missing = None;
                pick.ambiguous.clear();
                pick.stored_several.clear();
            }
            _ if switched => self.detect(ui.ctx(), state, &game),
            _ => {}
        }
        if manifest_loading {
            ui.weak("The game manifest is loading.");
        }
        if self.backing_up {
            ui.weak("A backup is running.");
        }
        ui.add_space(16.0);
        let delete_all = egui::Button::new(format!("Delete all backups of {}", game.title));
        if ui.add_enabled(!self.backing_up, delete_all).clicked() {
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
        if state.config.is_none() {
            return;
        }
        self.selected = Some(game.slug.clone());
        self.snapshot = game.default;
        self.pick = None;
        self.error = None;
        self.removed = None;
        self.forget_plan();
        self.detect(ctx, state, game);
    }

    /// The chosen snapshot of the chosen game.
    fn chosen(&self) -> Option<&SnapshotInfo> {
        let game = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref())?;
        game.snapshots.get(self.snapshot?)
    }

    /// Works the default targets out again when the chosen snapshot's read failed during
    /// detection and the user has not changed the pick, so a retry can still use its prefix.
    fn detect_unread(&mut self, ctx: &egui::Context, state: &State) {
        let read = (self.chosen()).is_some_and(|snapshot| {
            (self.last_read.as_ref()).is_some_and(|listed| listed.snapshot.id == snapshot.id)
        });
        let edited = self.pick.as_ref().is_some_and(|pick| pick.edited);
        let selected = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref());
        if let (false, false, Some(game)) = (read, edited, selected.cloned()) {
            self.detect(ctx, state, &game);
        }
    }

    /// Reads the chosen snapshot of `game` and works out its default targets, which depend on
    /// the prefix it names and the folders it holds (ADR-0023). The listing is kept for the plan.
    fn detect(&mut self, ctx: &egui::Context, state: &State, game: &GameBackups) {
        let Some(config) = &state.config else {
            return;
        };
        let snapshot = self
            .snapshot
            .and_then(|index| game.snapshots.get(index))
            .cloned();
        let (title, manifest, config) = (
            game.title.clone(),
            state.manifest_or_empty(),
            config.clone(),
        );
        let storage = state
            .session
            .as_ref()
            .map(|session| session.storage.clone());
        let environment = state.env.environment.clone();
        let work_dir = own_dir(&state.env.cache_dir, "restore-work");
        let last_read = self.last_read.clone();
        self.detecting = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            // Without a listing the default is today's; the plan job reads again and says why.
            let listed = snapshot
                .as_ref()
                .zip(storage)
                .and_then(|(snapshot, storage)| {
                    match last_read.filter(|listed| listed.snapshot.id == snapshot.id) {
                        Some(listed) => Some(listed),
                        None => {
                            let mut storage = lock_storage(&storage).ok()?;
                            let listed = restore::read(&mut storage, snapshot, &work_dir).ok()?;
                            Some(Arc::new(listed))
                        }
                    }
                });
            let staged = listed.as_ref().map(|listed| &listed.staged);
            let defaults =
                restore::default_targets(&title, &manifest, &config, &environment, staged);
            Ok(Detected {
                snapshot: snapshot.map(|snapshot| snapshot.id),
                listed,
                defaults,
            })
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
        let (targets, reason) = (pick.targets(), pick.default_reason());
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
            // A typed or browsed prefix is named too, not only the default's.
            let prefix_origin = match &targets.win {
                Some(Context::WinePrefix { prefix, .. }) => {
                    restore::describe_prefix(prefix, &title, &manifest, &config, &environment)
                }
                _ => None,
            };
            let fetched = Fetched {
                storage,
                vars: restore::game_vars(&title, info.entry.as_ref(), &config, &environment),
                title,
                listed,
                targets,
                prefix_origin,
                reason,
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
        let backing_up = self.backing_up;
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
                if backing_up {
                    ui.weak("Waiting for the running backup to end.");
                }
                let button = egui::Button::new(RichText::new("Delete").color(error).strong());
                delete |= ui.add_enabled(!backing_up, button).clicked();
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

    fn preview_ui(&mut self, ui: &mut Ui, state: &mut State) {
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
                self.detect_unread(ui.ctx(), state);
            }
            return;
        };
        ui.add_space(16.0);
        ui.heading("Plan");
        let fetched = &preview.fetched;
        let used = restore::used_targets(&preview.plan, &fetched.targets);
        let (into, hover) = into_line(&used, fetched.prefix_origin.as_ref(), fetched.reason);
        hover_label(ui, into, hover);
        let error_color = ui.visuals().error_fg_color;
        warnings_ui(ui, &preview.plan);
        let ticked = preview.tree.ticked(&preview.selection);
        let compared: Vec<_> = (preview.labels.iter().zip(&ticked))
            .filter_map(|(label, &ticked)| match label {
                Label::State(state) if ticked => Some(*state),
                Label::State(_) | Label::WillFail(_) | Label::Blocks(_) => None,
            })
            .collect();
        let writes = ticked.iter().filter(|&&ticked| ticked).count();
        let left_out = ticked.len() - writes;
        let overridden = (preview.plan.writes.iter())
            .filter(|write| write.target_file.is_none())
            .count();
        theme::row(
            ui,
            |ui| {
                ui.label(format!("Files to write ({writes})"));
                if overridden > 0 {
                    ui.weak(format!("{} overridden", count(overridden, "file")));
                }
                if left_out > 0 {
                    ui.weak(format!("{left_out} left out"));
                }
            },
            |ui| batch::state_chips(ui, &compared),
        );
        let blocking = blocking(&preview.plan, &preview.labels, &ticked);
        if !blocking.is_empty() {
            let text = "Nothing will be restored: the safety backup cannot read these files:";
            theme::status(ui, icons::X_CIRCLE, error_color, text);
            for path in &blocking {
                ui.weak(path);
            }
        }
        let (opened, details) = theme::group(ui, |ui| {
            let clicked = preview.files_ui(ui, busy);
            preview.registry_ui(ui, busy);
            clicked
        });
        if let Some(index) = details {
            let (fetched, write) = (&preview.fetched, &preview.plan.writes[index]);
            // The plan's own snapshot names the game, so a dropped list does not matter.
            let listed = &fetched.listed;
            if let (Some(slug), Some(backed_up)) = (listed.slug(), listed.backed_up(write)) {
                let key = target_key(write).unwrap_or_else(|| write.target.display().to_string());
                let (ctx, storage) = (ui.ctx(), &fetched.storage);
                let slug = slug.to_owned();
                let local = write.target.clone();
                self.details = Some(Details::start(ctx, storage, slug, key, local, backed_up));
            }
        }
        skipped_ui(ui, &preview.plan);
        // Typed text that is not the write's target would be silently ignored by the restore.
        let unapplied = (preview.editing.iter()).find_map(|(&index, text)| {
            let text = text.trim();
            (!text.is_empty() && Path::new(text) != preview.plan.writes[index].target)
                .then_some(index)
        });
        if let Some(index) = unapplied {
            let name = &preview.tree.names[index];
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("The new target of {name} is not applied: click Apply or clear it."),
            );
        }
        let blocked = !blocking.is_empty();
        let nothing = writes == 0 && !preview.keys.any_ticked();
        let ready = !busy
            && !self.backing_up
            && state.session.is_some()
            && unapplied.is_none()
            && !blocked
            && !nothing;
        let restoring = self.restoring.is_some();
        let empty = nothing_to_restore(&preview.plan);
        let has_registry = preview.plan.registry.is_some();
        let mut retry = false;
        let backing_up = self.backing_up;
        let start = |ui: &mut Ui| {
            if restoring {
                ui.weak("Downloading and restoring…");
            } else if backing_up {
                ui.weak("Waiting for the running backup to end.");
            } else if blocked {
                ui.weak("Untick them, or make them readable and check again.");
                retry |= ui.button("Try again").clicked();
            } else if empty {
                ui.weak("Nothing to restore.");
            } else if nothing && has_registry {
                ui.weak("Tick at least one file or registry key to restore.");
            } else if nothing {
                ui.weak("Tick at least one file to restore.");
            }
        };
        if theme::row(ui, start, |ui| restore_button(ui, ready)).clicked() {
            self.start_restore(ui.ctx(), state);
        }
        if retry {
            self.forget_plan();
        }
        if let Some(path) = opened
            && let Err(err) = (state.env.open_folder)(&path)
        {
            let path = path.display();
            state.notices.push(format!("Could not open {path}: {err}"));
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

    /// Downloads the planned snapshot into its cleared work dir, then runs the ticked part of the
    /// plan behind a safety backup staged in its own dir under the cache dir. A failed download
    /// writes nothing, not even the safety backup.
    fn start_restore(&mut self, ctx: &egui::Context, state: &State) {
        let Some(preview) = &self.preview else {
            return;
        };
        self.error = None;
        self.restored = None;
        self.batch.clear_result();
        let plan = ticked_plan(
            &preview.plan,
            &preview.tree.keys,
            &preview.keys,
            &preview.selection,
        );
        let fetched = preview.fetched.clone();
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
            let report =
                restore::run(&mut storage, &title, &plan, &stage_root, wineserver_running)?;
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

/// A prefix by its launcher's name for it (ADR-0023); `None` for a kind this version cannot
/// name.
fn prefix_name(origin: &PrefixOrigin) -> Option<String> {
    Some(match origin {
        PrefixOrigin::Proton(id) => format!("the Steam Proton prefix of app {id}"),
        PrefixOrigin::Heroic(app) => format!("the Heroic prefix of {app}"),
        PrefixOrigin::Lutris(slug) => format!("the Lutris prefix of {slug}"),
        PrefixOrigin::Bottles(bottle) => format!("the Bottles bottle {bottle}"),
        PrefixOrigin::Home(rest) => format!("the Wine prefix {}", rest.replacen("<home>", "~", 1)),
        _ => return None,
    })
}

/// `ctx` as a plan names it, with the prefix's path as hover text when a launcher's name
/// (`origin`) stands for it.
fn name_context(ctx: &Context, origin: Option<&PrefixOrigin>) -> (String, Option<String>) {
    if let Context::WinePrefix { prefix, .. } = ctx
        && let Some(name) = origin.and_then(prefix_name)
    {
        return (name, Some(prefix.display().to_string()));
    }
    (describe_context(ctx), None)
}

/// The "Into …" line of a plan: the targets its writes and registry use (`used`), the prefix
/// named by `origin` and followed by why the default chose it (`reason`), with the prefix's
/// path as hover text.
fn into_line(
    used: &Used,
    origin: Option<&PrefixOrigin>,
    reason: Option<PrefixReason>,
) -> (String, Option<String>) {
    let mut parts = Vec::new();
    let mut hover = None;
    if used.linux {
        parts.push("native Linux".to_owned());
    }
    match &used.win {
        Some(Context::WindowsNative(_)) => parts.push("this Windows user's folders".to_owned()),
        Some(ctx) => {
            let (name, path) = name_context(ctx, origin);
            let why = reason.map_or("", |reason| match reason {
                PrefixReason::Backup => " (from the backup)",
                PrefixReason::Saves => " (its saves are there)",
                PrefixReason::CustomGame => " (custom game)",
                _ => " (the game's own prefix)",
            });
            parts.push(format!("{name}{why}"));
            hover = path;
        }
        None => {}
    }
    if used.install_dir {
        parts.push("the game's install folder".to_owned());
    }
    let into = if parts.is_empty() {
        "no folder on this machine".to_owned()
    } else {
        parts.join(" and ")
    };
    (format!("Into {into}"), hover)
}

/// A label of `text`, with `hover` as its hover text when some.
fn hover_label(ui: &mut Ui, text: String, hover: Option<String>) {
    let label = ui.label(text);
    if let Some(hover) = hover {
        label.on_hover_text(hover);
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
