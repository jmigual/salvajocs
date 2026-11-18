//! The title bar the app draws in place of the OS one: window buttons, moving and resizing.

mod common;

use common::{
    env, init_repo, local_config, repo_dir, run_until, run_until_ready, save_config, shows,
};
use eframe::egui::{
    Event, Id, LayerId, Modifiers, Order, PointerButton, Pos2, Ui, UiBuilder, Vec2,
    ViewportCommand, ViewportId, viewport::ResizeDirection,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};

/// Drives the app on the whole window, as eframe does, rather than in the margin of kittest's
/// panel: the window edges are then where the app's are.
fn harness(env: Env, size: Vec2) -> Harness<'static, App> {
    Harness::builder().with_size(size).build_ui_state(
        |ui, app| {
            let ctx = ui.ctx().clone();
            let builder = UiBuilder::new()
                .layer_id(LayerId::background())
                .max_rect(ctx.viewport_rect());
            let mut root = Ui::new(ctx, Id::unique("window"), builder);
            eframe::App::ui(app, &mut root, &mut eframe::Frame::_new_kittest());
        },
        App::new(env),
    )
}

/// An app with its repository open, in a window of `size`.
fn ready_sized(size: Vec2) -> (tempfile::TempDir, Harness<'static, App>) {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env, size);
    run_until_ready(&mut harness);
    (tmp, harness)
}

/// An app with its repository open.
fn ready() -> (tempfile::TempDir, Harness<'static, App>) {
    ready_sized(Vec2::new(1000.0, 1400.0))
}

/// An app showing the first-run dialog.
fn first_run() -> (tempfile::TempDir, Harness<'static, App>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp), Vec2::new(1000.0, 1400.0));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });
    (tmp, harness)
}

/// The viewport commands of the last frame.
fn commands(harness: &Harness<'static, App>) -> Vec<ViewportCommand> {
    harness.output().viewport_output[&ViewportId::ROOT]
        .commands
        .clone()
}

/// A press or release of `button` at `pos`.
fn button_event(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A press or release of the primary button at `pos`.
fn button(pos: Pos2, pressed: bool) -> Event {
    button_event(pos, PointerButton::Primary, pressed)
}

/// Presses `button` at `from` and moves 20 px to the right, past the drag threshold, and returns
/// the viewport commands of all those frames.
fn drag(
    harness: &mut Harness<'static, App>,
    from: Pos2,
    button: PointerButton,
) -> Vec<ViewportCommand> {
    let mut sent = Vec::new();
    for event in [
        Event::PointerMoved(from),
        button_event(from, button, true),
        Event::PointerMoved(from + Vec2::new(20.0, 0.0)),
    ] {
        harness.event(event);
        harness.step();
        sent.extend(commands(harness));
    }
    sent
}

/// Empty bar space, between the last tab and the manifest status.
fn empty_bar(harness: &Harness<'static, App>) -> Pos2 {
    let last_tab = harness
        .get_by_label(Tab::ALL[Tab::ALL.len() - 1].label())
        .rect();
    Pos2::new(last_tab.right() + 40.0, last_tab.center().y)
}

/// Reports the window as maximized from the next frame on, as the backend would.
fn maximize(harness: &mut Harness<'static, App>) {
    harness
        .input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .maximized = Some(true);
    harness.step();
}

#[test]
fn window_buttons_send_their_commands() {
    let (_tmp, mut harness) = ready();
    for (label, command) in [
        ("Minimize", ViewportCommand::Minimized(true)),
        ("Maximize", ViewportCommand::Maximized(true)),
        ("Close", ViewportCommand::Close),
    ] {
        harness.get_by_label(label).click();
        harness.step();
        assert_eq!(commands(&harness), [command], "{label}");
    }
}

#[test]
fn the_window_buttons_hold_only_their_own_room() {
    let (_tmp, harness) = ready();
    let close = harness.get_by_label("Close").rect();
    let tab = harness.get_by_label(Tab::ALL[0].label()).rect();
    assert!(
        (close.center().y - tab.center().y).abs() < 0.5,
        "the buttons sit on the tabs' line: {close:?}, {tab:?}"
    );
    for pos in [
        Pos2::new(close.center().x, close.bottom() + 20.0),
        Pos2::new(close.left() - 200.0, close.center().y),
    ] {
        let layer = harness.ctx.layer_id_at(pos).unwrap();
        assert_ne!(layer.order, Order::Tooltip, "at {pos:?}");
    }
}

#[test]
fn a_maximized_window_offers_restore_with_close_in_the_corner() {
    let (_tmp, mut harness) = ready();
    maximize(&mut harness);
    harness.step();
    let close = harness.get_by_label("Close").rect();
    let window = harness.ctx.viewport_rect();
    assert_eq!(
        (close.right(), close.top()),
        (window.right(), window.top()),
        "the pointer thrown into the corner hits Close"
    );
    harness.get_by_label("Restore window").click();
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::Maximized(false)]);
}

#[test]
fn close_works_while_the_first_run_dialog_is_open() {
    let (_tmp, mut harness) = first_run();
    harness.get_by_label("Close").click();
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::Close]);
}

#[test]
fn the_bar_moves_the_window_while_the_first_run_dialog_is_open() {
    let (_tmp, mut harness) = first_run();
    let minimize = harness.get_by_label("Minimize").rect();
    let bar = Pos2::new(minimize.left() - 40.0, minimize.center().y);
    assert_eq!(
        drag(&mut harness, bar, PointerButton::Primary),
        [ViewportCommand::StartDrag]
    );
}

