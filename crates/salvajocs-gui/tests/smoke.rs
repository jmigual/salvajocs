//! Headless checks that the window builds its widget tree.

mod common;

use common::{
    env, harness, harness_sized, init_repo, local_config, repo_dir, run_settled, run_until,
    run_until_ready, save_config, shows,
};
use eframe::egui::Vec2;
use egui_kittest::kittest::Queryable;
use salvajocs_gui::tabs::Tab;

#[test]
fn smoke_renders_every_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env);
    run_until_ready(&mut harness);

    for tab in Tab::ALL {
        harness.state_mut().set_tab(tab);
        // Settings loads the retention policy in a job, so plain `run` would not settle.
        run_settled(&mut harness);
        assert!(
            shows(&harness, tab.label()),
            "{} is on the strip",
            tab.label()
        );
        // Only Settings has content so far; the others are empty panels.
        for section in ["Repository", "Compression", "Roots"] {
            assert_eq!(
                shows(&harness, section),
                tab == Tab::Settings,
                "{section} belongs to Settings only, checked on {}",
                tab.label()
            );
        }
    }
    assert!(!shows(&harness, "Welcome to salvajocs"));
}

#[test]
fn first_run_modal_shown_without_config() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));

    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    for label in [
        "Location",
        "Encryption password",
        "Compression (new repository)",
    ] {
        assert!(shows(&harness, label), "{label} is asked on the first run");
    }
}

#[test]
fn modal_taller_than_the_window_scrolls() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness_sized(env(&tmp), Vec2::new(1000.0, 1400.0));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });
    run_settled(&mut harness);

    harness.set_size(Vec2::new(1000.0, 300.0));
    for _ in 0..5 {
        harness.step();
    }
    harness.get_by_label("Continue").scroll_to_me();
    run_settled(&mut harness);

    let bottom = harness.get_by_label("Continue").rect().max.y;
    assert!(bottom <= 300.0, "the dialog's last button is on screen");
}

#[test]
fn modal_in_a_window_shorter_than_its_margins_does_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness_sized(env(&tmp), Vec2::new(1000.0, 20.0));
    for _ in 0..20 {
        harness.step();
    }
}
