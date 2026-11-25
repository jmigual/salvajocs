use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, TextEdit, Ui, Vec2};
use salvajocs_core::config::{AppConfig, ListSort, SortKey};
use salvajocs_core::layout::{Context, LinuxFolders, PrefixOrigin, WinFolders};
use salvajocs_core::prefix;
use salvajocs_core::restore::{
    self, DefaultTargets, GameBackups, Listed, PRE_RESTORE_TAG, PrefixReason, Targets,
};
use salvajocs_core::scan::Environment;
use salvajocs_core::stats::{self, BackupStats};
use salvajocs_core::storage::SnapshotInfo;
use salvajocs_core::sync;

use super::delete::Deletion;
use super::dirs::own_dir;
use super::plan::prefix_name;
use super::{RestoreTab, batch};
use crate::job::Job;
use crate::state::{State, lock_storage};
use crate::tabs::space::Space;
use crate::tabs::{format_date, format_time, sort};
use crate::theme::{self, icons};

/// How a snapshot shows in the picker: time, host and the `pre-restore` label.
pub(super) fn snapshot_label(snapshot: &SnapshotInfo) -> String {
    let mut label = format!("{}, {}", format_time(snapshot.time), snapshot.host);
    if snapshot.tags.iter().any(|tag| tag == PRE_RESTORE_TAG) {
        label += ", pre-restore";
    }
    label
}

/// The slugs of the `games` whose newest regular snapshot is not this PC's recorded base of them
/// in the repository `scope`. A game without a record is not among them: a seed is no record.
fn newer_than_base(games: &[GameBackups], bases: &sync::Bases, scope: &str) -> BTreeSet<String> {
    (games.iter())
        .filter(|game| {
            let newest = game.default.and_then(|index| game.snapshots.get(index));
            let base = bases.get(scope, &game.slug);
            matches!((base, newest), (Some(base), Some(newest)) if base.snapshot != newest.id)
        })
        .map(|game| game.slug.clone())
        .collect()
}

/// The build the user restores into, preselected from `restore::default_targets` (ADR-0005).
pub(super) struct TargetPick {
    /// The native Windows folders on a Windows host, where there is nothing to pick.
    windows: Option<Context>,
    /// The native Linux folders on a Linux host.
    linux: Option<LinuxFolders>,
    pub(super) use_linux: bool,
    pub(super) use_prefix: bool,
    /// The prefix path as typed or chosen.
    pub(super) prefix: String,
    /// Whether `prefix` was typed and not applied yet, so half-typed paths are not planned.
    pub(super) typing: bool,
    /// The prefix `default_targets` found with its path as text, reused while `prefix` still
    /// names it.
    found: Option<(String, Context)>,
    /// Whether the user changed the pick, which another snapshot's default then keeps.
    pub(super) edited: bool,
    /// Why the default chose `found`, and its launcher's name for it (ADR-0023).
    pub(super) reason: Option<PrefixReason>,
    origin: Option<PrefixOrigin>,
    /// The prefixes that all hold the backup's save folders, so none was chosen.
    pub(super) ambiguous: Vec<PathBuf>,
    /// The prefixes the backup's prefix name fits when none holds its saves.
    pub(super) stored_several: Vec<PathBuf>,
    /// The prefix the backup names when this machine does not have it.
    pub(super) missing: Option<PrefixOrigin>,
}