#[test]
fn the_top_edge_only_resizes_while_the_first_run_dialog_is_open() {
    let (_tmp, mut harness) = first_run();
    let minimize = harness.get_by_label("Minimize").rect();
    let edge = Pos2::new(minimize.left() - 40.0, 1.0);
    assert_eq!(
        drag(&mut harness, edge, PointerButton::Primary),
        [ViewportCommand::BeginResize(ResizeDirection::North)]
    );
}

#[test]
fn double_clicking_the_bar_maximizes() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.step();
    // In one frame: a frame is `step_dt`, a quarter second, longer than a double click allows.
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(button(empty, pressed));
    }
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::Maximized(true)]);
}

#[test]
fn dragging_the_bar_moves_the_window() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    assert_eq!(
        drag(&mut harness, empty, PointerButton::Primary),
        [ViewportCommand::StartDrag]
    );
}

#[test]
fn the_first_motion_after_a_press_on_the_bar_moves_the_window() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.event(button(empty, true));
    harness.step();
    assert_eq!(commands(&harness), [], "a press alone may be a click");
    harness.event(Event::PointerMoved(empty + Vec2::new(1.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::StartDrag]);
    // The compositor keeps the release, so egui still holds the button down.
    harness.event(Event::PointerMoved(empty + Vec2::new(20.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), [], "one move per press");
}

#[test]
fn the_bar_still_clicks_after_the_compositor_kept_the_release() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.event(button(empty, true));
    harness.step();
    harness.event(Event::PointerMoved(empty + Vec2::new(2.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::StartDrag]);
    harness.event(Event::PointerGone);
    harness.step();
    harness.hover_at(empty);
    harness.step();
    assert_eq!(commands(&harness), [], "the pointer back over the bar");
    for pressed in [true, false] {
        harness.event(button(empty, pressed));
        harness.step();
        assert_eq!(commands(&harness), [], "a click does not move the window");
    }
    // Past the double-click time, so the next two clicks are a double click and not a triple.
    harness.step();
    harness.step();
    for pressed in [true, false, true, false] {
        harness.input_mut().events.push(button(empty, pressed));
    }
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::Maximized(true)]);
}

#[test]
fn a_press_whose_release_went_with_the_focus_does_not_move_the_window() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.event(button(empty, true));
    harness.step();
    for focused in [false, true] {
        harness.event(Event::WindowFocused(focused));
        harness.step();
    }
    harness.event(Event::PointerMoved(empty + Vec2::new(1.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), []);
}

#[test]
fn dragging_the_manifest_status_moves_the_window() {
    let (_tmp, mut harness) = ready();
    let status = harness
        .query_all_by_label_contains("Manifest:")
        .next()
        .unwrap()
        .rect()
        .center();
    assert_eq!(
        drag(&mut harness, status, PointerButton::Primary),
        [ViewportCommand::StartDrag]
    );
}

#[test]
fn only_the_primary_button_moves_the_window() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    assert_eq!(drag(&mut harness, empty, PointerButton::Secondary), []);
}

#[test]
fn a_secondary_drag_after_a_click_on_the_bar_leaves_the_window_be() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    for pressed in [true, false] {
        harness.event(button(empty, pressed));
        harness.step();
    }
    harness.step();
    harness.step();
    assert_eq!(drag(&mut harness, empty, PointerButton::Secondary), []);
}

#[test]
fn dragging_from_a_tab_leaves_the_window_and_the_tab_be() {
    let (_tmp, mut harness) = ready();
    let tab = harness.get_by_label(Tab::Restore.label()).rect().center();
    assert_eq!(
        drag(&mut harness, tab, PointerButton::Primary),
        [],
        "the window stays put"
    );
    harness.event(button(tab + Vec2::new(20.0, 0.0), false));
    harness.step();

    harness.get_by_label(Tab::Restore.label()).click();
    common::run_settled(&mut harness);
    assert!(shows(&harness, "Load backups"), "the tab still opens");
}

#[test]
fn nudging_a_tab_leaves_the_window_be() {
    let (_tmp, mut harness) = ready();
    let tab = harness.get_by_label(Tab::Restore.label()).rect().center();
    harness.hover_at(tab);
    harness.event(button(tab, true));
    harness.step();
    harness.event(Event::PointerMoved(tab + Vec2::new(1.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), []);
}

#[test]
fn pressing_a_window_edge_starts_a_resize() {
    let (_tmp, mut harness) = ready();
    let corner = Pos2::new(1.0, 1.0);
    harness.hover_at(corner);
    harness.event(button(corner, true));
    harness.step();
    assert_eq!(
        commands(&harness),
        [ViewportCommand::BeginResize(ResizeDirection::NorthWest)]
    );

    harness.event(button(corner, false));
    maximize(&mut harness);
    harness.event(button(corner, true));
    harness.step();
    assert_eq!(commands(&harness), [], "a maximized window does not resize");
}

#[test]
fn pressing_the_right_edge_leaves_the_page_scroll_alone() {
    let (_tmp, mut harness) = ready_sized(Vec2::new(1000.0, 500.0));
    harness.state_mut().set_tab(Tab::Settings);
    common::run_settled(&mut harness);
    let before = harness.get_by_label("Roots").rect();
    let window = harness.ctx.viewport_rect();
    let edge = Pos2::new(window.right() - 1.0, window.bottom() - 40.0);
    harness.hover_at(edge);
    harness.event(button(edge, true));
    harness.event(button(edge, false));
    common::run_settled(&mut harness);
    assert_eq!(harness.get_by_label("Roots").rect(), before);
}
