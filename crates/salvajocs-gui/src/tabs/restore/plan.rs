use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::anyhow;
use eframe::egui::{self, Response, Role, TextEdit, Ui, WidgetInfo};
use salvajocs_core::crossbuild::{Choice, SnapshotRoot};
use salvajocs_core::layout::{Context, GameVars, PrefixOrigin};
use salvajocs_core::restore::{
    self, GameInfo, Listed, PrefixReason, PrepareError, RestorePlan, SkippedItem, Targets, Used,
    Write, WriteState,
};
use salvajocs_core::scan::Environment;
use salvajocs_core::selection::GameSettings;
use salvajocs_core::storage::SnapshotInfo;

use super::dirs::own_dir;
use super::keys::KeyTree;
use super::pick::TargetPick;
use super::{RestoreTab, batch};
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::count;
use crate::tabs::details::Details;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::tabs::tree::{Badge, Clicked, FileTree, Mode, Row, Slot};
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
}

impl Fetched {
    pub(super) fn prepare(
        &self,
        choices: &[(SnapshotRoot, Choice)],
    ) -> Result<RestorePlan, PrepareError> {
        restore::prepare(&self.listed, &self.targets, &self.vars, &self.info, choices)
    }

    /// `plan` with how each write compares with its target, the tree of its writes and the tree
    /// of its registry keys. Reads the targets and which target folders exist, so it runs in a
    /// job.
    pub(super) fn labelled(
        &self,
        plan: Result<RestorePlan, PrepareError>,
    ) -> Result<Labelled, PrepareError> {
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
pub(super) struct Labelled {
    pub(super) plan: RestorePlan,
    pub(super) labels: Vec<Label>,
    pub(super) tree: WriteTree,
    pub(super) keys: KeyTree,
}

/// A plan's writes as a tree keyed by where they land as planned, with a checkbox on each file
/// and folder (ADR-0025). Its row positions are the write indices. An overridden write keeps its
/// row, so it stays where the user is.
pub(super) struct WriteTree {
    pub(super) tree: FileTree,
    /// Each write's planned `<Base>/<tail>` key, which its checkbox ticks; an override keeps it.
    pub(super) keys: Vec<String>,
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
    pub(super) fn ticked(&self, selection: &GameSettings) -> Vec<bool> {
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
    pub(super) tree: WriteTree,
    /// The registry keys with their ticks.
    pub(super) keys: KeyTree,
    /// Which files the restore writes, by planned key, all ticked at first. Never saved; a new
    /// plan starts with a new one.
    pub(super) selection: GameSettings,
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
    pub(super) fn new(fetched: Arc<Fetched>, labelled: Labelled) -> Self {
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
pub(super) fn ticked_plan(
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
pub(super) fn nothing_to_restore(plan: &RestorePlan) -> bool {
    plan.writes.is_empty() && plan.registry.is_none()
}

/// The targets of the ticked writes (`ticked`, by write index) the safety backup cannot read,
/// any of which keeps the whole restore from running.
pub(super) fn blocking(plan: &RestorePlan, labels: &[Label], ticked: &[bool]) -> Vec<String> {
    (plan.writes.iter().zip(labels).zip(ticked))
        .filter(|((_, label), ticked)| **ticked && matches!(label, Label::Blocks(_)))
        .map(|((write, _), _)| write.target.display().to_string())
        .collect()
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
}
