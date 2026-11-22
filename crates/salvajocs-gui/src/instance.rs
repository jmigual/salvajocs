//! The window shown instead of the app when another instance runs; the lock that tells is
//! `salvajocs_core::instance`.

use eframe::egui::{self, Ui};

/// The notice that another salvajocs runs, with a button to close it.
pub struct AlreadyRunning;

impl eframe::App for AlreadyRunning {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.heading("Salvajocs is already running");
            ui.label("Two copies at once could damage a backup. Use the window that is open.");
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                ui.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

#[cfg(test)]
mod tests;
