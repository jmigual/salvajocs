// No console window in release builds on Windows; debug builds keep it for the logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let result = salvajocs_gui::run();
    // Without a console, stderr goes nowhere, so a startup failure would be silent.
    if let Err(err) = &result
        && cfg!(all(windows, not(debug_assertions)))
    {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Salvajocs")
            .set_description(format!("Salvajocs could not start: {err}"))
            .show();
    }
    result
}