impl TargetPick {
    /// The pick `defaults` preselect; `missing` is the prefix the snapshot names when this
    /// machine does not have it.
    pub(super) fn new(defaults: DefaultTargets, missing: Option<PrefixOrigin>) -> Self {
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
    pub(super) fn default_reason(&self) -> Option<PrefixReason> {
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
    pub(super) fn ready(&self) -> bool {
        self.windows.is_some()
            || (self.use_linux && self.linux.is_some())
            || (self.use_prefix && !self.prefix.trim().is_empty())
    }

    /// The targets the pick names. Reads the prefix's Wine user from disk, so it runs in a job.
    pub(super) fn targets(&self) -> impl FnOnce() -> anyhow::Result<Targets> + Send + use<> {
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

/// What a detection job hands back: the default targets of the chosen snapshot, and its listing
/// when it could be read.
pub(super) struct Detected {
    /// The id of the snapshot detected for; `None` when none was chosen.
    pub(super) snapshot: Option<String>,
    pub(super) listed: Option<Arc<Listed>>,
    pub(super) defaults: DefaultTargets,
}

impl RestoreTab {
    /// Starts working out the space of the listed games' backups that the session has not yet.
    pub(super) fn start_space(&mut self, ctx: &egui::Context, state: &State) {
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
            .map(|(game, figures)| (game.title.as_str(), Some(figures), None))
            .collect();
        self.order = stats::order(&rows, sort);
        self.lines = vec![None; self.games.len()];
        self.sorted = Some((sort, self.list_changes));
    }

    pub(super) fn start_listing(&mut self, ctx: &egui::Context, state: &State) {
        if self.read_list(ctx, state) {
            self.selected = None;
            self.reselect = None;
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
        let config_dir = state.env.config_dir.clone();
        let scope = (state.config.as_ref()).map(AppConfig::secret_scope);
        self.listing = Some(Job::spawn(ctx, move || {
            let games = restore::list(&*lock_storage(&storage)?)?;
            let newer = scope.map_or_else(BTreeSet::new, |scope| {
                newer_than_base(&games, &sync::load(&config_dir), &scope)
            });
            Ok((games, newer))
        }));
        true
    }

    /// Lists again after a restore or delete, then chooses the game that was chosen if it is
    /// still listed.
    pub(super) fn list_keeping_choice(&mut self, ctx: &egui::Context, state: &State) {
        let chosen = self.selected.clone();
        self.start_listing(ctx, state);
        if self.listing.is_some() {
            self.reselect = chosen;
        }
    }

    /// Before the first listing: what the tab is for and the listing as its one action.
    pub(super) fn empty_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
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
    pub(super) fn games_ui(&mut self, ui: &mut Ui, state: &mut State, idle: bool) {
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
        // Changes is a Backup tab key; a hand-edited config that names it sorts by name here.
        let sort = if sort.by == SortKey::Changes {
            ListSort::default()
        } else {
            sort
        };
        if let Some(picked) = sort::sort_ui(ui, "restore_sort", sort::RESTORE_KEYS, sort)
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
                        if self.newer.contains(&game.slug)
                            && let Some(newest) = game.default.and_then(|i| game.snapshots.get(i))
                        {
                            let hover = format!(
                                "The newest backup is not the one this PC last synced: from {}, {}",
                                newest.host,
                                format_date(newest.time)
                            );
                            let accent = theme::palette(ui).accent;
                            theme::labeled_mark(ui, icons::ARROW_DOWN, accent, &hover);
                        }
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
    pub(super) fn game_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
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
        self.error = None;
        self.removed = None;
        self.select(ctx, state, game);
    }

    /// [`Self::choose`] without clearing the error and the removed count, which a relisting
    /// after a delete must keep showing.
    pub(super) fn select(&mut self, ctx: &egui::Context, state: &State, game: &GameBackups) {
        self.selected = Some(game.slug.clone());
        self.snapshot = game.default;
        self.pick = None;
        self.forget_plan();
        self.detect(ctx, state, game);
    }

    /// The chosen snapshot of the chosen game.
    pub(super) fn chosen(&self) -> Option<&SnapshotInfo> {
        let game = (self.games.iter()).find(|game| Some(&game.slug) == self.selected.as_ref())?;
        game.snapshots.get(self.snapshot?)
    }

    /// Works the default targets out again when the chosen snapshot's read failed during
    /// detection and the user has not changed the pick, so a retry can still use its prefix.
    pub(super) fn detect_unread(&mut self, ctx: &egui::Context, state: &State) {
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
}
