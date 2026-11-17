//! The Custom games tab: games the user adds to the manifest or overrides in it (ADR-0008), with
//! the Linux/Windows path pairs cross-build restores use (ADR-0011).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, TextEdit, Ui};
use salvajocs_core::crossbuild::{self, PairError, SkipError};
use salvajocs_core::manifest::{CustomGame, FileRule, Integration, Pair, Tag};

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

/// The game being added or edited.
struct Form {
    /// Position in `custom_games` of the game being edited; `None` for a new one.
    index: Option<usize>,
    name: String,
    integration: Integration,
    /// One file template per line.
    files: String,
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
            files: String::new(),
            registry: String::new(),
            wine_prefix: String::new(),
            pairs: Vec::new(),
            skips: Vec::new(),
            snapshot: None,
            error: None,
        }
    }

    fn edit(index: usize, game: &CustomGame) -> Self {
        Self {
            index: Some(index),
            name: game.name.clone(),
            integration: game.integration,
            files: game
                .entry
                .files
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
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
        }
    }

    /// The game the form describes. `games` are all custom games, including the one being edited.
    ///
    /// # Errors
    /// [`FormError`] for an empty or taken name, a pair side or skipped folder that
    /// `crossbuild::validate_pair` or `validate_skip` rejects, and a game that is no longer the
    /// one the form was opened on.
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
        let mut entry = original.map(|game| game.entry.clone()).unwrap_or_default();
        entry.files = lines(&self.files)
            .map(|template| {
                let rule = original
                    .and_then(|game| game.entry.files.get(template))
                    .cloned();
                let save = || FileRule {
                    tags: vec![Tag::Save],
                    when: Vec::new(),
                };
                (template.to_owned(), rule.unwrap_or_else(save))
            })
            .collect();
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
            entry,
            integration: self.integration,
            wine_prefix: kept
                .or_else(|| (!wine_prefix.is_empty()).then(|| PathBuf::from(wine_prefix))),
            pairs,
            skips,
        })
    }
}

/// What the user did with the form this frame.
enum Action {
    Save,
    Cancel,
    Reload,
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
                    labelled_lines(ui, "Files", &mut self.files);
                    ui.end_row();
                    labelled_lines(ui, "Registry keys", &mut self.registry);
                    ui.end_row();
                    labelled_edit(ui, "Wine prefix", &mut self.wine_prefix, false);
                    ui.end_row();
                });
            ui.weak(
                "One path template per line, such as <winDocuments>/My Game/*.sav. Editing a \
                 line resets its OS condition; use a pair instead.",
            );
        });
        ui.add_space(16.0);
        self.pairs_ui(ui);
        ui.add_space(16.0);
        self.skips_ui(ui);
        if let Some(error) = &self.error {
            let color = ui.visuals().error_fg_color;
            theme::status(ui, icons::X_CIRCLE, color, error.to_string());
        }
        let mut action = None;
        theme::row_end(ui, |ui| {
            if theme::primary(ui, true, Some(icons::FLOPPY_DISK), "Save game").clicked() {
                action = Some(Action::Save);
            }
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
            if self.error == Some(FormError::Changed) && ui.button("Reload").clicked() {
                action = Some(Action::Reload);
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
                        // Right to left: the row's actions sit at its end.
                        ui.horizontal(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Remove").clicked() {
                                    remove = Some(index);
                                }
                                // One form at a time, so opening another cannot drop unsaved
                                // edits.
                                let idle = self.form.is_none();
                                if ui.add_enabled(idle, egui::Button::new("Edit")).clicked() {
                                    edit = Some(index);
                                }
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    let in_manifest =
                                        manifest.is_some_and(|m| m.games.contains_key(&game.name));
                                    let relation = relation(game, in_manifest);
                                    ui.label(format!("{} ({relation})", game.name));
                                });
                            });
                        });
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
