use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::*;

/// The WCAG 2 relative luminance of `color`.
fn luminance(color: Color32) -> f64 {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

/// The WCAG 2 contrast ratio of two colors.
fn contrast(a: Color32, b: Color32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Text needs 4.5:1 (WCAG 1.4.3), control outlines and marks 3:1 (WCAG 1.4.11).
#[test]
fn both_palettes_meet_the_contrast_floors() {
    for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
        let checks = [
            ("text on bg", p.text, p.bg, 4.5),
            ("text on surface", p.text, p.surface, 4.5),
            ("text on surface_hover", p.text, p.surface_hover, 4.5),
            ("text on field", p.text, p.field, 4.5),
            ("muted on bg", p.muted, p.bg, 4.5),
            ("muted on surface", p.muted, p.surface, 4.5),
            ("muted on surface_hover", p.muted, p.surface_hover, 4.5),
            ("white on accent_fill", Color32::WHITE, p.accent_fill, 4.5),
            (
                "white on accent_fill_hover",
                Color32::WHITE,
                p.accent_fill_hover,
                4.5,
            ),
            ("accent_text on bg", p.accent_text, p.bg, 4.5),
            ("input_stroke on bg", p.input_stroke, p.bg, 3.0),
            ("input_stroke on surface", p.input_stroke, p.surface, 3.0),
            ("accent on bg", p.accent, p.bg, 3.0),
            ("accent on surface", p.accent, p.surface, 3.0),
            (
                "selection_stroke on selection_bg",
                p.selection_stroke,
                p.selection_bg,
                4.5,
            ),
            ("text on selection_bg", p.text, p.selection_bg, 4.5),
            ("selection_stroke on bg", p.selection_stroke, p.bg, 3.0),
            (
                "selection_stroke on surface",
                p.selection_stroke,
                p.surface,
                3.0,
            ),
            ("warn on bg", p.warn, p.bg, 4.5),
            ("warn on surface", p.warn, p.surface, 4.5),
            ("error on bg", p.error, p.bg, 4.5),
            ("error on surface", p.error, p.surface, 4.5),
        ];
        for (what, fg, bg, floor) in checks {
            let ratio = contrast(fg, bg);
            assert!(
                ratio >= floor,
                "{name}: {what} is {ratio:.2}:1, below {floor}:1"
            );
        }
    }
}

#[test]
fn the_fonts_define_the_semibold_family_after_the_defaults() {
    let fonts = fonts();
    let semibold = &fonts.families[&FontFamily::Name(SEMIBOLD.into())];
    assert_eq!(semibold[0], "atkinson-semibold");
    // The defaults stay as fallbacks, for glyphs Atkinson lacks.
    assert!(semibold.len() > 1);
    assert_eq!(fonts.families[&FontFamily::Proportional][0], "atkinson");
}

#[test]
fn the_icons_have_their_own_family_only() {
    let fonts = fonts();
    assert_eq!(fonts.families[&FontFamily::Name(ICONS.into())], [ICONS]);
    assert!(!fonts.families[&FontFamily::Proportional].contains(&ICONS.to_owned()));
}

/// The icon is painted, so tests and screen readers find the button by its label alone.
#[test]
fn a_primary_button_with_an_icon_is_named_by_its_label() {
    let mut harness = Harness::new_ui_state(
        |ui, clicks: &mut usize| {
            apply(ui.ctx());
            if primary(ui, true, Some(icons::MAGNIFYING_GLASS), "Scan for games").clicked() {
                *clicks += 1;
            }
        },
        0,
    );
    harness.run();
    assert_eq!(harness.query_all_by_label("Scan for games").count(), 1);
    harness.get_by_label("Scan for games").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}
