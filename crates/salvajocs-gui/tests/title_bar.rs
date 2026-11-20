//! The bars the app draws: the title bar in place of the OS one, with the window buttons, moving
//! and resizing, and the bottom bar with the manifest status and the update notice, and on the
//! Backup tab the repository and the backup button.

mod common;

use std::sync::Arc;

use common::{
    MemorySecrets, env, env_with, init_repo, init_repo_with, local_config, repo_dir, run_until,
    run_until_ready, save_config, shows,
};
use eframe::egui::{
    Event, Id, LayerId, Modifiers, Order, PointerButton, Pos2, Rect, Shape, Ui, UiBuilder, Vec2,
    ViewportCommand, ViewportId, viewport::ResizeDirection,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::PasswordMode;
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::Compression;
use salvajocs_core::update::Release;
use salvajocs_gui::env::SecretStore;
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env, MIN_WINDOW_SIZE};

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

/// Empty bar space, right of the last tab.
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
fn a_maximized_window_keeps_the_icons_on_the_tabs_line() {
    let (_tmp, mut harness) = ready();
    maximize(&mut harness);
    harness.step();
    let close = harness.get_by_label("Close").rect();
    let tab = harness.get_by_label(Tab::ALL[0].label()).rect();
    let icon = harness
        .output()
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if close.contains_rect(text.visual_bounding_rect()) => {
                Some(text.visual_bounding_rect())
            }
            _ => None,
        })
        .expect("the Close icon");
    assert!(
        (icon.center().y - tab.center().y).abs() < 0.5,
        "the icon sits on the tabs' line: {icon:?}, {tab:?}"
    );
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
fn a_click_on_the_bar_over_a_dialog_does_not_arm_it_for_a_later_press() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "right", Compression::Default);
    let mut config = local_config(&repository);
    config.password = PasswordMode::Keyring;
    secrets
        .set(&config.secret_scope(), &SecretKey::Password, "bad")
        .unwrap();
    save_config(&env, &config);
    let mut harness = harness(env, Vec2::new(1000.0, 1400.0));
    run_until(&mut harness, "the password prompt", |h| {
        shows(h, "Wrong encryption password")
    });
    harness.step();
    let minimize = harness.get_by_label("Minimize").rect();
    let bar = Pos2::new(minimize.left() - 40.0, minimize.center().y);
    harness.hover_at(bar);
    for pressed in [true, false] {
        harness.event(button(bar, pressed));
        harness.step();
    }

    let field = harness.get_by_label("Encryption password");
    field.focus();
    field.type_text("worse");
    harness.step();
    // Not `click`: its pointer press would rewrite the bar's state while the dialog is open.
    harness.get_by_label("Open repository").click_accesskit();
    run_until(&mut harness, "the repository to be opening", |h| {
        shows(h, "Opening the repository")
    });
    let elsewhere = harness.ctx.viewport_rect().center();
    harness.hover_at(elsewhere);
    harness.event(button(elsewhere, true));
    harness.step();
    assert!(
        shows(&harness, "Opening the repository"),
        "pressed while no dialog is open"
    );
    run_until(&mut harness, "the password prompt again", |h| {
        shows(h, "Wrong encryption password")
    });
    let mut sent = Vec::new();
    for _ in 0..2 {
        harness.step();
        sent.extend(commands(&harness));
    }
    harness.event(Event::PointerMoved(elsewhere + Vec2::new(3.0, 0.0)));
    harness.step();
    sent.extend(commands(&harness));
    assert_eq!(sent, []);
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
fn a_double_click_that_jitters_still_maximizes() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.step();
    // The second release a frame later, still within the double-click time.
    for event in [
        button(empty, true),
        button(empty, false),
        button(empty, true),
        Event::PointerMoved(empty + Vec2::new(1.0, 0.0)),
    ] {
        harness.input_mut().events.push(event);
    }
    harness.step();
    assert_eq!(
        commands(&harness),
        [],
        "the jitter does not move the window"
    );
    harness.event(button(empty + Vec2::new(1.0, 0.0), false));
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
fn three_points_from_a_press_on_the_bar_move_the_window() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.hover_at(empty);
    harness.event(button(empty, true));
    harness.step();
    assert_eq!(commands(&harness), [], "a press alone may be a click");
    harness.event(Event::PointerMoved(empty + Vec2::new(1.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), [], "a jitter may still be a click");
    harness.event(Event::PointerMoved(empty + Vec2::new(3.0, 0.0)));
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
    harness.event(Event::PointerMoved(empty + Vec2::new(3.0, 0.0)));
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
    harness.event(Event::PointerMoved(empty + Vec2::new(3.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), []);
}

#[test]
fn a_press_that_focuses_the_window_moves_it_once_focused() {
    let (_tmp, mut harness) = ready();
    let empty = empty_bar(&harness);
    harness.input_mut().focused = false;
    harness.hover_at(empty);
    harness.event(button(empty, true));
    harness.step();
    harness.event(Event::PointerMoved(empty + Vec2::new(3.0, 0.0)));
    harness.step();
    assert_eq!(
        commands(&harness),
        [],
        "the backend drops a move before the focus"
    );
    harness.input_mut().focused = true;
    harness.event(Event::PointerMoved(empty + Vec2::new(4.0, 0.0)));
    harness.step();
    assert_eq!(commands(&harness), [ViewportCommand::StartDrag]);
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
    harness.event(Event::PointerMoved(tab + Vec2::new(3.0, 0.0)));
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

/// An app with its repository open and an update to install, in a window of `size`. The app was
/// `installed` by the installer, or it is a portable one that links the release page.
fn ready_with_update(size: Vec2, installed: bool) -> (tempfile::TempDir, Harness<'static, App>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    env.check_update = Some(|_| {
        Ok(Some(Release::new(
            "99.0.0".to_string(),
            "https://example.invalid/releases/tag/v99.0.0".to_string(),
            Vec::new(),
        )))
    });
    env.installed_by_installer = if installed { || true } else { || false };
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env, size);
    run_until_ready(&mut harness);
    run_until(&mut harness, "the update notice", |h| {
        shows(h, "Update available")
    });
    (tmp, harness)
}

