//! The look of the app: the colors of the dark and light themes, the fonts, the type scale and
//! the spacing. The app follows the system theme.

use std::sync::Arc;

use eframe::egui::epaint::text::VariationCoords;
use eframe::egui::{
    self, Align2, Atom, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily,
    FontId, FontTweak, Frame, Label, Margin, Rect, Response, RichText, ScrollArea, Sense,
    Separator, Stroke, Style, TextStyle, Theme, Ui, UiBuilder, Vec2, Visuals,
};

/// The named font family of the semibold face, for headings, primary buttons and tabs.
const SEMIBOLD: &str = "semibold";
/// The named font family of the icon subset; not in the proportional fallbacks, so a
/// private-use codepoint in plain text never turns into an icon.
const ICONS: &str = "icons";

/// Phosphor icon codepoints in `assets/fonts/salvajocs-icons.ttf`.
pub mod icons {
    pub const FLOPPY_DISK: &str = "\u{E248}";
    pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{E1A0}";
    pub const GAME_CONTROLLER: &str = "\u{E26E}";
    pub const GEAR_SIX: &str = "\u{E272}";
    pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const X_CIRCLE: &str = "\u{E4F8}";
    pub const CHECK_CIRCLE: &str = "\u{E184}";
    pub const LOCK_SIMPLE: &str = "\u{E308}";
    pub const LOCK_SIMPLE_OPEN: &str = "\u{E30A}";
    pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
    pub const ARROW_CIRCLE_UP: &str = "\u{E030}";
}

/// The colors of one theme. Contrast floors (WCAG) are checked by the tests.
pub struct Palette {
    /// The window and panel background.
    pub bg: Color32,
    /// Boxed lists, dialogs and menus.
    pub surface: Color32,
    /// Secondary button fill and row hover.
    pub surface_hover: Color32,
    /// The background of text fields.
    pub field: Color32,
    pub text: Color32,
    /// Hints, summaries and other secondary text.
    pub muted: Color32,
    /// The fill of primary buttons, under white text.
    pub accent_fill: Color32,
    pub accent_fill_hover: Color32,
    pub accent_fill_pressed: Color32,
    /// The brand green, for marks that are not text on a fill.
    pub accent: Color32,
    /// Links.
    pub accent_text: Color32,
    /// Selected tabs and rows, and selected text.
    pub selection_bg: Color32,
    /// The text of selected tabs and rows, and the outline of the focused widget.
    pub selection_stroke: Color32,
    /// The outline of text fields, checkboxes and secondary buttons.
    pub input_stroke: Color32,
    /// Hairlines between rows and around boxed lists.
    pub divider: Color32,
    pub warn: Color32,
    pub error: Color32,
}

/// The dark theme: a green-tinted charcoal with the icon's greens.
pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x1b, 0x21, 0x1f),
    surface: Color32::from_rgb(0x23, 0x2a, 0x27),
    surface_hover: Color32::from_rgb(0x2b, 0x33, 0x2f),
    field: Color32::from_rgb(0x1b, 0x21, 0x1f),
    text: Color32::from_rgb(0xe6, 0xeb, 0xe8),
    muted: Color32::from_rgb(0x9a, 0xa6, 0xa0),
    accent_fill: Color32::from_rgb(0x1c, 0x7a, 0x4f),
    accent_fill_hover: Color32::from_rgb(0x1a, 0x71, 0x49),
    accent_fill_pressed: Color32::from_rgb(0x17, 0x68, 0x3f),
    accent: Color32::from_rgb(0x26, 0xa2, 0x69),
    accent_text: Color32::from_rgb(0x8f, 0xd8, 0xac),
    selection_bg: Color32::from_rgb(0x1d, 0x4a, 0x34),
    selection_stroke: Color32::from_rgb(0x8f, 0xd8, 0xac),
    input_stroke: Color32::from_rgb(0x75, 0x82, 0x7c),
    divider: Color32::from_rgb(0x34, 0x3d, 0x39),
    warn: Color32::from_rgb(0xe8, 0xc4, 0x6b),
    error: Color32::from_rgb(0xff, 0x8a, 0x80),
};

