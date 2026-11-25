use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;
use eframe::egui::{self, Response, Role, TextEdit, Ui, WidgetInfo};
use salvajocs_core::crossbuild::{Choice, SnapshotRoot};
use salvajocs_core::layout::{self, Context, GameVars, PrefixOrigin};
use salvajocs_core::restore::{
    self, Delete, GameInfo, Listed, PRE_RESTORE_TAG, PrefixReason, PrepareError, RestorePlan,
    SkippedItem, Targets, Used, Write, WriteState,
};
use salvajocs_core::scan::Environment;
use salvajocs_core::selection::GameSettings;
use salvajocs_core::storage::SnapshotInfo;

use super::dirs::own_dir;
use super::keys::KeyTree;
use super::manifest_paths::Source;
use super::pick::TargetPick;
use super::run::{selection_of, selections};
use super::{RestoreTab, batch};
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::backup::choices::Saved;
use crate::tabs::count;
use crate::tabs::details::Details;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::tabs::tree::{Badge, Clicked, FileTree, FolderSlot, Mode, Row, Slot};
use crate::theme::{self, icons};
use crate::widgets::named_button;

/// A snapshot's listing and what `restore::prepare` needs besides the user's choices.
pub(super) struct Fetched {
    /// The repository the snapshot came from, which the safety backup goes into.
    pub(super) storage: SharedStorage,
    pub(super) title: String,
    pub(super) listed: Arc<Listed>,
    pub(super) targets: Targets,
    /// The planned prefix as its launcher names it (ADR-0023).
    pub(super) prefix_origin: Option<PrefixOrigin>,
    /// Why the default chose the planned prefix; `None` once the user picked another.
    pub(super) reason: Option<PrefixReason>,
    pub(super) vars: GameVars,
    pub(super) info: GameInfo,
    /// Where `info.entry` comes from, worked out from the same manifest and custom games.
    pub(super) source: Option<Source>,
    /// The game's selection now, with this PC's pending changes, which keeps files from being
    /// deleted; `None` when it cannot be read, so nothing is deleted (ADR-0036).
    pub(super) today: Option<GameSettings>,
}

impl Fetched {
    pub(super) fn prepare(
        &self,
        choices: &[(SnapshotRoot, Choice)],
    ) -> Result<RestorePlan, PrepareError> {
        restore::prepare(&self.listed, &self.targets, &self.vars, &self.info, choices)
    }

    /// `plan` with the files on this PC its backup lacks as its `deletes` (ADR-0036), how each
    /// write compares with its target, the tree of its writes and deletions and the tree of its
    /// registry keys. Lists and reads the targets and which target folders exist, so it runs in
    /// a job.
    pub(super) fn labelled(
        &self,
        plan: Result<RestorePlan, PrepareError>,
    ) -> Result<Labelled, PrepareError> {
        plan.map(|mut plan| {
            let (listed, today) = (&self.listed, self.today.as_ref());
            let found = restore::deletions(listed, &plan, &self.vars, &self.info, today);
            plan.deletes = found.files;
            let unlisted = (found.unlisted.iter())
                .map(|dir| dir.path.display().to_string())
                .collect();
            let labels = (plan.writes.iter())
                .map(|write| Label::of(write, listed))
                .collect();
            let unreadable = (plan.deletes.iter())
                .map(|delete| unreadable(delete, listed))
                .collect();
            let tree = WriteTree::new(&plan, listed);
            let keys = KeyTree::of(&plan);
            Labelled {
                plan,
                labels,
                unreadable,
                unlisted,
                tree,
                keys,
            }
        })
    }
}

/// Why the safety backup cannot read `delete`'s file, which fails the whole restore; `None`
/// when it can.
fn unreadable(delete: &Delete, listed: &Listed) -> Option<String> {
    let mut plan = RestorePlan::new(Vec::new(), None, Vec::new(), false, Vec::new());
    plan.deletes = vec![delete.clone()];
    let compared = restore::compare_writes(&plan, listed);
    compared.err().map(|err| format!("{:#}", anyhow!(err)))
}

/// A plan with how each write compares with its target, which of its deletions the safety
/// backup cannot read, its writes and deletions as a tree and its registry keys as a tree.
pub(super) struct Labelled {
    pub(super) plan: RestorePlan,
    pub(super) labels: Vec<Label>,
    /// Why the safety backup cannot read each of `plan.deletes`, in order; `None` when it can.
    pub(super) unreadable: Vec<Option<String>>,
    /// The target folders that could not be listed, so nothing in them is deleted.
    pub(super) unlisted: Vec<String>,
    pub(super) tree: WriteTree,
    pub(super) keys: KeyTree,
}

/// A plan's writes as a tree keyed by where they land as planned, with a checkbox on each file
/// and folder (ADR-0025). Its row positions are the write indices. An overridden write keeps its
/// row, so it stays where the user is.
pub(super) struct WriteTree {
    pub(super) tree: FileTree,
    /// The rows of `tree`, then one per deletion of the plan, keyed by its file (ADR-0036); shown
    /// while the game's box is on. A deletion's row position is its index in the plan's
    /// `deletes` after the writes. `None` when the plan deletes nothing.
    pub(super) deleting: Option<FileTree>,
    /// Each write's planned `<Base>/<tail>` key, which its checkbox ticks; an override keeps it.
    pub(super) keys: Vec<String>,
    /// Each deletion's `<Base>/<tail>` key, by its index in the plan's `deletes`.
    pub(super) delete_keys: Vec<String>,
    /// What each write's controls are named after: its key, and its row number when another
    /// write lands on the same key, so screen readers tell them apart.
    names: Vec<String>,
}

