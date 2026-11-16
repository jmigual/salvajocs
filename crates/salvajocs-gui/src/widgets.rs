//! Controls shared by several screens.

use eframe::egui::{Response, TextEdit, Ui};

pub mod backend;
pub mod compression;

/// A label followed by a single-line text field linked to it, so the field is found by that
/// label. `password` masks the text. Lays out two cells of a grid row.
pub fn labelled_edit(ui: &mut Ui, label: &str, text: &mut String, password: bool) -> Response {
    let label = ui.label(label);
    ui.add(TextEdit::singleline(text).password(password))
        .labelled_by(label.id)
}
