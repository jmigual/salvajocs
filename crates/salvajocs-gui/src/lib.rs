//! egui front-end for salvajocs.

pub mod env;
pub mod first_run;
pub mod job;
pub mod prompt;
pub mod startup;
pub mod state;
pub mod tabs;
pub mod widgets;

/// Opens the main window and blocks until it is closed.
pub fn run() -> eframe::Result {
    eframe::run_native(
        "salvajocs",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
}

#[derive(Default)]
struct App;

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {}
}