impl WriteTree {
    /// The tree of `plan`'s writes, sized from the listing `listed`, and with its deletions.
    /// Reads which target folders exist and each deletion's size, so it runs in a job.
    fn new(plan: &RestorePlan, listed: &Listed) -> Self {
        let write_rows = || {
            (plan.writes.iter()).map(|write| Row {
                key: target_key(write).expect("`restore::plan` gives every write its target's key"),
                size: listed.source(write).map_or(0, |entry| entry.size),
                real: Some(write.target.clone()),
                removed: false,
            })
        };
        let files: Vec<Row> = write_rows().collect();
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
        let delete_keys: Vec<String> = (plan.deletes.iter())
            .map(|delete| {
                format!(
                    "{}/{}",
                    delete.target_file.0.dir_name(),
                    delete.target_file.1
                )
            })
            .collect();
        let deleting = (!plan.deletes.is_empty()).then(|| {
            let deletes = (plan.deletes.iter().zip(&delete_keys)).map(|(delete, key)| Row {
                key: key.clone(),
                size: fs::metadata(&delete.target).map_or(0, |meta| meta.len()),
                real: Some(delete.target.clone()),
                removed: false,
            });
            FileTree::new(write_rows().chain(deletes)).existing_folders_only()
        });
        Self {
            tree: FileTree::new(files).existing_folders_only(),
            deleting,
            keys,
            delete_keys,
            names,
        }
    }

    /// Whether `selection` ticks each write, by write index.
    pub(super) fn ticked(&self, selection: &GameSettings) -> Vec<bool> {
        (self.keys.iter())
            .map(|key| selection.path_included(key))
            .collect()
    }

    /// Which deletions the restore runs, by index in the plan's `deletes`: none while the game's
    /// box is off (`on`), else those `selection` ticks.
    pub(super) fn deleted(&self, selection: &GameSettings, on: bool) -> Vec<bool> {
        (self.delete_keys.iter())
            .map(|key| on && selection.path_included(key))
            .collect()
    }

