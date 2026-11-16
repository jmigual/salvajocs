//! egui front-end for salvajocs.

pub mod app;
pub mod env;
pub mod first_run;
pub mod job;
pub mod prompt;
pub mod startup;
pub mod state;
pub mod tabs;
pub mod widgets;

pub use app::App;
pub use env::Env;

/// Opens the main window and blocks until it is closed.
pub fn run() -> eframe::Result {
    let env = Env::system().ok_or_else(|| {
        eframe::Error::AppCreation("this OS has no config or cache directory".into())
    })?;
    eframe::run_native(
        "salvajocs",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| Ok(Box::new(App::new(env)))),
    )
}