/// The light theme: a cool off-white with the icon's shadow green.
pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf4, 0xf7, 0xf5),
    surface: Color32::WHITE,
    surface_hover: Color32::from_rgb(0xee, 0xf2, 0xef),
    field: Color32::WHITE,
    text: Color32::from_rgb(0x1b, 0x21, 0x1f),
    muted: Color32::from_rgb(0x5a, 0x66, 0x60),
    accent_fill: Color32::from_rgb(0x1c, 0x7a, 0x4f),
    accent_fill_hover: Color32::from_rgb(0x1a, 0x71, 0x49),
    accent_fill_pressed: Color32::from_rgb(0x17, 0x68, 0x3f),
    accent: Color32::from_rgb(0x1c, 0x7a, 0x4f),
    accent_text: Color32::from_rgb(0x1c, 0x7a, 0x4f),
    selection_bg: Color32::from_rgb(0xd3, 0xed, 0xdf),
    selection_stroke: Color32::from_rgb(0x17, 0x68, 0x3f),
    input_stroke: Color32::from_rgb(0x7d, 0x89, 0x83),
    divider: Color32::from_rgb(0xe1, 0xe7, 0xe3),
    warn: Color32::from_rgb(0x8a, 0x5a, 0x00),
    error: Color32::from_rgb(0xc0, 0x1c, 0x28),
};

/// Installs the fonts and both themes' visuals, type scale and spacing.
///
/// Call it before any text of the frame is laid out, so the fonts apply in that same frame.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, Visuals::dark()));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, Visuals::light()));
    ctx.all_styles_mut(style);
}

/// egui's default fonts with Atkinson Hyperlegible Next in front of the proportional family, its
/// semibold instance as the `semibold` family, and the icon subset as the `icons` family.
fn fonts() -> FontDefinitions {
    const ATKINSON: &[u8] = include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext.ttf");
    let mut fonts = FontDefinitions::default();
    // The variable font's default instance is the regular weight, 400.
    fonts.font_data.insert(
        "atkinson".to_owned(),
        Arc::new(FontData::from_static(ATKINSON)),
    );
    // `from_static` borrows the bytes, so the second instance adds nothing to the binary.
    fonts.font_data.insert(
        "atkinson-semibold".to_owned(),
        Arc::new(FontData::from_static(ATKINSON).tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", 600.0)]),
            ..Default::default()
        })),
    );
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    let fallbacks = proportional.clone();
    proportional.insert(0, "atkinson".to_owned());
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        std::iter::once("atkinson-semibold".to_owned())
            .chain(fallbacks)
            .collect(),
    );
    fonts.font_data.insert(
        ICONS.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/salvajocs-icons.ttf"
        ))),
    );
    fonts
        .families
        .insert(FontFamily::Name(ICONS.into()), vec![ICONS.to_owned()]);
    fonts
}

/// The palette of the theme `ui` is drawn in.
pub fn palette(ui: &Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

/// The family `name` if the fonts define it. Screens drawn without [`apply`], as in tab-level
/// tests, have only egui's fonts, and a family without fonts cannot draw anything.
fn family(ctx: &egui::Context, name: &str) -> Option<FontFamily> {
    let family = FontFamily::Name(name.into());
    ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family))
        .then_some(family)
}

/// `text` in the semibold face.
pub fn semibold(ui: &Ui, text: impl Into<String>) -> RichText {
    let family = family(ui.ctx(), SEMIBOLD).unwrap_or(FontFamily::Proportional);
    RichText::new(text).family(family)
}

/// A 16 px icon of [`icons`] in the widget's text color. It is painted, not text, so the widget's
/// accessible name stays its label.
pub fn icon(glyph: &'static str) -> Atom<'static> {
    Atom::paint(Vec2::splat(16.0), move |ui, args| {
        paint_icon(ui, args.rect, glyph, args.fallback_text_color);
    })
}

/// An icon of [`icons`], `size` px and in `color`, as a widget of its own.
pub fn mark(ui: &mut Ui, glyph: &'static str, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui, rect, glyph, color);
    response
}

fn paint_icon(ui: &Ui, rect: Rect, glyph: &'static str, color: Color32) {
    if let Some(family) = family(ui.ctx(), ICONS) {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::new(rect.height(), family),
            color,
        );
    }
}

/// A line of `text` in `color` after the icon `glyph`, for warnings, errors and results.
pub fn status(ui: &mut Ui, glyph: &'static str, color: Color32, text: impl Into<String>) {
    ui.horizontal(|ui| {
        mark(ui, glyph, 16.0, color);
        ui.add(Label::new(RichText::new(text).color(color)).wrap());
    });
}

/// Drops the outline of the buttons in `ui` at rest and under the pointer, for lists of
/// selectable pills such as tabs and menus. A focused pill keeps its outline.
pub fn pill_list(ui: &mut Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.hovered.bg_stroke = Stroke::NONE;
}

