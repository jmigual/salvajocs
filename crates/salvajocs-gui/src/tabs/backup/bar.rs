use std::f32::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::emath::Rot2;
use eframe::egui::{Label, Rect, RichText, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo};
use salvajocs_core::config::PasswordMode;

use super::unsent::has_unsent;
use super::{BackupTab, lock};
use crate::state::State;
use crate::status_bar;
use crate::tabs::count;
use crate::theme::{self, icons};

impl BackupTab {
    /// Whether the tab adds the repository and the backup button to the bottom bar. It waits for
    /// a scan report, so the empty state has one call to action.
    pub(crate) fn has_bar(&self, state: &State) -> bool {
        state.config.is_some() && (self.report.is_some() || has_unsent(state.config.as_ref()))
    }

    /// Draws the bottom bar in `bar`, the room [`status_bar::reserve`] took, with the app's
    /// `start` and `end`: then where backups go, or while a backup runs its progress, on the left,
    /// and the backup button last on the right. Call it after the page, when [`Self::has_bar`].
    pub(crate) fn bar_ui(
        &mut self,
        ui: &mut Ui,
        bar: Rect,
        state: &State,
        manifest_loading: bool,
        start: impl FnOnce(&mut Ui),
        mut end: impl FnMut(&mut Ui),
    ) {
        let selected = self.chosen();
        // With no game chosen, the backup only saves the unsent changes (ADR-0029).
        let unsent = has_unsent(state.config.as_ref());
        // A pending file write does not hold the button back: the click waits for it.
        let ready = state.session.is_some()
            && self.choices.loaded()
            && (selected > 0 || unsent)
            && !manifest_loading
            && self.scanning.is_none()
            && self.running.is_none()
            && !self.backup_requested
            && self.checking.is_none()
            && self.asking.is_none()
            && self.cleared.is_none();
        let label = if selected == 0 && unsent {
            "Back up".to_owned()
        } else {
            format!("Back up {}", count(selected, "game"))
        };
        let button = |ui: &mut Ui| theme::primary(ui, ready, Some(icons::FLOPPY_DISK), &label);
        let back_up = status_bar::show(
            ui,
            bar,
            |ui| {
                let status =
                    |ui: &mut Ui| status_bar::status_text(ui, |ui| self.bar_status(ui, state));
                // A running backup's progress goes first, so the manifest status gives way to it.
                if self.running.is_some() {
                    status(ui);
                    start(ui);
                } else {
                    start(ui);
                    status(ui);
                }
            },
            |ui| {
                // `end` gets what the button leaves, so its text gives way and the button stays.
                let width = theme::natural_width(ui, |ui| {
                    button(ui);
                });
                let room = ui.available_width() - width - ui.spacing().item_spacing.x;
                ui.scope(|ui| {
                    ui.set_max_width(room.max(0.0));
                    end(ui);
                });
                button(ui)
            },
        );
        if back_up.clicked() {
            self.backup_requested = true;
            self.start_requested(ui.ctx(), state);
        }
        let progress = lock(&self.progress);
        let (checked, games) = *lock(&self.checked);
        // While the backup's scan runs, the rule shows the scan.
        let (done, total) = if checked < games {
            (checked, games)
        } else {
            (progress.done.len(), progress.total)
        };
        if self.running.is_some() && total > 0 {
            let done = done as f32 / total as f32;
            let rule = Rect::from_min_size(bar.left_top(), Vec2::new(bar.width() * done, 3.0));
            ui.painter()
                .rect_filled(rule, 0.0, theme::palette(ui).accent);
        }
    }

    /// The tab's part of the bottom bar's left: the repository and whether it is encrypted, or the
    /// running backup's progress.
    fn bar_status(&self, ui: &mut Ui, state: &State) {
        let muted = ui.visuals().weak_text_color();
        if self.checking.is_some() {
            ui.spinner();
            ui.label("Checking the repository");
        } else if self.running.is_some() {
            autosave_mark(ui);
            let progress = lock(&self.progress);
            let (checked, games) = *lock(&self.checked);
            if progress.sending {
                ui.label("Saving the custom games and rules to the repository");
            } else if progress.cleaning {
                ui.label("Removing old backups");
            } else if checked < games {
                ui.label(format!("Checked {checked} of {}", count(games, "game")));
            } else {
                ui.label(format!(
                    "Backed up {} of {}",
                    progress.done.len(),
                    count(progress.total, "game")
                ));
            }
        } else if let (Some(config), Some(_)) = (&state.config, &state.session) {
            let (glyph, encryption) = match config.password {
                PasswordMode::None => (icons::LOCK_SIMPLE_OPEN, "Not encrypted"),
                PasswordMode::Keyring | PasswordMode::Ask => (icons::LOCK_SIMPLE, "Encrypted"),
                _ => (icons::LOCK_SIMPLE_OPEN, "Encryption unknown"),
            };
            theme::mark(ui, glyph, 16.0, muted)
                .on_hover_text(encryption)
                .widget_info(|| WidgetInfo::labeled(Role::Image, true, encryption));
            ui.add(Label::new(RichText::new(&config.repository).color(muted)).truncate());
        } else {
            ui.weak("The repository is not open.");
        }
    }
}

/// The app icon's save mark, the arc with its arrowhead, turning once every 1.2 s in the accent
/// green while a backup runs: the "saving, do not turn off" sign of games.
fn autosave_mark(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    // The icon's mark spans about 92 of its 128 units; its arc is centered on (64, 63).
    let scale = rect.width() / 92.0;
    let turn = ui.input(|input| input.time) as f32 * TAU / 1.2;
    let rotation = Rot2::from_angle(turn);
    let at = |x: f32, y: f32| rect.center() + rotation * (Vec2::new(x, y) * scale);
    // 270° from the top, clockwise, ending on the left where the arrowhead sits.
    let arc = (0..=24)
        .map(|step| {
            let angle = -FRAC_PI_2 + 1.5 * PI * step as f32 / 24.0;
            at(39.0 * angle.cos(), 39.0 * angle.sin())
        })
        .collect();
    let accent = theme::palette(ui).accent;
    let painter = ui.painter();
    painter.add(Shape::line(arc, Stroke::new(11.0 * scale, accent)));
    painter.add(Shape::convex_polygon(
        vec![at(-53.0, 2.0), at(-25.0, 2.0), at(-39.0, -18.0)],
        accent,
        Stroke::NONE,
    ));
    ui.ctx().request_repaint();
}
