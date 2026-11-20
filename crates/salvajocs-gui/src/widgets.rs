//! Controls shared by several screens.

use eframe::egui::{
    Button, Color32, Grid, Label, Response, RichText, Role, TextEdit, Ui, WidgetInfo,
};

use crate::theme;

pub mod backend;
pub mod compression;

/// The URL of a page under `docs/guide/` at the tag of the running version, so the page matches
/// what this build does. A macro so that it can build a `const`.
macro_rules! guide_url {
    ($page:literal) => {
        concat!(
            "https://github.com/jmigual/salvajocs/blob/v",
            env!("CARGO_PKG_VERSION"),
            "/docs/guide/",
            $page
        )
    };
}
pub(crate) use guide_url;

/// A label followed by a single-line text field linked to it, so the field is found by that
/// label. `password` masks the text. Lays out two cells of a grid row.
pub fn labelled_edit(ui: &mut Ui, label: &str, text: &mut String, password: bool) -> Response {
    let label = ui.label(label);
    ui.add(TextEdit::singleline(text).password(password))
        .labelled_by(label.id)
}

/// A button showing `text` whose accessible name is `name`, so a button repeated on every row
/// can still be told apart.
pub fn named_button(ui: &mut Ui, enabled: bool, text: &str, name: &str) -> Response {
    let response = ui.add_enabled(enabled, Button::new(text));
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
    response
}

/// A two-column form grid whose rows are only as tall as their contents, so a hint row sits
/// close under its field.
pub fn form_grid(id: &str) -> Grid {
    Grid::new(id)
        .num_columns(2)
        .min_row_height(0.0)
        .spacing([8.0, 6.0])
}

/// A warning or error line in a cell of a grid row, as tall as its wrapped text. Bare
/// `theme::status` in a cell reports one line, and the next row is drawn over the rest.
pub fn field_status(ui: &mut Ui, glyph: &'static str, color: Color32, text: impl Into<String>) {
    ui.vertical(|ui| {
        ui.set_max_width(ui.spacing().text_edit_width);
        theme::status(ui, glyph, color, text);
    });
}

/// A muted hint under the field of the grid row above, as wide as a field. Lays out one cell of
/// a grid row; the gaps to the rows around it are the grid's, the same as between fields.
pub fn field_hint(ui: &mut Ui, hint: &str) {
    ui.vertical(|ui| {
        ui.set_max_width(ui.spacing().text_edit_width);
        ui.add(Label::new(RichText::new(hint).weak()).wrap());
    });
}
