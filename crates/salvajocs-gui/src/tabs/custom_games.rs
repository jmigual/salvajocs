//! The Custom games tab: games the user adds to the manifest or overrides in it (ADR-0008), their
//! files as Linux/Windows rows (ADR-0016), and the pairs and skips cross-build restores saved
//! (ADR-0011).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use eframe::egui::{self, TextEdit, Ui};
use salvajocs_core::config::AppConfig;
use salvajocs_core::crossbuild::{self, Family, FileError, PairError, SkipError};
use salvajocs_core::layout::{Context, GameVars, WinFolders};
use salvajocs_core::manifest::{CustomFile, CustomGame, Integration, Pair};
use salvajocs_core::prefix;
use salvajocs_core::restore;
use salvajocs_core::scan::Environment;

use crate::env::Env;
use crate::job::Job;
use crate::state::State;
use crate::theme::{self, icons};
use crate::widgets::labelled_edit;

fn integration_label(integration: Integration) -> &'static str {
    match integration {
        Integration::Override => "Override the manifest entry",
        Integration::Extend => "Extend the manifest entry",
    }
}

/// Why a form cannot be saved.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum FormError {
    /// The name is empty.
    #[error("The game needs a name")]
    EmptyName,
    /// Another custom game has the name; titles match the manifest exactly.
    #[error("There is already a custom game named {0}")]
    DuplicateName(String),
    /// A file row is not valid.
    #[error(transparent)]
    File(#[from] FileError),
    /// A pair side is not a placeholder template of its OS.
    #[error(transparent)]
    Pair(#[from] PairError),
    /// A skipped folder is not a placeholder template.
    #[error(transparent)]
    Skip(#[from] SkipError),
    /// The game changed since the form was opened, for example by a restore mapping.
    #[error("This game changed since the form was opened; reload it to see the new values")]
    Changed,
}

/// One editable Linux/Windows pair; both fields empty means the row is unused.
#[derive(Default)]
struct PairRow {
    linux: String,
    windows: String,
}

/// One editable file row; both fields empty means the row is unused.
#[derive(Default)]
struct FileRow {
    linux: String,
    windows: String,
    /// Why the last Browse for one side found no path, until that side is edited or browsed.
    browse_error: Option<(Family, String)>,
}

impl FileRow {
    fn side(&mut self, side: Family) -> &mut String {
        match side {
            Family::Linux => &mut self.linux,
            Family::Windows => &mut self.windows,
        }
    }

    /// The row as stored, with blank sides left out; `None` for an unused row.
    fn file(&self) -> Option<CustomFile> {
        let side = |text: &str| {
            Some(text.trim())
                .filter(|text| !text.is_empty())
                .map(Into::into)
        };
        let file = CustomFile {
            linux: side(&self.linux),
            windows: side(&self.windows),
        };
        (file != CustomFile::default()).then_some(file)
    }

    /// Why the row cannot be saved; `None` for a valid or unused row.
    fn error(&self) -> Option<FileError> {
        crossbuild::validate_file(&self.file()?).err()
    }
}

/// A folder pick in flight for one side of a file row.
struct Browse {
    row: usize,
    side: Family,
    /// The picked folder as a template; `None` when the user cancelled.
    job: Job<Option<String>>,
}

/// The game being added or edited.
struct Form {
    /// Position in `custom_games` of the game being edited; `None` for a new one.
    index: Option<usize>,
    name: String,
    integration: Integration,
    files: Vec<FileRow>,
    /// One registry key per line.
    registry: String,
    wine_prefix: String,
    pairs: Vec<PairRow>,
    /// One skipped root template per row; an empty row is unused.
    skips: Vec<String>,
    /// The game as the form read it. What the form does not edit (install dirs, Steam id, rules
    /// of kept lines, a wine prefix left as is) survives a save, and a save refuses when the
    /// config no longer holds this game.
    snapshot: Option<CustomGame>,
    error: Option<FormError>,
    /// One pick at a time, so the row it fills cannot move while it runs.
    browsing: Option<Browse>,
}

/// The non-blank lines of `text`, trimmed.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

fn wine_prefix_text(path: Option<&Path>) -> String {
    path.map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Form {
    fn new() -> Self {
        Self {
            index: None,
            name: String::new(),
            integration: Integration::Extend,
            files: Vec::new(),
            registry: String::new(),
            wine_prefix: String::new(),
            pairs: Vec::new(),
            skips: Vec::new(),
            snapshot: None,
            error: None,
            browsing: None,
        }
    }

    fn edit(index: usize, game: &CustomGame) -> Self {
        Self {
            index: Some(index),
            name: game.name.clone(),
            integration: game.integration,
            files: (game.files.iter())
                .map(|file| FileRow {
                    linux: file.linux.clone().unwrap_or_default(),
                    windows: file.windows.clone().unwrap_or_default(),
                    browse_error: None,
                })
                .collect(),
            registry: game
                .entry
                .registry
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            wine_prefix: wine_prefix_text(game.wine_prefix.as_deref()),
            pairs: game
                .pairs
                .iter()
                .map(|pair| PairRow {
                    linux: pair.linux.clone(),
                    windows: pair.windows.clone(),
                })
                .collect(),
            skips: game.skips.clone(),
            snapshot: Some(game.clone()),
            error: None,
            browsing: None,
        }
    }

    /// The game the form describes. `games` are all custom games, including the one being edited.
    ///
    /// # Errors
    /// [`FormError`] for an empty or taken name, a file row, pair side or skipped folder that
    /// `crossbuild::validate_file`, `validate_pair` or `validate_skip` rejects, and a game that
    /// is no longer the one the form was opened on.
    fn build(&self, games: &[CustomGame]) -> Result<CustomGame, FormError> {
        if let Some(index) = self.index
            && games.get(index) != self.snapshot.as_ref()
        {
            return Err(FormError::Changed);
        }
        let name = self.name.trim();
        if name.is_empty() {
            return Err(FormError::EmptyName);
        }
        let taken = games
            .iter()
            .enumerate()
            .any(|(index, game)| Some(index) != self.index && game.name == name);
        if taken {
            return Err(FormError::DuplicateName(name.to_owned()));
        }
        let files = (self.files.iter().filter_map(FileRow::file))
            .map(|file| crossbuild::validate_file(&file).map(|()| file))
            .collect::<Result<_, _>>()?;
        let pairs = self
            .pairs
            .iter()
            .filter(|row| !(row.linux.trim().is_empty() && row.windows.trim().is_empty()))
            .map(|row| {
                let pair = Pair {
                    linux: row.linux.trim().to_owned(),
                    windows: row.windows.trim().to_owned(),
                };
                crossbuild::validate_pair(&pair).map(|()| pair)
            })
            .collect::<Result<_, _>>()?;
        let skips = (self.skips.iter().map(|skip| skip.trim()))
            .filter(|skip| !skip.is_empty())
            .map(|skip| crossbuild::validate_skip(skip).map(|()| skip.to_owned()))
            .collect::<Result<_, _>>()?;
        let original = self.snapshot.as_ref();
        // `entry.files` is no longer written; a stored one is read as it is (ADR-0016).
        let mut entry = original.map(|game| game.entry.clone()).unwrap_or_default();
        entry.registry = lines(&self.registry)
            .map(|key| {
                let rule = original
                    .and_then(|game| game.entry.registry.get(key))
                    .cloned();
                (key.to_owned(), rule.unwrap_or_default())
            })
            .collect();
        let wine_prefix = self.wine_prefix.trim();
        // Text that was not touched keeps the stored path, which may not be valid UTF-8.
        let kept = original
            .and_then(|game| game.wine_prefix.clone())
            .filter(|path| wine_prefix_text(Some(path)) == self.wine_prefix);
        Ok(CustomGame {
            name: name.to_owned(),
            files,
            entry,
            integration: self.integration,
            wine_prefix: kept
                .or_else(|| (!wine_prefix.is_empty()).then(|| PathBuf::from(wine_prefix))),
            pairs,
            skips,
        })
    }

    /// Starts a folder pick for `side` of file row `row`; [`Form::ui`] fills the row with its
    /// template.
    fn browse(&mut self, ctx: &egui::Context, env: &Env, config: &AppConfig, (row, side): Pick) {
        let pick = Arc::clone(&env.pick_folder);
        let environment = env.environment.clone();
        let config = config.clone();
        let title = self.name.trim().to_owned();
        let entry = self.snapshot.as_ref().map(CustomGame::game_entry);
        if let Some(row) = self.files.get_mut(row) {
            row.browse_error = None;
        }
        let job = Job::spawn(ctx, move || {
            let Some(path) = pick() else {
                return Ok(None);
            };
            let environment = environment.unwrap_or_else(Environment::detect);
            let vars = restore::game_vars(&title, entry.as_ref(), &config, &environment);
            picked_template(&path, side, &environment, &vars).map(Some)
        });
        self.browsing = Some(Browse { row, side, job });
    }

    /// Puts a finished pick into its row.
    fn poll_browse(&mut self) {
        let Some(browse) = &mut self.browsing else {
            return;
        };
        let Some(result) = browse.job.take_result() else {
            return;
        };
        let (index, side) = (browse.row, browse.side);
        self.browsing = None;
        let Some(row) = self.files.get_mut(index) else {
            return;
        };
        match result {
            Ok(Some(template)) => *row.side(side) = template,
            Ok(None) => {}
            Err(error) => row.browse_error = Some((side, format!("{error:#}"))),
        }
    }
}

/// The template of a folder picked for `side` of a file row: through this machine's own folders,
/// or for the Windows side on Linux through the Wine prefix that holds it (ADR-0016).
///
/// # Errors
/// A folder outside every Wine prefix, a prefix whose user cannot be read, unknown native
/// folders, or a folder no template covers.
fn picked_template(
    path: &Path,
    side: Family,
    environment: &Environment,
    vars: &GameVars,
) -> anyhow::Result<String> {
    let ctx = if side == Family::Windows && !cfg!(windows) {
        let prefix =
            prefix::enclosing_prefix(path).context("Pick a folder inside a Wine prefix")?;
        let user = prefix::wine_user(&prefix)?;
        let folders = WinFolders::for_prefix(&prefix, &user);
        Context::WinePrefix { prefix, folders }
    } else {
        (environment.native.clone()).context("The folders of this system are unknown")?
    };
    crossbuild::template_for(path, &ctx, vars).context("No path template covers this folder")
}

/// A side of a file row: its index and family.
type Pick = (usize, Family);

/// What the user did with the form this frame.
enum Action {
    Save,
    Cancel,
    Reload,
    Browse(Pick),
}

/// An error line in the error color.
fn error_line(ui: &mut Ui, text: impl Into<String>) {
    let color = ui.visuals().error_fg_color;
    theme::status(ui, icons::X_CIRCLE, color, text);
}

/// A label followed by a multi-line text field linked to it. Lays out two cells of a grid row.
fn labelled_lines(ui: &mut Ui, label: &str, text: &mut String) {
    let label = ui.label(label);
    ui.add(TextEdit::multiline(text).desired_rows(3))
        .labelled_by(label.id);
}

/// A label and a single-line field linked to it, side by side, narrow enough for two of them
/// and a button on a row of the content column.
fn inline_edit(ui: &mut Ui, label: String, text: &mut String, hint: &str) {
    let label = ui.label(label);
    ui.add(
        TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(260.0),
    )
    .labelled_by(label.id);
}

/// `items` as the rows of a boxed list, each drawn by `row` in a horizontal layout; nothing
/// when there are none.
fn rows<T>(ui: &mut Ui, items: &mut [T], mut row: impl FnMut(&mut Ui, usize, &mut T)) {
    if items.is_empty() {
        return;
    }
    theme::group(ui, |ui| {
        for (index, item) in items.iter_mut().enumerate() {
            if index > 0 {
                theme::row_separator(ui);
            }
            ui.horizontal(|ui| row(ui, index, item));
        }
    });
}

/// How a custom game relates to the manifest, for the list. A title the manifest lacks is a
/// plain custom game whatever its integration.
fn relation(game: &CustomGame, in_manifest: bool) -> &'static str {
    match (in_manifest, game.integration) {
        (false, _) => "custom game",
        (true, Integration::Override) => "overrides the manifest game",
        (true, Integration::Extend) => "extends the manifest game",
    }
}

impl Form {
    /// The file rows, each with a Linux and a Windows path and a Browse button per side. Returns
    /// the side whose Browse was clicked.
    fn files_ui(&mut self, ui: &mut Ui) -> Option<Pick> {
        ui.heading("Files");
        ui.weak(
            "A path on one side is backed up only for that build; with both paths, they hold the \
             same saves in the Linux and the Windows build.",
        );
        let busy = self
            .browsing
            .as_ref()
            .map(|browse| (browse.row, browse.side));
        let mut clicked = None;
        let mut remove = None;
        for (index, row) in self.files.iter_mut().enumerate() {
            theme::group(ui, |ui| {
                ui.label(theme::semibold(format!("File {}", index + 1)));
                for side in [Family::Linux, Family::Windows] {
                    let (label, hint) = match side {
                        Family::Linux => ("Linux path", "<xdgData>/My Game"),
                        Family::Windows => ("Windows path", "<winDocuments>/My Game"),
                    };
                    ui.horizontal(|ui| {
                        let label = ui.label(label);
                        let edit = TextEdit::singleline(row.side(side))
                            .hint_text(hint)
                            .desired_width(360.0);
                        if ui.add(edit).labelled_by(label.id).changed() {
                            row.browse_error.take_if(|(failed, _)| *failed == side);
                        }
                        // A Windows host has no Linux folders to pick from.
                        let can_browse = side == Family::Windows || !cfg!(windows);
                        let idle = busy.is_none();
                        if can_browse && ui.add_enabled(idle, egui::Button::new("Browse")).clicked()
                        {
                            clicked = Some((index, side));
                        }
                        if busy == Some((index, side)) {
                            ui.spinner();
                        }
                    });
                    if let Some((_, message)) = row.browse_error.as_ref().filter(|e| e.0 == side) {
                        error_line(ui, message.clone());
                    }
                }
                if let Some(error) = row.error() {
                    error_line(ui, error.to_string());
                }
                let remove_button = egui::Button::new("Remove file");
                if ui.add_enabled(busy.is_none(), remove_button).clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.files.remove(index);
        }
        if ui
            .add_enabled(busy.is_none(), egui::Button::new("Add file"))
            .clicked()
        {
            self.files.push(FileRow::default());
        }
        clicked
    }

    /// The Linux/Windows pairs, each editable and removable, including those the restore
    /// mapping dialog saved.
    fn pairs_ui(&mut self, ui: &mut Ui) {
        ui.heading("Linux and Windows pairs");
        ui.weak("The same saves in the native Linux build and in the Windows build.");
        let mut remove = None;
        rows(ui, &mut self.pairs, |ui, index, row| {
            let number = index + 1;
            inline_edit(
                ui,
                format!("Linux {number}"),
                &mut row.linux,
                "<xdgData>/Game",
            );
            inline_edit(
                ui,
                format!("Windows {number}"),
                &mut row.windows,
                "<winAppData>/Game",
            );
            if ui.button("Remove pair").clicked() {
                remove = Some(index);
            }
        });
        if let Some(index) = remove {
            self.pairs.remove(index);
        }
        if ui.button("Add pair").clicked() {
            self.pairs.push(PairRow::default());
        }
    }

    /// The root templates a cross-build restore leaves out, each editable and removable,
    /// including those the restore mapping dialog saved.
    fn skips_ui(&mut self, ui: &mut Ui) {
        ui.heading("Skipped folders");
        ui.weak("Folders a restore on the other OS leaves out.");
        let mut remove = None;
        rows(ui, &mut self.skips, |ui, index, skip| {
            inline_edit(
                ui,
                format!("Skipped folder {}", index + 1),
                skip,
                "<xdgData>/Game",
            );
            if ui.button("Remove skipped folder").clicked() {
                remove = Some(index);
            }
        });
        if let Some(index) = remove {
            self.skips.remove(index);
        }
        if ui.button("Add skipped folder").clicked() {
            self.skips.push(String::new());
        }
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<Action> {
        self.poll_browse();
        ui.heading(if self.index.is_some() {
            "Edit custom game"
        } else {
            "New custom game"
        });
        theme::group(ui, |ui| {
            egui::Grid::new("custom_game_form")
                .num_columns(2)
                .show(ui, |ui| {
                    labelled_edit(ui, "Name", &mut self.name, false);
                    ui.end_row();
                    let label = ui.label("Integration");
                    egui::ComboBox::from_id_salt("custom_game_integration")
                        .selected_text(integration_label(self.integration))
                        .show_ui(ui, |ui| {
                            theme::pill_list(ui);
                            for integration in [Integration::Extend, Integration::Override] {
                                ui.selectable_value(
                                    &mut self.integration,
                                    integration,
                                    integration_label(integration),
                                );
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    ui.end_row();
                    labelled_lines(ui, "Registry keys", &mut self.registry);
                    ui.end_row();
                    labelled_edit(ui, "Wine prefix", &mut self.wine_prefix, false);
                    ui.end_row();
                });
        });
        ui.add_space(16.0);
        let browse = self.files_ui(ui);
        ui.add_space(16.0);
        self.pairs_ui(ui);
        ui.add_space(16.0);
        self.skips_ui(ui);
        if let Some(error) = &self.error {
            error_line(ui, error.to_string());
        }
        let mut action = browse.map(Action::Browse);
        let valid = self.files.iter().all(|row| row.error().is_none());
        theme::row_end(ui, |ui| {
            if self.error == Some(FormError::Changed) && ui.button("Reload").clicked() {
                action = Some(Action::Reload);
            }
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
            if theme::primary(ui, valid, Some(icons::FLOPPY_DISK), "Save game").clicked() {
                action = Some(Action::Save);
            }
        });
        action
    }
}

/// The state of the Custom games tab.
#[derive(Default)]
pub struct CustomGamesTab {
    form: Option<Form>,
}

impl CustomGamesTab {
    /// Draws the tab. Saving or removing a game saves the config.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        theme::page(ui, |ui| {
            let manifest = state.manifest.as_ref();
            let env = &state.env;
            let Some(config) = &mut state.config else {
                ui.label("Choose a repository first.");
                return;
            };
            let mut changed = false;
            let (mut edit, mut remove) = (None, None);
            if config.custom_games.is_empty() {
                ui.weak(
                    "No custom games. Add one for a game the manifest misses, or to change where \
                     a known game keeps its saves.",
                );
            } else {
                theme::group(ui, |ui| {
                    for (index, game) in config.custom_games.iter().enumerate() {
                        if index > 0 {
                            theme::row_separator(ui);
                        }
                        let in_manifest =
                            manifest.is_some_and(|m| m.games.contains_key(&game.name));
                        let relation = relation(game, in_manifest);
                        theme::row(
                            ui,
                            |ui| {
                                ui.label(format!("{} ({relation})", game.name));
                            },
                            |ui| {
                                // One form at a time, so opening another cannot drop unsaved
                                // edits.
                                let idle = self.form.is_none();
                                if ui.add_enabled(idle, egui::Button::new("Edit")).clicked() {
                                    edit = Some(index);
                                }
                                if ui.button("Remove").clicked() {
                                    remove = Some(index);
                                }
                            },
                        );
                    }
                });
            }
            if let Some(index) = edit {
                self.form = Some(Form::edit(index, &config.custom_games[index]));
            }
            if let Some(index) = remove {
                config.custom_games.remove(index);
                // The open form follows its game; a later game moved up by one.
                let open = self.form.as_ref().and_then(|form| form.index);
                match (open, &mut self.form) {
                    (Some(open), _) if open == index => self.form = None,
                    (Some(open), Some(form)) if open > index => form.index = Some(open - 1),
                    _ => {}
                }
                changed = true;
            }
            let adding = ui.add_enabled(self.form.is_none(), egui::Button::new("Add game"));
            if adding.clicked() {
                self.form = Some(Form::new());
            }
            if let Some(form) = &mut self.form {
                ui.add_space(16.0);
                match form.ui(ui) {
                    Some(Action::Save) => match form.build(&config.custom_games) {
                        Ok(game) => {
                            match form.index {
                                Some(index) => config.custom_games[index] = game,
                                None => config.custom_games.push(game),
                            }
                            self.form = None;
                            changed = true;
                        }
                        Err(error) => form.error = Some(error),
                    },
                    Some(Action::Cancel) => self.form = None,
                    Some(Action::Reload) => {
                        self.form = form.index.and_then(|index| {
                            Some(Form::edit(index, config.custom_games.get(index)?))
                        });
                    }
                    Some(Action::Browse(pick)) => form.browse(ui.ctx(), env, config, pick),
                    None => {}
                }
            }
            if changed {
                state.save_config(ui.ctx());
            }
        });
    }
}

#[cfg(test)]
mod tests;