    /// Forgets the cached counts of both trees, as after the badges changed.
    pub(super) fn invalidate(&mut self) {
        self.tree.invalidate();
        if let Some(deleting) = &mut self.deleting {
            deleting.invalidate();
        }
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
pub(super) enum Label {
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
    pub(super) fn badge(&self) -> Option<(Badge, String)> {
        batch::write_badge(match self {
            Label::State(state) => Ok(*state),
            Label::WillFail(reason) | Label::Blocks(reason) => Err(reason.as_str()),
        })
    }
}

/// What a plan job hands back.
pub(super) struct Prepared {
    pub(super) fetched: Arc<Fetched>,
    /// The dialog's choices this plan was prepared with; empty for the first try.
    pub(super) choices: Vec<(SnapshotRoot, Choice)>,
    pub(super) plan: Result<Labelled, PrepareError>,
}

/// The plan shown before anything is written.
pub(super) struct Preview {
    pub(super) fetched: Arc<Fetched>,
    pub(super) plan: RestorePlan,
    /// The writes as planned, which clearing an override puts back.
    planned: Vec<Write>,
    /// How each write compares with its target.
    labels: Vec<Label>,
    /// Why the safety backup cannot read each of `plan.deletes`; `None` when it can.
    unreadable: Vec<Option<String>>,
    /// The target folders that could not be listed.
    unlisted: Vec<String>,
    pub(super) tree: WriteTree,
    /// The registry keys with their ticks.
    pub(super) keys: KeyTree,
    /// Which files the restore writes and deletes, by planned key and deleted file's key, all
    /// ticked at first. Never saved; a new plan starts with a new one.
    pub(super) selection: GameSettings,
    /// The text of each write whose override field is open.
    editing: BTreeMap<usize, String>,
    /// The write whose override was refused, with why.
    override_error: Option<(usize, String)>,
    /// The overrides of whole folders in force, by the folder's key.
    folder_overrides: BTreeMap<String, FolderOverride>,
    /// The text of each folder whose override field is open, by the folder's key.
    folder_editing: BTreeMap<String, String>,
    /// The folder whose override was refused, with why.
    folder_error: Option<(String, String)>,
    /// The writes a folder override or its Clear moved and the job comparing them with their new
    /// targets, which keeps Restore files waiting.
    relabelling: Option<(Vec<usize>, Job<Vec<Label>>)>,
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

/// A folder's new target and the writes below the folder it moved.
struct FolderOverride {
    target: PathBuf,
    /// An action on any of these writes ends the override, as it no longer holds for the folder.
    writes: Vec<usize>,
}

/// What a folder row of the plan was clicked for, with the folder's key; `Apply` and `Clear`
/// carry the plan positions of the writes below it.
enum FolderAction {
    /// Its Override button: opens or closes its field.
    Toggle(String),
    /// Its field's Apply button.
    Apply(String, Vec<usize>),
    /// Its Clear button: every write below back to its planned target.
    Clear(String, Vec<usize>),
}

/// Why a folder override was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum FolderOverrideError {
    /// The new folder is not an absolute path.
    #[error(transparent)]
    Target(#[from] restore::OverrideError),
    /// The planned target of a file has fewer plain components than the file has below the
    /// folder, so it has no place below the new folder.
    #[error("{0} has no place below the new folder")]
    NoPlace(String),
}

/// Where the file planned to `planned` goes once its folder, `below` plain components above the
/// file itself, is overridden to `new`: `new` and the last `below` plain components of `planned`
/// (a drive is its letter), which keep the case the planned target found on disk. `None` when
/// `below` is 0, `planned` has fewer components or one that is not plain, so the result always
/// lies under `new`.
fn folder_target(new: &Path, planned: &Path, below: usize) -> Option<PathBuf> {
    let tail = layout::tail_of(planned, Path::new(""))?;
    let parts: Vec<&str> = tail.split('/').filter(|part| !part.is_empty()).collect();
    let first = parts.len().checked_sub(below).filter(|_| below > 0)?;
    Some(
        parts[first..]
            .iter()
            .fold(new.to_path_buf(), |path, part| path.join(part)),
    )
}

/// How many plain components `key` has below its folder `folder`, as the target path counts them:
/// not the empty ones or `.`.
fn components_below(key: &str, folder: &str) -> usize {
    (key.get(folder.len()..).unwrap_or_default().split('/'))
        .filter(|part| !part.is_empty() && *part != ".")
        .count()
}

/// Sends the writes `writes` below the folder `folder` into `new`, each keeping its place below
/// the folder. `keys` are the writes' planned keys and `planned` their planned writes. Changes
/// nothing when it fails.
///
/// # Errors
/// [`FolderOverrideError::NoPlace`] when a write has no place below `new`;
/// [`FolderOverrideError::Target`] when `new` is not absolute.
fn override_folder(
    plan: &mut RestorePlan,
    planned: &[Write],
    keys: &[String],
    folder: &str,
    writes: &[usize],
    new: &Path,
) -> Result<(), FolderOverrideError> {
    let targets = (writes.iter())
        .map(|&write| {
            let below = components_below(&keys[write], folder);
            folder_target(new, &planned[write].target, below)
                .map(|target| (write, target))
                .ok_or_else(|| FolderOverrideError::NoPlace(keys[write].clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // A relative `new` fails the first, before any write changes.
    for (write, target) in targets {
        plan.override_target(write, target)?;
    }
    Ok(())
}

/// The labels of `count` writes from the job that compared them with their new targets. A job
/// that failed leaves them unchecked, so each is [`Label::Blocks`], which keeps the restore from
/// running.
fn moved_labels(result: anyhow::Result<Vec<Label>>, count: usize) -> Vec<Label> {
    result.unwrap_or_else(|err| {
        let reason = format!("{err:#}");
        (0..count).map(|_| Label::Blocks(reason.clone())).collect()
    })
}

/// Drops the folder overrides that moved any of `writes`.
fn forget_folders(overrides: &mut BTreeMap<String, FolderOverride>, writes: &[usize]) {
    overrides.retain(|_, over| !over.writes.iter().any(|write| writes.contains(write)));
}

impl Preview {
    pub(super) fn new(fetched: Arc<Fetched>, labelled: Labelled) -> Self {
        let Labelled {
            plan,
            labels,
            unreadable,
            unlisted,
            tree,
            keys,
        } = labelled;
        Self {
            fetched,
            planned: plan.writes.clone(),
            plan,
            labels,
            unreadable,
            unlisted,
            tree,
            keys,
            selection: GameSettings::default(),
            editing: BTreeMap::new(),
            override_error: None,
            folder_overrides: BTreeMap::new(),
            folder_editing: BTreeMap::new(),
            folder_error: None,
            relabelling: None,
        }
    }

    /// Takes the labels of the writes a folder override moved once their job is done.
    fn poll_relabelling(&mut self) {
        let Some((writes, job)) = &mut self.relabelling else {
            return;
        };
        let Some(result) = job.take_result() else {
            return;
        };
        let labels = moved_labels(result, writes.len());
        for (&write, label) in writes.iter().zip(labels) {
            self.labels[write] = label;
        }
        self.relabelling = None;
        self.tree.invalidate();
    }

    /// Why the restore keeps each of `plan.deletes`, by index, whatever its tick (ADR-0036): a
    /// folder override covers it, as the override sends that folder's writes elsewhere, or a
    /// ticked write now targets it (ASCII case ignored), which `restore::run` would refuse to
    /// delete. `None` for the others.
    fn kept(&self) -> Vec<Option<&'static str>> {
        let mut moved = GameSettings::default();
        for folder in self.folder_overrides.keys() {
            moved.set_path(folder, false);
        }
        let ticked = self.tree.ticked(&self.selection);
        let written: HashSet<PathBuf> = (self.plan.writes.iter().zip(ticked))
            .filter(|(_, ticked)| *ticked)
            .map(|(write, _)| restore::lowered(&write.target))
            .collect();
        (self.tree.delete_keys.iter().zip(&self.plan.deletes))
            .map(|(key, delete)| {
                if !moved.path_included(key) {
                    Some("its folder is overridden for this restore")
                } else if written.contains(&restore::lowered(&delete.target)) {
                    Some("the restore writes this file")
                } else {
                    None
                }
            })
            .collect()
    }

    /// Which of `plan.deletes` the restore runs, by index: none while the game's box is off
    /// (`on`), else the ticked ones it does not keep ([`Self::kept`]).
    pub(super) fn deleted(&self, on: bool) -> Vec<bool> {
        let ticked = self.tree.deleted(&self.selection, on);
        (ticked.into_iter().zip(self.kept()))
            .map(|(ticked, kept)| ticked && kept.is_none())
            .collect()
    }

    /// The tree of the writes, and of the deletions while the game's box is on (`on`), with a
    /// checkbox on each file and folder. Each file's name and Override button show where it
    /// goes on hover; the button opens a field under it for a new target, and an overridden file
    /// shows its target with a Clear button. A file whose target differs also has a Details
    /// button. A deletion's name shows its file on hover. A folder has the same Override button,
    /// which sends every file below it to a new folder and keeps the deletions below it, and a
    /// Clear button while that is in force. Applies what was clicked and returns the folder the
    /// user asked to open and the write whose details they asked for.
    fn files_ui(&mut self, ui: &mut Ui, busy: bool, on: bool) -> (Option<PathBuf>, Option<usize>) {
        let kept = self.kept();
        let Preview {
            fetched,
            plan,
            planned,
            labels,
            unreadable,
            tree:
                WriteTree {
                    tree,
                    deleting,
                    names,
                    keys: write_keys,
                    ..
                },
            selection,
            editing,
            override_error,
            folder_overrides,
            folder_editing,
            folder_error,
            relabelling,
            ..
        } = self;
        // Targets are compared in a job, so another override waits for it.
        let locked = busy || relabelling.is_some();
        // Row positions from here on are deletions.
        let first_delete = plan.writes.len();
        let badged = |index: usize| match index.checked_sub(first_delete) {
            None => labels[index].badge(),
            Some(at) => delete_badge(unreadable[at].as_deref(), kept[at].is_some()),
        };
        let badge = |index: usize, _: &str| badged(index).map(|(badge, _)| badge);
        let hint =
            |index: usize, _: &str, _| badged(index).map(|(_, hint)| hint).unwrap_or_default();
        let (error_color, muted) = (ui.visuals().error_fg_color, ui.visuals().weak_text_color());
        let mut action = None;
        let mut details = None;
        let mut row = |ui: &mut Ui, index: usize, _: &str, slot: Slot<'_>| {
            if let Some(at) = index.checked_sub(first_delete) {
                if let Slot::End(shown) = slot {
                    shown
                        .clone()
                        .on_hover_text(plan.deletes[at].target.display().to_string());
                    if let Some(why) = kept[at] {
                        ui.weak(format!("kept: {why}"));
                    }
                }
                return;
            }
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
                let button = ui.add_enabled(!locked, egui::Button::new("Override").small());
                let label = format!("Override {name}");
                button.widget_info(|| WidgetInfo::labeled(Role::Button, !locked, &label));
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
                    if named_button(ui, !locked, "Clear", &label).clicked() {
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
                    if named_button(ui, !locked, "Apply", &label).clicked() {
                        action = Some(RowAction::Apply(index));
                    }
                });
            }
            if let Some((_, error)) = (override_error.as_ref()).filter(|(i, _)| *i == index) {
                theme::status(ui, icons::X_CIRCLE, error_color, error);
            }
        };
        let mut folder_action = None;
        // A folder's rows past the writes are deletions, which an override does not move.
        let only_writes = |rows: Vec<usize>| -> Vec<usize> {
            (rows.into_iter())
                .filter(|&row| row < first_delete)
                .collect()
        };
        let mut folder_row = |ui: &mut Ui,
                              key: &str,
                              writes: &dyn Fn() -> Vec<usize>,
                              slot: FolderSlot<'_>| match slot {
            // A folder of deletions only has no write to send elsewhere.
            FolderSlot::End { .. } if on && only_writes(writes()).is_empty() => {}
            FolderSlot::End { open } => {
                let button = ui.add_enabled(!locked, egui::Button::new("Override").small());
                let label = format!("Override folder {key}");
                button.widget_info(|| WidgetInfo::labeled(Role::Button, !locked, &label));
                let hover = "Send every file in this folder to another folder";
                if button.on_hover_text(hover).clicked() {
                    *open = !folder_editing.contains_key(key);
                    folder_action = Some(FolderAction::Toggle(key.to_owned()));
                }
                if let Some(over) = folder_overrides.get(key) {
                    let label = format!("Clear the override of folder {key}");
                    if named_button(ui, !locked, "Clear", &label).clicked() {
                        folder_action =
                            Some(FolderAction::Clear(key.to_owned(), only_writes(writes())));
                    }
                    // Last and truncated, so a long path does not push the header wider than
                    // the window.
                    theme::mark(ui, icons::ARROW_RIGHT, 16.0, muted);
                    let text = format!("{} (overridden for this restore)", over.target.display());
                    let text = egui::RichText::new(&text).color(muted);
                    ui.add(egui::Label::new(text).truncate())
                        .on_hover_text(over.target.display().to_string());
                }
            }
            FolderSlot::Top => {
                if let Some(text) = folder_editing.get_mut(key) {
                    ui.horizontal(|ui| {
                        let label = ui.weak(format!("New target of folder {key}"));
                        let field = TextEdit::singleline(text)
                            .hint_text("an absolute path, for this restore only");
                        ui.add(field).labelled_by(label.id);
                        let label = format!("Apply the new target of folder {key}");
                        if named_button(ui, !locked, "Apply", &label).clicked() {
                            let action = FolderAction::Apply(key.to_owned(), only_writes(writes()));
                            folder_action = Some(action);
                        }
                    });
                }
                if let Some((_, error)) = (folder_error.as_ref()).filter(|(k, _)| k == key) {
                    theme::status(ui, icons::X_CIRCLE, error_color, error);
                }
            }
        };
        // The tree not drawn has its counts forgotten when the box toggles.
        let tree = match deleting {
            Some(deleting) if on => deleting,
            _ => tree,
        };
        let included = |key: &str| selection.path_included(key);
        let mode = Mode::Select {
            included: &included,
            enabled: !busy,
        };
        let clicked = ui.push_id(("restore_plan", &fetched.title), |ui| {
            tree.ui_with_folder_rows(ui, mode, &badge, &hint, &mut row, &mut folder_row)
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
            forget_folders(folder_overrides, &[index]);
            tree.invalidate();
        }
        // An action ends the folder field's error as it does a file's.
        if let Some(
            FolderAction::Toggle(key) | FolderAction::Apply(key, _) | FolderAction::Clear(key, _),
        ) = &folder_action
            && folder_error.as_ref().is_some_and(|(k, _)| k == key)
        {
            *folder_error = None;
        }
        let moved = match folder_action {
            Some(FolderAction::Toggle(key)) => {
                if folder_editing.remove(&key).is_none() {
                    folder_editing.insert(key, String::new());
                }
                None
            }
            Some(FolderAction::Apply(key, writes)) => {
                let new = PathBuf::from(folder_editing.get(&key).map_or("", |text| text.trim()));
                match override_folder(plan, planned, write_keys, &key, &writes, &new) {
                    Ok(()) => {
                        folder_editing.remove(&key);
                        forget_folders(folder_overrides, &writes);
                        let over = FolderOverride {
                            target: new,
                            writes: writes.clone(),
                        };
                        folder_overrides.insert(key, over);
                        Some(writes)
                    }
                    Err(err) => {
                        *folder_error = Some((key, err.to_string()));
                        None
                    }
                }
            }
            Some(FolderAction::Clear(_, writes)) => {
                for &write in &writes {
                    plan.writes[write] = planned[write].clone();
                }
                forget_folders(folder_overrides, &writes);
                Some(writes)
            }
            None => None,
        };
        // A folder holds many files, so its targets are compared in a job.
        if let Some(writes) = moved {
            let moved: Vec<Write> = writes.iter().map(|&i| plan.writes[i].clone()).collect();
            let listed = fetched.listed.clone();
            let job = Job::spawn(ui.ctx(), move || {
                Ok(moved
                    .iter()
                    .map(|write| Label::of(write, &listed))
                    .collect())
            });
            *relabelling = Some((writes, job));
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

/// `plan` with only what is ticked (ADR-0025, ADR-0026, ADR-0036): the writes whose planned key
/// (`keys`, by write index) `selection` includes, the registry keys `registry` ticks with the
/// keys it unticks kept, or no registry when nothing is written, and the deletions `deleted`
/// says run (by index in `plan.deletes`). The restore backs up, writes and deletes this one
/// plan, so nothing unticked is written or deleted.
pub(super) fn ticked_plan(
    plan: &RestorePlan,
    keys: &[String],
    registry: &KeyTree,
    selection: &GameSettings,
    deleted: &[bool],
) -> RestorePlan {
    let mut ticked = plan.clone();
    let mut keys = keys.iter();
    (ticked.writes).retain(|_| keys.next().is_some_and(|key| selection.path_included(key)));
    let mut deleted = deleted.iter();
    (ticked.deletes).retain(|_| deleted.next().is_some_and(|deleted| *deleted));
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
pub(super) fn nothing_to_restore(plan: &RestorePlan) -> bool {
    plan.writes.is_empty() && plan.registry.is_none()
}

/// The targets of the ticked writes (`ticked`, by write index) and the files of the deletions
/// that run (`deleted`, by index in `plan.deletes`) the safety backup cannot read
/// (`unreadable`), any of which keeps the whole restore from running.
pub(super) fn blocking(
    plan: &RestorePlan,
    labels: &[Label],
    ticked: &[bool],
    unreadable: &[Option<String>],
    deleted: &[bool],
) -> Vec<String> {
    let writes = (plan.writes.iter().zip(labels).zip(ticked))
        .filter(|((_, label), ticked)| **ticked && matches!(label, Label::Blocks(_)))
        .map(|((write, _), _)| &write.target);
    let deletes = (plan.deletes.iter().zip(unreadable).zip(deleted))
        .filter(|((_, unreadable), deleted)| **deleted && unreadable.is_some())
        .map(|((delete, _), _)| &delete.target);
    (writes.chain(deletes))
        .map(|target| target.display().to_string())
        .collect()
}

/// The icon of a deletion in the plan with its tooltip: why it fails the restore when the
/// safety backup cannot read its file (`unreadable`), and none when the restore keeps it
/// (`kept`).
pub(super) fn delete_badge(unreadable: Option<&str>, kept: bool) -> Option<(Badge, String)> {
    match unreadable {
        _ if kept => None,
        Some(reason) => batch::write_badge(Err(reason)),
        None => Some((
            Badge::Deletes,
            "Will be deleted: the backup does not have it".to_owned(),
        )),
    }
}

/// Whether the box "Delete files the backup does not have" of the game `title` was last left
/// on, as `state`'s config remembers it (ADR-0036).
pub(super) fn deletes_on(state: &State, title: &str) -> bool {
    (state.config.as_ref()).is_some_and(|config| config.restore_deletes.contains(title))
}

/// Remembers the box "Delete files the backup does not have" of each of the games `titles` as
/// `on` in `state`'s config and saves it (ADR-0036).
pub(super) fn remember_deletes<'a>(
    state: &mut State,
    ctx: &egui::Context,
    titles: impl IntoIterator<Item = &'a str>,
    on: bool,
) {
    let Some(config) = &mut state.config else {
        return;
    };
    for title in titles {
        if on {
            config.restore_deletes.insert(title.to_owned());
        } else {
            config.restore_deletes.remove(title);
        }
    }
    state.save_config(ctx);
}

/// The box "Delete files the backup does not have" of the game `fetched` plans, from and to
/// `state`'s config, which a toggle saves (ADR-0036), with what `plan` deletes and the target
/// folders that could not be listed (`unlisted`). It is disabled, with why on hover, for a
/// safety backup, while the game's selection cannot be read and when nothing would be deleted;
/// a line says so for the last two while the box is remembered on. Returns whether the box is on
/// and whether it toggled this frame.
pub(super) fn delete_box_ui(
    ui: &mut Ui,
    state: &mut State,
    fetched: &Fetched,
    plan: &RestorePlan,
    unlisted: &[String],
    enabled: bool,
) -> (bool, bool) {
    let title = &fetched.title;
    let pre_restore = (fetched.listed.snapshot.tags.iter()).any(|tag| tag == PRE_RESTORE_TAG);
    let unreadable = fetched.today.is_none();
    let why_not = if pre_restore {
        Some("A safety backup holds only what a restore replaced, so restoring it deletes nothing.")
    } else if unreadable {
        Some("What to back up of this game cannot be read, so no file is deleted.")
    } else if plan.deletes.is_empty() {
        Some("This PC has no file the backup lacks in the backup's folders.")
    } else {
        None
    };
    let remembered = deletes_on(state, title) && !pre_restore;
    let mut on = remembered && !unreadable;
    let checkbox = egui::Checkbox::new(&mut on, "Delete files the backup does not have");
    let usable = enabled && why_not.is_none();
    let response = ui.add_enabled(usable, checkbox);
    // Named after the game, as the batch confirm has one per game.
    let label = format!("Delete files the backup of {title} does not have");
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, usable, on, &label));
    let toggled = response.changed();
    match why_not {
        Some(why) => response.on_disabled_hover_text(why),
        None => response.on_hover_text(
            "Deletes the ticked files of this PC in the backup's folders that the backup does \
             not have, after the safety backup copies them",
        ),
    };
    if toggled {
        remember_deletes(state, ui.ctx(), [title.as_str()], on);
    }
    let warn = ui.visuals().warn_fg_color;
    if remembered && unreadable {
        let text = "No file will be deleted: what to back up of this game cannot be read.";
        theme::status(ui, icons::WARNING, warn, text);
    } else if on && plan.deletes.is_empty() {
        ui.weak("Nothing to delete: this PC has no file the backup lacks in the backup's folders.");
    }
    if on {
        for dir in unlisted {
            let text = format!("Nothing is deleted in {dir}: it cannot be listed.");
            theme::status(ui, icons::WARNING, warn, text);
        }
    }
    (on, toggled)
}

/// The line above the restore button while `deletes` files will be deleted.
pub(super) fn deletes_line(ui: &mut Ui, deletes: usize) {
    if deletes > 0 {
        let warn = ui.visuals().warn_fg_color;
        let text = format!(
            "{} will be deleted. The safety backup keeps them; restoring it brings them back.",
            count(deletes, "file")
        );
        theme::status(ui, icons::WARNING, warn, text);
    }
}

/// The cross-build warning and the notes of `plan`.
pub(super) fn warnings_ui(ui: &mut Ui, plan: &RestorePlan) {
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

/// A line pointing to the Custom games tab when another PC's version of the custom game `title`
/// waits there for review or is set aside, since its files would otherwise show as skipped with
/// no hint (ADR-0030). The tab hides an offer whose name has an unsent change, so the line does
/// too.
pub(super) fn offer_ui(ui: &mut Ui, state: &State, title: &str) {
    let Some(config) = &state.config else {
        return;
    };
    let Some(offered) = (state.custom_games.listed(config)).find(|game| game.name == title) else {
        return;
    };
    if config.set_aside(offered) {
        ui.weak(
            "Another PC's version of this game's custom definition is set aside in Custom games, \
             where you can still use it. This restore uses this PC's definition only.",
        );
    } else {
        ui.weak(
            "Another PC's version of this game's custom definition waits for review in Custom \
             games. This restore uses this PC's definition only.",
        );
    }
}

/// The items `plan` does not restore, with why.
pub(super) fn skipped_ui(ui: &mut Ui, plan: &RestorePlan) {
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

/// A prefix by its launcher's name for it (ADR-0023); `None` for a kind this version cannot
/// name.
pub(super) fn prefix_name(origin: &PrefixOrigin) -> Option<String> {
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
pub(super) fn name_context(
    ctx: &Context,
    origin: Option<&PrefixOrigin>,
) -> (String, Option<String>) {
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
pub(super) fn into_line(
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
pub(super) fn hover_label(ui: &mut Ui, text: String, hover: Option<String>) {
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

impl RestoreTab {
    /// Shows the plan, or the mapping dialog while a root needs a choice (ADR-0011). Choices
    /// that produced a plan are saved on the game's custom entry.
    pub(super) fn prepared(&mut self, prepared: Prepared, ctx: &egui::Context, state: &mut State) {
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

    /// Reads `snapshot`'s listing, or takes the last one read when it is the same snapshot, and
    /// plans its restore into the picked targets. It downloads nothing: the restore downloads
    /// into a work dir under the cache dir (ADR-0020).
    pub(super) fn start_plan(
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
        let config_dir = state.env.config_dir.clone();
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
            let read = match lock_storage(&storage) {
                Ok(mut storage) => selections(&mut storage, &config_dir, &config.secret_scope()),
                Err(err) => Err(format!("{err:#}")),
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
                source: Source::of(&title, &manifest, &config.custom_games),
                today: selection_of(&read, &title),
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

    pub(super) fn mapping_ui(&mut self, ctx: &egui::Context, state: &State) {
        let (busy, error) = (self.busy(), self.error.clone());
        let Some((fetched, dialog)) = (self.mapping.as_mut()).filter(|_| !self.dialog_hidden)
        else {
            return;
        };
        // Not dismissible: Escape in a target field or a click closing its candidates list would
        // hide the dialog with the typed targets.
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

    pub(super) fn preview_ui(&mut self, ui: &mut Ui, state: &mut State) {
        let busy = self.busy();
        let Some(preview) = &mut self.preview else {
            let (mut choose, mut retry, mut paths) = (false, false, false);
            theme::row(
                ui,
                |ui| self.status_ui(ui, &mut choose, &mut retry, &mut paths),
                |ui| restore_button(ui, false),
            );
            if choose {
                self.dialog_hidden = false;
            }
            if paths {
                self.manifest_paths = self.mapping.as_ref().map(|(fetched, _)| fetched.clone());
            }
            if retry {
                self.error = None;
                self.forget_plan();
                self.detect_unread(ui.ctx(), state);
            }
            return;
        };
        preview.poll_relabelling();
        ui.add_space(16.0);
        ui.heading("Plan");
        let fetched = &preview.fetched;
        let used = restore::used_targets(&preview.plan, &fetched.targets);
        let (into, hover) = into_line(&used, fetched.prefix_origin.as_ref(), fetched.reason);
        hover_label(ui, into, hover);
        let error_color = ui.visuals().error_fg_color;
        warnings_ui(ui, &preview.plan);
        offer_ui(ui, state, &fetched.title);
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
        let (on, toggled) = delete_box_ui(
            ui,
            state,
            &preview.fetched,
            &preview.plan,
            &preview.unlisted,
            !busy,
        );
        if toggled {
            preview.tree.invalidate();
        }
        let deleted = preview.deleted(on);
        let (unreadable, labels) = (&preview.unreadable, &preview.labels);
        let blocking = blocking(&preview.plan, labels, &ticked, unreadable, &deleted);
        if !blocking.is_empty() {
            let text = "Nothing will be restored: the safety backup cannot read these files:";
            theme::status(ui, icons::X_CIRCLE, error_color, text);
            for path in &blocking {
                ui.weak(path);
            }
        }
        let (opened, details) = theme::group(ui, |ui| {
            let clicked = preview.files_ui(ui, busy, on);
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
        let unapplied_file = (preview.editing.iter()).find_map(|(&index, text)| {
            let text = text.trim();
            (!text.is_empty() && Path::new(text) != preview.plan.writes[index].target)
                .then(|| preview.tree.names[index].clone())
        });
        let unapplied_folder = (preview.folder_editing.iter()).find_map(|(key, text)| {
            let text = text.trim();
            let applied = preview.folder_overrides.get(key);
            (!text.is_empty() && applied.is_none_or(|over| Path::new(text) != over.target))
                .then(|| format!("folder {key}"))
        });
        let unapplied = unapplied_file.or(unapplied_folder);
        let relabelling = preview.relabelling.is_some();
        if let Some(name) = &unapplied {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("The new target of {name} is not applied: click Apply or clear it."),
            );
        }
        let blocked = !blocking.is_empty();
        let deletes = deleted.iter().filter(|&&deleted| deleted).count();
        let nothing = writes == 0 && !preview.keys.any_ticked() && deletes == 0;
        // A deletion reads what to back up from the pending file, so it waits for its write.
        let saving = deletes > 0 && self.choices_saved == Saved::Saving;
        let ready = !busy
            && !self.backing_up
            && !saving
            && state.session.is_some()
            && unapplied.is_none()
            && !relabelling
            && !blocked
            && !nothing;
        let restoring = self.restoring.is_some();
        let empty = nothing_to_restore(&preview.plan);
        let has_registry = preview.plan.registry.is_some();
        let (mut retry, mut paths) = (false, false);
        let backing_up = self.backing_up;
        let start = |ui: &mut Ui| {
            if restoring {
                ui.weak("Downloading and restoring…");
            } else if backing_up {
                ui.weak("Waiting for the running backup to end.");
            } else if saving {
                ui.weak("Waiting for the changes to what to back up to be saved.");
            } else if relabelling {
                ui.weak("Checking the new targets…");
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
            paths |= ui.button("Manifest paths").clicked();
        };
        deletes_line(ui, deletes);
        let restore = theme::row(ui, start, |ui| restore_button(ui, ready)).clicked();
        if paths {
            self.manifest_paths = Some(preview.fetched.clone());
        }
        if restore {
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
    /// "Try again" after a failed plan, whose error shows above. Sets `choose`, `retry` or `paths`
    /// when their button ("Manifest paths" beside "Choose folders…") is clicked.
    fn status_ui(&self, ui: &mut Ui, choose: &mut bool, retry: &mut bool, paths: &mut bool) {
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
            *paths |= ui.button("Manifest paths").clicked();
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
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    fn write(target: &str) -> Write {
        Write::new(PathBuf::from("staged"), PathBuf::from(target), None, None)
    }

    #[test]
    fn a_folder_target_keeps_the_last_components_below_the_new_folder() {
        let planned = Path::new("/home/me/.local/share/Deep/sub/b.sav");
        let new = Path::new("/elsewhere");
        let target = folder_target(new, planned, 2);
        assert_eq!(target, Some(PathBuf::from("/elsewhere/sub/b.sav")));
        let target = folder_target(new, planned, 1);
        assert_eq!(target, Some(PathBuf::from("/elsewhere/b.sav")));
    }

    #[test]
    fn a_folder_target_always_lies_under_the_new_folder() {
        // Absolute on this OS, which a path without a drive is not on Windows.
        let new = Path::new(if cfg!(windows) {
            r"C:\elsewhere"
        } else {
            "/elsewhere"
        });
        // Too few components, none, or a component that is not plain: no target, never the
        // planned path itself.
        for (planned, below) in [
            ("/a/b.sav", 3),
            ("/a/b.sav", 0),
            ("/a/../b.sav", 2),
            ("/", 1),
        ] {
            assert_eq!(
                folder_target(new, Path::new(planned), below),
                None,
                "{planned}"
            );
        }
        for (planned, below) in [("/a/b/c.sav", 3), ("/a/b/c.sav", 1), ("/a/b/c.sav", 2)] {
            let target = folder_target(new, Path::new(planned), below).unwrap();
            assert!(target.starts_with(new), "{target:?}");
            assert!(target.is_absolute(), "{target:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_target_takes_the_drive_letter_as_a_component() {
        let new = Path::new(r"E:\Moved");
        let planned = Path::new(r"C:\Games\x.sav");
        let target = folder_target(new, planned, 3).unwrap();
        assert_eq!(target, PathBuf::from(r"E:\Moved\C\Games\x.sav"));
        // The drive and root are never taken as such, which would send the file off `new`.
        let target = folder_target(new, planned, 2).unwrap();
        assert_eq!(target, PathBuf::from(r"E:\Moved\Games\x.sav"));
        assert_eq!(folder_target(new, planned, 4), None);
    }

    #[test]
    fn components_below_skip_the_ones_the_target_path_drops() {
        assert_eq!(components_below("Base/a/b/c.sav", "Base/a"), 2);
        assert_eq!(components_below("Base/a/./b//c.sav", "Base/a"), 2);
        assert_eq!(
            components_below("WinAbsolute/C/Games/x.sav", "WinAbsolute"),
            3
        );
    }

    #[test]
    fn a_folder_override_with_a_file_without_a_place_changes_nothing() {
        let planned = vec![write("/a/b/c.sav"), write("/d.sav")];
        let keys = vec!["Base/f/b/c.sav".to_owned(), "Base/f/x/y/d.sav".to_owned()];
        let mut plan = RestorePlan::new(planned.clone(), None, Vec::new(), false, Vec::new());

        let err = override_folder(
            &mut plan,
            &planned,
            &keys,
            "Base/f",
            &[0, 1],
            Path::new("/n"),
        );

        assert_eq!(err, Err(FolderOverrideError::NoPlace(keys[1].clone())));
        assert_eq!(plan.writes[0].target, planned[0].target);
    }

    #[test]
    fn a_folder_override_with_a_relative_path_changes_nothing() {
        let planned = vec![write("/a/b/c.sav"), write("/a/b/d.sav")];
        let keys = vec!["Base/f/c.sav".to_owned(), "Base/f/d.sav".to_owned()];
        let mut plan = RestorePlan::new(planned.clone(), None, Vec::new(), false, Vec::new());

        let err = override_folder(
            &mut plan,
            &planned,
            &keys,
            "Base/f",
            &[0, 1],
            Path::new("n"),
        );

        assert_eq!(
            err,
            Err(FolderOverrideError::Target(
                restore::OverrideError::RelativePath
            ))
        );
        assert_eq!(plan.writes[1].target, planned[1].target);
    }

    #[test]
    fn a_failed_relabel_job_blocks_every_write_it_moved() {
        let labels = moved_labels(Err(anyhow!("the job panicked")), 2);

        assert_eq!(labels.len(), 2);
        assert!(labels.iter().all(|label| matches!(label, Label::Blocks(_))));
        // A blocking label stops the restore, as the plan's "safety backup cannot read" list.
        let plan = RestorePlan::new(
            vec![write("/a"), write("/b")],
            None,
            Vec::new(),
            false,
            Vec::new(),
        );
        assert_eq!(blocking(&plan, &labels, &[true, true], &[], &[]).len(), 2);
        assert!(blocking(&plan, &labels, &[false, false], &[], &[]).is_empty());
    }

    #[test]
    fn a_finished_relabel_job_keeps_its_labels() {
        let labels = moved_labels(Ok(vec![Label::State(WriteState::New)]), 1);
        assert!(matches!(labels[..], [Label::State(WriteState::New)]));
    }
}