/// Lays `add` out in the content column: at most 920 px wide and centered, like libadwaita's
/// clamp, with at least 24 px on each side.
pub fn column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_rect_before_wrap();
    let width = (available.width() - 48.0).clamp(0.0, 920.0);
    let rect = available.shrink2(Vec2::new((available.width() - width) / 2.0, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(rect), add).inner
}

/// A vertically scrolling page holding the content [`column`], 16 px below the top bar. The
/// scroll bar stays at the window edge.
pub fn page<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            column(ui, |ui| {
                ui.add_space(16.0);
                let inner = add(ui);
                ui.add_space(16.0);
                inner
            })
        })
        .inner
}

/// A boxed list: one rounded container on the surface color. Separate its rows with
/// [`row_separator`]; only the container is rounded.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let visuals = ui.visuals();
    Frame::new()
        .fill(visuals.window_fill)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(10)
        .inner_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// The hairline between two rows of a [`group`], from edge to edge.
pub fn row_separator(ui: &mut Ui) {
    ui.add(Separator::default().grow(12.0));
}

/// A button filled with the accent color, for the one main action of a screen, with an optional
/// icon of [`icons`] before its semibold `label`.
pub fn primary(ui: &mut Ui, enabled: bool, glyph: Option<&'static str>, label: &str) -> Response {
    let palette = palette(ui);
    ui.scope(|ui| {
        // Scoped visuals rather than `Button::fill`, which would drop the hover and press fills.
        let widgets = &mut ui.visuals_mut().widgets;
        for (state, fill) in [
            (&mut widgets.inactive, palette.accent_fill),
            (&mut widgets.hovered, palette.accent_fill_hover),
            (&mut widgets.active, palette.accent_fill_pressed),
        ] {
            state.weak_bg_fill = fill;
            state.bg_fill = fill;
            state.fg_stroke.color = Color32::WHITE;
        }
        widgets.inactive.bg_stroke.color = palette.accent_fill;
        let label = semibold(ui, label);
        let button = match glyph {
            Some(glyph) => Button::new((icon(glyph), label)),
            None => Button::new(label),
        };
        ui.add_enabled(enabled, button)
    })
    .inner
}

/// `base` with the colors of `palette`.
fn visuals(palette: &Palette, base: Visuals) -> Visuals {
    let mut visuals = base;
    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = Stroke::new(1.0, palette.divider);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.extreme_bg_color = palette.field;
    visuals.faint_bg_color = palette.surface_hover;
    visuals.hyperlink_color = palette.accent_text;
    visuals.warn_fg_color = palette.warn;
    visuals.error_fg_color = palette.error;
    visuals.weak_text_color = Some(palette.muted);
    visuals.selection.bg_fill = palette.selection_bg;
    visuals.selection.stroke = Stroke::new(1.0, palette.selection_stroke);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.weak_bg_fill = palette.bg;
    widgets.noninteractive.bg_fill = palette.bg;
    widgets.noninteractive.bg_stroke.color = palette.divider;
    widgets.noninteractive.fg_stroke.color = palette.text;
    for (state, stroke) in [
        (&mut widgets.inactive, palette.input_stroke),
        (&mut widgets.hovered, palette.selection_stroke),
        (&mut widgets.active, palette.selection_stroke),
        (&mut widgets.open, palette.input_stroke),
    ] {
        state.weak_bg_fill = palette.surface_hover;
        state.bg_fill = palette.surface_hover;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke.color = palette.text;
        state.corner_radius = CornerRadius::same(6);
    }
    visuals
}

/// The type scale and the spacing, the same in both themes.
fn style(style: &mut Style) {
    let proportional = |size| FontId::new(size, FontFamily::Proportional);
    style.text_styles = [
        (TextStyle::Small, proportional(12.0)),
        (TextStyle::Body, proportional(15.0)),
        // Also the text of checkboxes, combo boxes and collapsing headers, so it stays regular.
        (TextStyle::Button, proportional(15.0)),
        (
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Name(SEMIBOLD.into())),
        ),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(8.0, 8.0);
    spacing.button_padding = Vec2::new(12.0, 6.0);
    spacing.interact_size = Vec2::new(40.0, 24.0);
    spacing.indent = 24.0;
    spacing.window_margin = Margin::same(20);
    spacing.menu_margin = Margin::same(6);
    spacing.icon_width = 18.0;
    spacing.icon_width_inner = 10.0;
    spacing.icon_spacing = 8.0;
    spacing.text_edit_width = 320.0;
}

#[cfg(test)]
mod tests;
