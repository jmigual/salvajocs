use egui_kittest::kittest::Queryable;

use super::*;
use crate::theme;

#[test]
fn the_already_running_window_says_so_and_closes_on_the_button() {
    let mut harness = theme::harness(
        |ui, app: &mut AlreadyRunning| {
            eframe::App::ui(app, ui, &mut eframe::Frame::_new_kittest());
        },
        AlreadyRunning,
    );
    harness.run();
    assert!(
        harness
            .query_by_label("Salvajocs is already running")
            .is_some()
    );
    let closed = |harness: &egui_kittest::Harness<'_, AlreadyRunning>| {
        (harness.output().viewport_output.values())
            .any(|viewport| viewport.commands.contains(&egui::ViewportCommand::Close))
    };
    assert!(!closed(&harness));
    harness.get_by_label("Close").click();
    // One frame: the output is that of the last frame, the one that handled the click.
    harness.step();
    assert!(closed(&harness));
}
