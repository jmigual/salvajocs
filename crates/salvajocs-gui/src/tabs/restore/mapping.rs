//! The cross-build mapping dialog: where each snapshot root of the other OS's build goes in this
//! one, shown only while a root is unknown (ADR-0011).

use eframe::egui::{self, Color32, TextEdit, Ui};
use salvajocs_core::crossbuild::{self, Choice, Confidence, MappingEntry, SnapshotRoot};

/// One root of the game.
struct Row {
    entry: MappingEntry,
    /// The target template as typed, picked or proposed.
    target: String,
    skip: bool,
    /// Why `prepare` refused the target.
    error: Option<String>,
}

/// What the user did with the dialog this frame.
pub(super) enum DialogAction {
    /// Every root has a choice; prepare the restore with them.
    Confirm(Vec<(SnapshotRoot, Choice)>),
    Cancel,
}

/// The rows of the dialog, the known roots prefilled.
pub(super) struct Dialog {
    rows: Vec<Row>,
}

fn confidence_label(entry: &MappingEntry) -> &'static str {
    match (entry.confidence, &entry.proposed) {
        (Confidence::Saved, Some(_)) => "Saved pair",
        (Confidence::Saved, None) => "Saved as skipped",
        (Confidence::EngineRule, _) => "Engine layout (Unity or Godot)",
        (Confidence::AutoPair, _) => "The manifest's only save paths of each build",
        (Confidence::Unknown, _) => "Unknown: choose a target or skip this folder",
    }
}

impl Dialog {
    /// A dialog over `entries`, every root of the game from `PrepareError::NeedsMapping`.
    pub(super) fn new(entries: Vec<MappingEntry>) -> Self {
        let rows = entries
            .into_iter()
            .map(|entry| Row {
                target: entry.proposed.clone().unwrap_or_default(),
                skip: entry.confidence == Confidence::Saved && entry.proposed.is_none(),
                entry,
                error: None,
            })
            .collect();
        Self { rows }
    }

    /// Shows `prepare`'s refusal of `template` on the row of `root`.
    pub(super) fn reject(&mut self, root: &SnapshotRoot, template: &str) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.entry.root == *root) {
            row.error = Some(format!(
                "{template} is not a placeholder template of this OS, such as <xdgData>/Game"
            ));
        }
    }

    /// A choice for every root; `None` while a row has neither a target nor skip.
    fn choices(&self) -> Option<Vec<(SnapshotRoot, Choice)>> {
        (self.rows.iter())
            .map(|row| {
                let target = row.target.trim();
                let choice = match (row.skip, target.is_empty()) {
                    (true, _) => Choice::Skip,
                    (false, false) => Choice::Template(target.to_owned()),
                    (false, true) => return None,
                };
                Some((row.entry.root.clone(), choice))
            })
            .collect()
    }

    /// Draws the dialog for the game `title`; Confirm waits for `busy` to end.
    pub(super) fn ui(&mut self, ui: &mut Ui, title: &str, busy: bool) -> Option<DialogAction> {
        ui.heading(format!("Where should the saves of {title} go?"));
        ui.label(
            "This snapshot holds folders of the other OS's build. Choose where each goes in \
             this build, or skip it. The choices are saved with the game and can be changed in \
             Custom games.",
        );
        egui::ScrollArea::vertical()
            .max_height(480.0)
            .show(ui, |ui| {
                for (index, row) in self.rows.iter_mut().enumerate() {
                    ui.separator();
                    row_ui(ui, index + 1, row);
                }
            });
        ui.separator();
        let choices = self.choices();
        let mut action = None;
        ui.horizontal(|ui| {
            let ready = choices.is_some() && !busy;
            if ui
                .add_enabled(ready, egui::Button::new("Confirm"))
                .clicked()
            {
                action = choices.map(DialogAction::Confirm);
            }
            if ui.button("Cancel").clicked() {
                action = Some(DialogAction::Cancel);
            }
        });
        action
    }
}

fn row_ui(ui: &mut Ui, number: usize, row: &mut Row) {
    let entry = &row.entry;
    ui.strong(format!(
        "{} ({} files)",
        crossbuild::root_template(&entry.root),
        entry.files
    ));
    ui.label(confidence_label(entry));
    let mut edited = false;
    ui.horizontal(|ui| {
        let label = ui.label(format!("Target {number}"));
        let field = TextEdit::singleline(&mut row.target).hint_text("<xdgData>/Game");
        edited |= ui.add(field).labelled_by(label.id).changed();
        let label = ui.label(format!("Candidates {number}"));
        egui::ComboBox::from_id_salt(("restore_candidates", number))
            .selected_text("the manifest's paths")
            .show_ui(ui, |ui| {
                for candidate in &row.entry.candidates {
                    if ui.selectable_label(false, candidate).clicked() {
                        row.target.clone_from(candidate);
                        row.skip = false;
                        edited = true;
                    }
                }
            })
            .response
            .labelled_by(label.id);
        edited |= ui
            .checkbox(&mut row.skip, format!("Skip {number}"))
            .changed();
    });
    for note in &row.entry.notes {
        ui.colored_label(Color32::YELLOW, note);
    }
    if edited {
        row.error = None;
    }
    if let Some(error) = &row.error {
        ui.colored_label(Color32::LIGHT_RED, error);
    }
}
