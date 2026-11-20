//! The bar along the bottom of the window: the manifest status on the left and the update notice
//! on the right. On the Backup tab it also holds the repository and the backup button.

use eframe::egui::{self, Align, Layout, Margin, Rect, Ui, UiBuilder, Vec2};

use crate::theme;

/// The bar's margins. The vertical one is no smaller than the resize edge, so a press on a button
/// never resizes the window.
const MARGIN: Margin = Margin::symmetric(16, 4);
/// The height of the bar's controls, shorter than the top bar's, so this bar is the thinner one.
const ROW: f32 = 28.0;

/// Takes the bar's room at the bottom of `ui`, the whole window, and returns it for [`show`]. A
/// `tall` bar's row is as tall as the theme's controls, to fit the Backup tab's button. Call it
/// before the central panel, so the bar stays below everything else.
pub fn reserve(ui: &mut Ui, tall: bool) -> Rect {
    let row = if tall {
        ui.spacing().interact_size.y
    } else {
        ROW
    };
    let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(MARGIN);
    // Exact: a panel otherwise lays out at last frame's height, a frame off when it changes.
    let panel = egui::Panel::bottom("status_bar")
        .frame(frame)
        .exact_size(row + frame.total_margin().sum().y)
        .show(ui, |ui| {
            ui.allocate_space(Vec2::new(ui.available_width(), row));
        });
    panel.response.rect
}

/// Draws the bar's row in `bar`, the room [`reserve`] took, with `start` from the left edge and
/// `end` against the right edge, as [`theme::row`] lays them out, and returns what `end` returned.
/// Call it after the central panel, so keyboard focus and screen readers reach the bar after the
/// page.
pub fn show<R>(
    ui: &mut Ui,
    bar: Rect,
    start: impl FnOnce(&mut Ui),
    end: impl FnMut(&mut Ui) -> R,
) -> R {
    let row = bar - MARGIN;
    let builder = UiBuilder::new()
        .id_salt("status_bar")
        .max_rect(row)
        .layout(Layout::left_to_right(Align::Center));
    let mut ui = ui.new_child(builder);
    let spacing = ui.spacing_mut();
    // A window shorter than the margins leaves the row a negative height.
    spacing.interact_size.y = row.height().max(0.0);
    spacing.button_padding.y = 4.0;
    theme::row(&mut ui, start, end)
}

/// Whether text in `width` shows more than a bare ellipsis: it has at least a button's width.
pub fn text_fits(ui: &Ui, width: f32) -> bool {
    width >= ui.spacing().interact_size.x
}

/// Draws `text`, a piece of the bar's status, only if [`text_fits`] in what is left of the row.
pub fn status_text(ui: &mut Ui, text: impl FnOnce(&mut Ui)) {
    if text_fits(ui, ui.available_width()) {
        text(ui);
    }
}
