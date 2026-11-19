//! The title bar the app draws itself, as the window has no OS decorations: the app icon, moving
//! the window by its bar, the minimize, maximize and close buttons, and resizing from the edges.

use eframe::egui::{
    self, Align, Area, Button, ColorImage, CursorIcon, Event, Id, Layout, Margin, Order,
    PointerButton, Pos2, Rect, Response, Sense, Stroke, TextWrapMode, TextureHandle,
    TextureOptions, Ui, Vec2, ViewportCommand, viewport::ResizeDirection,
};

use crate::theme::{self, icons};

/// The size of each window button.
const BUTTON: Vec2 = Vec2::new(40.0, 32.0);
/// How far from the window edge the pointer resizes the window.
const EDGE: f32 = 4.0;
/// How far the pointer moves from the press on the bar before the window moves, so the jitter of
/// a double click does not.
const DRAG_START: f32 = 3.0;

/// Whether the window fills the screen; the backend may not say, which counts as no.
fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Makes the bar's row, from the cursor of `ui` and as tall as a row of controls, move the
/// window when dragged and maximize or restore it when double-clicked, and returns the bar.
/// `margin` is the bar's own, which also counts. Call it before the row's widgets, so they take
/// the pointer first.
pub fn drag_area(ui: &mut Ui, margin: Margin) -> Rect {
    let row = Rect::from_min_size(
        ui.cursor().min,
        Vec2::new(ui.available_width(), ui.spacing().interact_size.y),
    );
    let bar = row + margin;
    let response = ui.interact(bar, Id::unique("title_bar"), Sense::click_and_drag());
    move_window(ui.ctx(), &response);
    bar
}

/// Moves the window when `response` is dragged and maximizes or restores it when double-clicked,
/// both with the primary button.
fn move_window(ctx: &egui::Context, response: &Response) {
    let (pressed, down, moved, focused, focus_lost) = ctx.input(|i| {
        (
            i.pointer.button_pressed(PointerButton::Primary),
            i.pointer.button_down(PointerButton::Primary),
            i.pointer
                .press_origin()
                .zip(i.pointer.latest_pos())
                .is_some_and(|(origin, pos)| origin.distance(pos) >= DRAG_START),
            i.focused,
            i.events.contains(&Event::WindowFocused(false)),
        )
    });
    // Only a press on the bar itself moves the window, not one on a tab or button over it. One
    // slot for the bar and its stand-in, as `drag_area` rewrites it on every press.
    let key = Id::unique("title_bar_press");
    if pressed || focus_lost {
        let hit = ctx.viewport(|viewport| viewport.hits.click.map(|hit| hit.id));
        ctx.data_mut(|data| data.insert_temp(key, hit.filter(|_| pressed)));
    }
    let armed = ctx.data(|data| data.get_temp::<Option<Id>>(key)).flatten() == Some(response.id);
    if response.double_clicked_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
    } else if armed && down && moved && focused {
        // Not drag_started: it waits for 6 pt or 0.8 s. egui-winit ignores StartDrag without focus.
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        // Once per press: the compositor keeps the release, so egui still sees the button down.
        ctx.data_mut(|data| data.insert_temp(key, None::<Id>));
    }
}

/// The app icon, as tall as the tab icons.
pub fn app_icon(ui: &mut Ui) {
    let id = Id::unique("app_icon");
    let texture = match ui.ctx().data(|data| data.get_temp::<TextureHandle>(id)) {
        Some(texture) => texture,
        None => {
            let icon = eframe::icon_data::from_png_bytes(crate::ICON_PNG)
                .expect("the embedded icon is a valid PNG");
            let size = [icon.width as usize, icon.height as usize];
            let image = ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
            // Mipmaps, or the 256 px icon shimmers at 20 px.
            let options =
                TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
            let texture = ui.ctx().load_texture("app_icon", image, options);
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        }
    };
    ui.add(egui::Image::new((texture.id(), Vec2::splat(20.0))));
}

/// The width of the three window buttons.
const BUTTONS_WIDTH: f32 = 3.0 * BUTTON.x;

/// Takes the room [`controls`] draws the window buttons in, at the end of the bar's row, and
/// returns it.
pub fn reserve_buttons(ui: &mut Ui) -> Rect {
    ui.allocate_exact_size(Vec2::new(BUTTONS_WIDTH, BUTTON.y), Sense::hover())
        .0
}

/// The width left in the bar's row for [`status_area`], once the window buttons and the gaps
/// around them are taken. Call it after the tabs, which keep their width.
pub fn status_room(ui: &Ui) -> f32 {
    (ui.available_width() - BUTTONS_WIDTH - 2.0 * ui.spacing().item_spacing.x).max(0.0)
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

/// Draws the bar's status with `content` in at most `room`. Text that does not fit is
/// truncated with an ellipsis and shows in full on hover.
pub fn status_area(ui: &mut Ui, room: f32, content: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.set_max_width(room);
        ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
        content(ui);
    });
}

