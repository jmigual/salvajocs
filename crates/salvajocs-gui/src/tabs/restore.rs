//! The Restore tab: the games in the repository with their snapshots from every host, the build
//! to restore into, a preview of the plan and the restore behind a safety backup (ADR-0005,
//! ADR-0010, ADR-0011).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use eframe::egui::{self, Color32, TextEdit, Ui};
use salvajocs_core::crossbuild::{self, Choice, SnapshotRoot};
use salvajocs_core::layout::{Base, Context, FILES_DIR, GameVars, LinuxFolders, WinFolders};
use salvajocs_core::manifest::{GameEntry, Manifest, Pair, merge_custom};
use salvajocs_core::prefix;
use salvajocs_core::restore::{
    self, GameBackups, PRE_RESTORE_TAG, PrepareError, RestorePlan, RunError, SafetyBackup,
    SkippedItem, StagedGame, Targets,
};
use salvajocs_core::scan::Environment;
use salvajocs_core::storage::SnapshotInfo;

use crate::job::Job;
use crate::state::{SharedStorage, State};

mod mapping;

use mapping::{Dialog, DialogAction};

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
            Some(Context::Linux(_)) | None => {}
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
            changed |= ui.add(field).labelled_by(label.id).changed();
        });
        ui.weak(
            "Saves of the other build are mapped onto the chosen one (cross-build restore); \
             with both chosen, each goes into its own.",
        );
        changed
    }
}

/// A fetched snapshot and what `restore::prepare` needs besides the user's choices.
struct Fetched {
    title: String,
    staged: StagedGame,
    targets: Targets,
    vars: GameVars,
    entry: Option<GameEntry>,
    pairs: Vec<Pair>,
    skips: Vec<String>,
}

impl Fetched {
    fn prepare(&self, choices: &[(SnapshotRoot, Choice)]) -> Result<RestorePlan, PrepareError> {
        restore::prepare(
            &self.staged,
            &self.targets,
            &self.vars,
            self.entry.as_ref(),
            &self.pairs,
            &self.skips,
            choices,
        )
    }
}

/// What a preview job hands back.
struct Prepared {
    fetched: Arc<Fetched>,
    /// The dialog's choices this plan was prepared with; empty for the first try.
    choices: Vec<(SnapshotRoot, Choice)>,
    plan: Result<RestorePlan, PrepareError>,
}

