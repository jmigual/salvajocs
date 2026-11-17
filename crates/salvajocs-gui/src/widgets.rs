//! Controls shared by several screens.

use eframe::egui::{Grid, Label, Response, RichText, TextEdit, Ui};

pub mod backend;
pub mod compression;

/// A label followed by a single-line text field linked to it, so the field is found by that
/// label. `password` masks the text. Lays out two cells of a grid row.
pub fn labelled_edit(ui: &mut Ui, label: &str, text: &mut String, password: bool) -> Response {
    let label = ui.label(label);
    ui.add(TextEdit::singleline(text).password(password))
        .labelled_by(label.id)
}

/// A two-column form grid whose rows are only as tall as their contents, so a hint row sits
/// close under its field.
pub fn form_grid(id: &str) -> Grid {
    Grid::new(id)
        .num_columns(2)
        .min_row_height(0.0)
        .spacing([8.0, 6.0])
}

/// A muted hint under the field of the grid row above, as wide as a field and with room below
/// it, so it reads as part of that field. Lays out one cell of a grid row.
pub fn field_hint(ui: &mut Ui, hint: &str) {
    ui.vertical(|ui| {
        ui.set_max_width(ui.spacing().text_edit_width);
        ui.add(Label::new(RichText::new(hint).weak()).wrap());
        ui.add_space(8.0);
    });
}
