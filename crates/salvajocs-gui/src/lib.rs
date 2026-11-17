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

/// The application ID; Wayland matches it against the `.desktop` file name to find the icon.
const APP_ID: &str = "io.github.jmigual.salvajocs";

/// Opens the main window and blocks until it is closed.
pub fn run() -> eframe::Result {
    let env = Env::system().ok_or_else(|| {
        eframe::Error::AppCreation("this OS has no config or cache directory".into())
    })?;
    eframe::run_native(
        "salvajocs",
        native_options()?,
        Box::new(move |_cc| Ok(Box::new(App::new(env)))),
    )
}

/// The window options: the default ones plus the app id and the embedded 256 px icon.
fn native_options() -> eframe::Result<eframe::NativeOptions> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../assets/io.github.jmigual.salvajocs.png"
    ))
    .map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
    let mut options = eframe::NativeOptions::default();
    options.viewport = options.viewport.with_app_id(APP_ID).with_icon(icon);
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_options_set_the_app_id_and_icon() {
        let options = native_options().expect("the embedded icon decodes");
        assert_eq!(options.viewport.app_id.as_deref(), Some(APP_ID));
        let icon = options.viewport.icon.expect("an icon is set");
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