/// The plan shown before anything is written.
struct Preview {
    fetched: Arc<Fetched>,
    plan: RestorePlan,
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

/// The state of the Restore tab.
#[derive(Default)]
pub struct RestoreTab {
    listing: Option<Job<Vec<GameBackups>>>,
    games: Vec<GameBackups>,
    /// The slug of the chosen game.
    selected: Option<String>,
    /// The chosen snapshot's index in the game's `snapshots`. It starts as the game's default, so
    /// `None` when every snapshot is a safety backup, which is never preselected (ADR-0010).
    snapshot: Option<usize>,
    detecting: Option<Job<Targets>>,
    pick: Option<TargetPick>,
    picking: Option<Job<Option<PathBuf>>>,
    preparing: Option<Job<Prepared>>,
    mapping: Option<(Arc<Fetched>, Dialog)>,
    preview: Option<Preview>,
    restoring: Option<Job<Restored>>,
    restored: Option<Restored>,
    error: Option<String>,
}

impl RestoreTab {
    /// Draws the tab. Listing, fetching, planning and restoring run in jobs; the mapping dialog's
    /// choices are saved to the config. `manifest_loading` is whether the manifest job runs.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        self.poll(ui.ctx(), state);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Restore");
            if state.config.is_none() {
                ui.label("Choose a repository first.");
                return;
            }
            let busy = self.busy();
            // `default_targets` looks the game's Steam id up in the manifest.
            let idle = !busy && !manifest_loading;
            ui.add_enabled_ui(idle, |ui| self.games_ui(ui, state, manifest_loading));
            if busy {
                ui.spinner();
            }
            if let Some(error) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            self.preview_ui(ui, state);
            self.restored_ui(ui);
        });
        self.mapping_ui(ui.ctx(), state);
    }

    fn busy(&self) -> bool {
        self.listing.is_some()
            || self.detecting.is_some()
            || self.picking.is_some()
            || self.preparing.is_some()
            || self.restoring.is_some()
    }

    /// Drops the plan and the dialog, which no longer match the chosen game or targets.
    fn forget_plan(&mut self) {
        self.preview = None;
        self.mapping = None;
    }

    fn poll(&mut self, ctx: &egui::Context, state: &mut State) {
        if let Some(job) = &mut self.listing
            && let Some(result) = job.take_result()
        {
            self.listing = None;
            match result {
                Ok(games) => self.games = games,
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
        if let Some(job) = &mut self.preparing
            && let Some(result) = job.take_result()
        {
            self.preparing = None;
            match result {
                Ok(prepared) => self.prepared(prepared, ctx, state),
                Err(err) => self.error = Some(format!("The preview failed: {err:#}")),
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
            Ok(plan) => {
                if let (false, Some(config)) = (choices.is_empty(), &mut state.config) {
                    crossbuild::apply_choices(config, &fetched.title, &choices);
                    state.save_config(ctx);
                }
                self.mapping = None;
                self.preview = Some(Preview {
                    overrides: vec![String::new(); plan.writes.len()],
                    fetched,
                    plan,
                    override_error: None,
                });
            }
            Err(PrepareError::NeedsMapping(entries)) => {
                self.mapping = Some((fetched, Dialog::new(entries)));
            }
            Err(PrepareError::Unclassifiable { root, template }) => match &mut self.mapping {
                Some((_, dialog)) => dialog.reject(&root, &template),
                None => {
                    self.error = Some(format!(
                        "The saved target {template} is not a placeholder template of this OS"
                    ));
                }
            },
            Err(err) => self.error = Some(format!("The preview failed: {err}")),
        }
    }

    fn start_listing(&mut self, ctx: &egui::Context, state: &State) {
        let Some(session) = &state.session else {
            return;
        };
        let storage = session.storage.clone();
        self.selected = None;
        self.pick = None;
        self.listing = Some(Job::spawn(ctx, move || {
            Ok(restore::list(&*lock(&storage)?)?)
        }));
    }

    fn games_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        let refresh = ui.add_enabled(state.session.is_some(), egui::Button::new("Load backups"));
        if refresh.clicked() {
            self.error = None;
            self.forget_plan();
            self.start_listing(ui.ctx(), state);
        }
        if state.session.is_none() {
            ui.weak("The repository is not open.");
        }
        let mut chosen = None;
        for game in &self.games {
            let selected = self.selected.as_ref() == Some(&game.slug);
            if ui.selectable_label(selected, &game.title).clicked() && !selected {
                chosen = Some(game.clone());
            }
        }
        if let Some(game) = chosen {
            self.choose(ui.ctx(), state, &game);
        }
        let selected = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref());
        let Some(game) = selected.cloned() else {
            return;
        };
        ui.separator();
        ui.heading(&game.title);
        let before = self.snapshot;
        let label = ui.label("Snapshot");
        egui::ComboBox::from_id_salt("restore_snapshot")
            .selected_text(
                (self.snapshot.and_then(|index| game.snapshots.get(index)))
                    .map_or_else(|| "Choose a snapshot".to_owned(), snapshot_label),
            )
            .show_ui(ui, |ui| {
                for (index, snapshot) in game.snapshots.iter().enumerate() {
                    ui.selectable_value(&mut self.snapshot, Some(index), snapshot_label(snapshot));
                }
            })
            .response
            .labelled_by(label.id);
        let mut changed = self.snapshot != before;
        if let Some(pick) = &mut self.pick {
            changed |= pick.ui(ui);
            if pick.windows.is_none() && ui.button("Choose prefix").clicked() {
                self.picking = Some(Job::spawn(ui.ctx(), || {
                    Ok(rfd::FileDialog::new().pick_folder())
                }));
            }
        }
        if changed {
            self.forget_plan();
        }
        let ready = self.pick.as_ref().is_some_and(TargetPick::ready)
            && self.snapshot.is_some()
            && state.session.is_some();
        if ui
            .add_enabled(ready, egui::Button::new("Preview"))
            .clicked()
            && let Some(snapshot) = self.snapshot.and_then(|index| game.snapshots.get(index))
        {
            let title = game.title.clone();
            self.start_preview(ui.ctx(), state, title, snapshot.clone());
        }
        if manifest_loading {
            ui.weak("The game manifest is loading.");
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
        self.forget_plan();
        let (title, manifest, config) = (game.title.clone(), manifest(state), config.clone());
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

    /// Fetches `snapshot` into a fresh work dir under the cache dir and plans its restore.
    fn start_preview(
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
        self.forget_plan();
        let (storage, manifest, config) =
            (session.storage.clone(), manifest(state), config.clone());
        let environment = state.env.environment.clone();
        let work_dir = state.env.cache_dir.join("restore-work");
        self.preparing = Some(Job::spawn(ctx, move || {
            let targets = targets()?;
            remove_dir(&work_dir)?;
            let staged = restore::fetch(&mut *lock(&storage)?, &snapshot, &work_dir)?;
            let mut merged = Manifest {
                games: (manifest.games.get(&title).cloned())
                    .map(|entry| (title.clone(), entry))
                    .into_iter()
                    .collect(),
            };
            merge_custom(&mut merged, &config.custom_games);
            let entry = merged.games.remove(&title);
            let custom = || (config.custom_games.iter()).filter(|game| game.name == title);
            let environment = environment.unwrap_or_else(Environment::detect);
            let fetched = Fetched {
                vars: restore::game_vars(&title, entry.as_ref(), &config, &environment),
                pairs: custom().flat_map(|game| game.pairs.clone()).collect(),
                skips: custom().flat_map(|game| game.skips.clone()).collect(),
                title,
                staged,
                targets,
                entry,
            };
            Ok(Prepared {
                plan: fetched.prepare(&[]),
                fetched: Arc::new(fetched),
                choices: Vec::new(),
            })
        }));
    }

    fn mapping_ui(&mut self, ctx: &egui::Context, state: &State) {
        let busy = self.busy();
        let Some((fetched, dialog)) = &mut self.mapping else {
            return;
        };
        let action = egui::Modal::new(egui::Id::unique("restore_mapping"))
            .show(ctx, |ui| {
                ui.set_width(720.0);
                dialog.ui(ui, &fetched.title, busy)
            })
            .inner;
        match action {
            Some(DialogAction::Confirm(choices)) if state.session.is_some() => {
                let fetched = fetched.clone();
                self.preparing = Some(Job::spawn(ctx, move || {
                    Ok(Prepared {
                        plan: fetched.prepare(&choices),
                        fetched,
                        choices,
                    })
                }));
            }
            Some(DialogAction::Cancel) => self.mapping = None,
            Some(DialogAction::Confirm(_)) | None => {}
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, state: &State) {
        let busy = self.busy();
        let Some(preview) = &mut self.preview else {
            ui.add_enabled(false, egui::Button::new("Restore files"));
            return;
        };
        ui.separator();
        ui.heading("Plan");
        if preview.plan.cross_build {
            ui.colored_label(
                Color32::YELLOW,
                "Cross-build restore: the other build may use a different save format. The \
                 safety backup lets you undo it.",
            );
        }
        for note in &preview.plan.notes {
            ui.colored_label(Color32::YELLOW, format!("Note: {note}"));
        }
        ui.label(format!("Files to write ({})", preview.plan.writes.len()));
        let files = preview.fetched.staged.root.join(FILES_DIR);
        let mut overridden = None;
        for (index, write) in preview.plan.writes.iter().enumerate() {
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
            ui.label(format!(
                "{number}. {source} -> {}{flag}",
                write.target.display()
            ));
            ui.horizontal(|ui| {
                let label = ui.label(format!("Override target {number}"));
                let field = TextEdit::singleline(&mut preview.overrides[index])
                    .hint_text("an absolute path, for this restore only");
                ui.add(field).labelled_by(label.id);
                let button = egui::Button::new(format!("Override {number}"));
                if ui.add_enabled(!busy, button).clicked() {
                    overridden = Some(index);
                }
            });
            if let Some((_, error)) = (preview.override_error.as_ref()).filter(|(i, _)| *i == index)
            {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
        }
        if let Some(index) = overridden {
            let target = PathBuf::from(preview.overrides[index].trim());
            preview.override_error = (preview.plan.override_target(index, target))
                .err()
                .map(|err| (index, err.to_string()));
        }
        if let Some((export, ctx)) = &preview.plan.registry {
            ui.label(format!(
                "Registry: {} keys into {}",
                export.keys.len(),
                describe_context(ctx)
            ));
        }
        if !preview.plan.skipped.is_empty() {
            ui.label(format!("Skipped ({})", preview.plan.skipped.len()));
            for skipped in &preview.plan.skipped {
                let item = match &skipped.item {
                    SkippedItem::File(base, tail) => format!("{}/{tail}", base.dir_name()),
                    SkippedItem::Registry => "registry".to_owned(),
                    SkippedItem::RegistryKey(key) => key.clone(),
                };
                ui.weak(format!("{item}: {}", skipped.reason));
            }
        }
        let ready = !busy && state.session.is_some();
        if ui
            .add_enabled(ready, egui::Button::new("Restore files"))
            .clicked()
        {
            self.start_restore(ui.ctx(), state);
        }
    }

    /// Runs the previewed plan behind a safety backup staged in its own dir under the cache dir.
    fn start_restore(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(preview)) = (&state.session, &self.preview) else {
            return;
        };
        self.error = None;
        self.restored = None;
        let (storage, plan) = (session.storage.clone(), preview.plan.clone());
        let title = preview.fetched.title.clone();
        let stage_root = state.env.cache_dir.join("restore-stage");
        self.restoring = Some(Job::spawn(ctx, move || {
            let report = restore::run(
                &mut *lock(&storage)?,
                &title,
                &plan,
                &stage_root,
                prefix::wineserver_running(),
            )?;
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
            Ok(Restored {
                title,
                safety_backup,
                lines,
            })
        }));
    }

    fn restored_ui(&self, ui: &mut Ui) {
        let Some(restored) = &self.restored else {
            return;
        };
        ui.separator();
        ui.heading(format!("Restored {}", restored.title));
        ui.label(&restored.safety_backup);
        for (line, failed) in &restored.lines {
            if *failed {
                ui.colored_label(Color32::LIGHT_RED, line);
            } else {
                ui.label(line);
            }
        }
    }
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

fn describe_context(ctx: &Context) -> String {
    match ctx {
        Context::WinePrefix { prefix, .. } => format!("the prefix {}", prefix.display()),
        Context::WindowsNative(_) => "the Windows registry".to_owned(),
        Context::Linux(_) => "Linux".to_owned(),
    }
}

/// Removes `dir` and everything below it; a missing dir is fine.
fn remove_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// The manifest to plan with: the loaded one, or an empty one so that custom games still restore.
fn manifest(state: &State) -> Arc<Manifest> {
    state.manifest.clone().unwrap_or_default()
}

fn lock(
    storage: &SharedStorage,
) -> anyhow::Result<std::sync::MutexGuard<'_, salvajocs_core::storage::Storage>> {
    storage
        .lock()
        .map_err(|_| anyhow!("the repository lock is poisoned; restart the app"))
}

#[cfg(test)]
mod tests;