/// The window buttons in `slot`, the room [`reserve_buttons`] took, the resize edges and the
/// window border, and, while a dialog is open, a stand-in for [`drag_area`]'s `bar`. `ui` is the
/// whole window. Call it after everything else, dialogs included: the buttons, the bar and the
/// edges stay usable while a dialog blocks the rest of the window.
pub fn controls(ui: &Ui, bar: Rect, slot: Rect) {
    let ctx = ui.ctx();
    let maximized = maximized(ctx);
    let window = ctx.viewport_rect();
    if ctx.memory(|memory| memory.top_modal_layer()).is_some() {
        let mut bar = bar.with_max_x(slot.left());
        if !maximized {
            // Clear of the resize edges, or a press there would also move the window.
            bar = bar.intersect(window.shrink(EDGE));
        }
        Area::new(Id::unique("title_bar_over_dialog"))
            .order(Order::Tooltip)
            .movable(false)
            .fixed_pos(bar.min)
            .show(ctx, |ui| {
                move_window(ctx, &ui.allocate_rect(bar, Sense::click_and_drag()));
            });
    }
    let (pos, height) = if maximized {
        // Flush in the corner, so the pointer thrown there hits Close.
        let pos = Pos2::new(window.right() - slot.width(), window.top());
        (pos, 2.0 * (slot.center().y - window.top()))
    } else {
        (slot.min, slot.height())
    };
    Area::new(Id::unique("window_buttons"))
        // Above the dialogs' foreground layer, which blocks every layer below it.
        .order(Order::Tooltip)
        .movable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            // Buttons place their icon by the layout's alignment; bounded, so the area, and its
            // hold on the pointer, does not stretch over the window.
            let layout = Layout::left_to_right(Align::Center);
            ui.allocate_ui_with_layout(Vec2::new(slot.width(), height), layout, |ui| {
                buttons(ui, maximized, height);
            });
        });
    if !maximized {
        resize_edges(ui);
        let stroke = ctx.global_style().visuals.window_stroke;
        ctx.layer_painter(egui::LayerId::new(
            Order::Tooltip,
            Id::unique("window_border"),
        ))
        .rect_stroke(window, 0.0, stroke, egui::StrokeKind::Inside);
    }
}

/// The window buttons, each `height` tall.
fn buttons(ui: &mut Ui, maximized: bool, height: f32) {
    ui.spacing_mut().item_spacing.x = 0.0;
    theme::pill_list(ui);
    let size = Vec2::new(BUTTON.x, height);
    if button(ui, size, icons::MINUS, "Minimize").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    let (glyph, label) = if maximized {
        (icons::COPY, "Restore window")
    } else {
        (icons::SQUARE, "Maximize")
    };
    if button(ui, size, glyph, label).clicked() {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    let palette = theme::palette(ui);
    let widgets = &mut ui.visuals_mut().widgets;
    for state in [&mut widgets.hovered, &mut widgets.active] {
        state.weak_bg_fill = palette.error;
        state.bg_fill = palette.error;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke.color = palette.bg;
    }
    if button(ui, size, icons::X, "Close").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
}

/// A frameless icon button of `size` named `label`, which is also its tooltip.
fn button(ui: &mut Ui, size: Vec2, glyph: &'static str, label: &str) -> Response {
    let button = Button::new(theme::icon(glyph))
        .frame_when_inactive(false)
        .min_size(size);
    ui.add(button).on_hover_text(label)
}

/// Shows the resize cursor on the window edges and starts the resize on a press there.
fn resize_edges(ui: &Ui) {
    // Last on the window's layer, so the edges take the press from the widgets under them, such as
    // a scroll bar, and from those within egui's interact radius of them.
    let window = ui.ctx().viewport_rect();
    for (index, edge) in [
        window.with_max_y(window.top() + EDGE),
        window.with_min_y(window.bottom() - EDGE),
        window.with_max_x(window.left() + EDGE),
        window.with_min_x(window.right() - EDGE),
    ]
    .into_iter()
    .enumerate()
    {
        ui.interact(
            edge,
            Id::unique(("resize_edge", index)),
            Sense::click_and_drag(),
        );
    }
    // From the input rather than the edges' responses: a dialog blocks those.
    let ctx = ui.ctx();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let Some(direction) = resize_direction(ctx.viewport_rect(), pos) else {
        return;
    };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// The edge or corner of `window` that `pos` resizes, or `None` away from the edges.
fn resize_direction(window: Rect, pos: Pos2) -> Option<ResizeDirection> {
    let west = pos.x < window.left() + EDGE;
    let east = pos.x >= window.right() - EDGE;
    let north = pos.y < window.top() + EDGE;
    let south = pos.y >= window.bottom() - EDGE;
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (.., true, _) => ResizeDirection::West,
        (.., true) => ResizeDirection::East,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_direction_follows_the_edges_and_corners() {
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        for (pos, direction) in [
            ((0.0, 0.0), Some(ResizeDirection::NorthWest)),
            ((799.0, 0.0), Some(ResizeDirection::NorthEast)),
            ((0.0, 599.0), Some(ResizeDirection::SouthWest)),
            ((799.0, 599.0), Some(ResizeDirection::SouthEast)),
            ((400.0, 1.0), Some(ResizeDirection::North)),
            ((400.0, 598.0), Some(ResizeDirection::South)),
            ((2.0, 300.0), Some(ResizeDirection::West)),
            ((797.0, 300.0), Some(ResizeDirection::East)),
            ((400.0, 300.0), None),
            ((EDGE, EDGE), None),
        ] {
            assert_eq!(
                resize_direction(window, Pos2::from(pos)),
                direction,
                "at {pos:?}"
            );
        }
    }
}