/// The rects of the items labeled by one of `labels`.
fn items(
    harness: &Harness<'static, App>,
    labels: impl IntoIterator<Item = &'static str>,
) -> Vec<(&'static str, Rect)> {
    labels
        .into_iter()
        .flat_map(|label| {
            harness
                .query_all_by_label_contains(label)
                .map(|node| (label, node.rect()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The labels of the top bar's items.
fn top_bar_labels() -> impl Iterator<Item = &'static str> {
    (Tab::ALL.iter().map(|tab| tab.label())).chain(["Minimize", "Maximize", "Close"])
}

/// The labels of the bottom bar's items.
const BOTTOM_BAR: [&str; 5] = [
    "Update available",
    "Update now",
    "Release page",
    "Manifest",
    "Back up",
];

/// Checks that `items`, on one row, do not overlap.
fn assert_apart(mut items: Vec<(&'static str, Rect)>) {
    items.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    // The link that took over the dropped notice is found by both labels.
    items.dedup_by(|a, b| a.1 == b.1);
    for pair in items.windows(2) {
        assert!(
            pair[0].1.right() <= pair[1].1.left(),
            "{} and {} overlap",
            pair[0].0,
            pair[1].0
        );
    }
}

/// Checks that the top bar's items of `harness` do not overlap, that its tabs are as wide as in
/// `wide`, a window wide enough for all of the bar, and that the window buttons keep the bar's
/// 16 pt margin.
fn assert_top_bar_fits(harness: &Harness<'static, App>, wide: &Harness<'static, App>) {
    let close = harness.get_by_label("Close").rect();
    assert!(
        close.right() <= harness.ctx.viewport_rect().right() - 16.0,
        "the window buttons are in the bar's margin"
    );
    let items = items(harness, top_bar_labels());
    for ((label, rect), (_, natural)) in items.iter().zip(self::items(wide, top_bar_labels())) {
        if Tab::ALL.iter().any(|tab| tab.label() == *label) {
            assert_eq!(rect.width(), natural.width(), "the {label} tab is squeezed");
        }
    }
    assert_apart(items);
}

/// Checks that the bottom bar's items of `harness` do not overlap and keep the bar's 16 pt
/// margins.
fn assert_bottom_bar_fits(harness: &Harness<'static, App>) {
    let window = harness.ctx.viewport_rect();
    let items = items(harness, BOTTOM_BAR);
    for (label, rect) in &items {
        assert!(
            rect.left() >= window.left() + 16.0 && rect.right() <= window.right() - 16.0,
            "{label} is in the bar's margin: {rect:?}"
        );
    }
    assert_apart(items);
}

#[test]
fn bar_items_fit_at_the_smallest_window() {
    let [width, height] = MIN_WINDOW_SIZE;
    let (_tmp, harness) = ready_with_update(Vec2::new(width, height), true);
    let (_wide_tmp, wide) = ready_with_update(Vec2::new(1100.0, height), true);
    assert_top_bar_fits(&harness, &wide);
    assert_bottom_bar_fits(&harness);
}

#[test]
fn manifest_status_and_update_notice_are_in_the_bottom_bar() {
    let (_tmp, harness) = ready_with_update(Vec2::new(1000.0, 1400.0), false);
    let window = harness.ctx.viewport_rect();
    let close = harness.get_by_label("Close").rect();
    let items = items(&harness, BOTTOM_BAR);
    assert_eq!(items.len(), 3, "the manifest, the notice and the link");
    for (label, rect) in &items {
        // Both bars center their row, so the thinner bar has its row nearer its window edge.
        assert!(
            rect.bottom() <= window.bottom()
                && window.bottom() - rect.center().y < close.center().y - window.top(),
            "{label} is in a bar along the bottom, thinner than the top bar: {rect:?}"
        );
    }
}

#[test]
fn dragging_the_bottom_bar_leaves_the_window_be() {
    let (_tmp, mut harness) = ready();
    let status = harness.get_by_label_contains("Manifest:").rect().center();
    assert_eq!(drag(&mut harness, status, PointerButton::Primary), []);
}

#[test]
fn screen_readers_reach_the_bottom_bar_after_the_page() {
    let (_tmp, harness) = ready_with_update(Vec2::new(1000.0, 1400.0), false);
    let order: Vec<String> = harness
        .query_all_by(|node| node.label().is_some())
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    let at = |label: &str| order.iter().position(|l| l.contains(label)).unwrap();
    assert!(
        at("Scan for games") < at("Release page"),
        "the page comes first: {order:?}"
    );
}

#[test]
fn update_notice_gives_way_in_a_narrow_window() {
    // Narrower than the window can be, as a zoomed UI or a large text scale makes it in effect.
    let (_tmp, mut harness) = ready_with_update(Vec2::new(300.0, 480.0), true);
    let (_wide_tmp, wide) = ready_with_update(Vec2::new(1100.0, 480.0), true);
    assert_bottom_bar_fits(&harness);
    let notice = |h: &Harness<'static, App>| h.get_by_label("Update available: v99.0.0").rect();
    assert!(
        notice(&harness).width() < notice(&wide).width(),
        "the notice is truncated, and its accessible label is whole"
    );

    harness.get_by_label("Update now").click();
    // The fake release has no installer asset, so the started download fails at once.
    run_until(&mut harness, "the failed download", |h| {
        shows(h, "The update failed")
    });
}

#[test]
fn release_page_link_fits_in_a_narrow_window() {
    let (_tmp, harness) = ready_with_update(Vec2::new(300.0, 480.0), false);
    assert_bottom_bar_fits(&harness);
}

#[test]
fn dropped_notice_stays_the_label_of_the_release_page_link() {
    let (_tmp, harness) = ready_with_update(Vec2::new(160.0, 480.0), false);
    assert!(
        harness
            .query_by_label("Update available: v99.0.0")
            .is_none(),
        "the notice's text is dropped"
    );
    harness.get_by_label("Release page, Update available: v99.0.0");
}

/// Like [`ready_with_update`] for an installed app, after a scan that found no games, so the
/// Backup tab adds the repository and **Back up 0 games** to the bottom bar.
fn scanned_with_update(size: Vec2) -> (tempfile::TempDir, Harness<'static, App>) {
    let (tmp, mut harness) = ready_with_update(size, true);
    harness.get_by_label("Scan for games").click();
    run_until(&mut harness, "the scan", |h| shows(h, "0 games found"));
    common::run_settled(&mut harness);
    (tmp, harness)
}

#[test]
fn the_backup_tab_has_one_bottom_bar_with_the_repository_and_back_up() {
    let (tmp, mut harness) = scanned_with_update(Vec2::new(1000.0, 1400.0));
    let window = harness.ctx.viewport_rect();
    let repository = repo_dir(&tmp);
    let row = [
        "Manifest",
        &repository,
        "Update available",
        "Update now",
        "Back up 0 games",
    ];
    let rects: Vec<Rect> = (row.iter())
        .map(|label| harness.get_by_label_contains(label).rect())
        .collect();
    let back_up = rects[4];
    for (label, rect) in row.iter().zip(&rects) {
        assert!(
            (rect.center().y - back_up.center().y).abs() < 1.0,
            "{label} is on the row of Back up: {rect:?}"
        );
    }
    for (pair, labels) in rects.windows(2).zip(row.windows(2)) {
        assert!(
            pair[0].right() <= pair[1].left(),
            "{} is left of {}",
            labels[0],
            labels[1]
        );
    }
    assert!(back_up.right() <= window.right() - 16.0 && back_up.bottom() <= window.bottom());
    assert_eq!(harness.query_all_by_label_contains("Back up").count(), 1);
    let order: Vec<String> = harness
        .query_all_by(|node| node.label().is_some())
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    let at = |label: &str| order.iter().position(|l| l.contains(label)).unwrap();
    assert!(
        at("Scan for games") < at("Back up 0 games"),
        "the page comes first: {order:?}"
    );

    let backup_gap = window.bottom() - back_up.center().y;
    harness.state_mut().set_tab(Tab::Restore);
    common::run_settled(&mut harness);
    assert!(harness.query_by_label_contains("Back up").is_none());
    let manifest = harness.get_by_label_contains("Manifest").rect();
    assert!(
        window.bottom() - manifest.center().y < backup_gap,
        "the other tabs' bar is thinner: {manifest:?}"
    );
}

#[test]
fn the_bottom_bar_changes_height_without_a_frame_out_of_place() {
    let (_tmp, mut harness) = scanned_with_update(Vec2::new(1000.0, 1400.0));
    let manifest = |h: &Harness<'static, App>| h.get_by_label_contains("Manifest").rect();
    for tab in [Tab::Restore, Tab::Backup] {
        harness.state_mut().set_tab(tab);
        harness.step();
        let first = manifest(&harness);
        common::run_settled(&mut harness);
        assert_eq!(first, manifest(&harness), "on the first frame of {tab:?}");
    }
}

#[test]
fn back_up_stays_in_a_narrow_bottom_bar() {
    let (_wide_tmp, wide) = scanned_with_update(Vec2::new(1400.0, 480.0));
    let width = |h: &Harness<'static, App>, label| {
        (h.query_by_label_contains(label)).map_or(0.0, |node| node.rect().width())
    };
    // The smallest window, and narrower, as a zoomed UI or a large text scale makes it in effect;
    // a bar narrower than its two buttons cannot hold them.
    let (_tmp, smallest) = scanned_with_update(Vec2::new(MIN_WINDOW_SIZE[0], 480.0));
    let (_narrow_tmp, narrow) = scanned_with_update(Vec2::new(400.0, 480.0));
    for harness in [&smallest, &narrow] {
        assert_bottom_bar_fits(harness);
        assert_eq!(
            width(harness, "Back up 0 games"),
            width(&wide, "Back up 0 games"),
            "Back up keeps its width"
        );
    }
    assert!(
        width(&narrow, "Update available") < width(&wide, "Update available"),
        "the notice gives way"
    );

    // Without an update the button fits a bar narrower than the window can be.
    let (_bare_tmp, mut bare) = ready_sized(Vec2::new(300.0, 480.0));
    bare.get_by_label("Scan for games").click();
    run_until(&mut bare, "the scan", |h| shows(h, "0 games found"));
    common::run_settled(&mut bare);
    assert_bottom_bar_fits(&bare);
    assert_eq!(
        width(&bare, "Back up 0 games"),
        width(&wide, "Back up 0 games")
    );
}
