//! Headless checks that the window builds its widget tree.

mod common;

use common::{
    env, harness, init_repo, local_config, repo_dir, run_until, run_until_ready, save_config, shows,
};
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
        harness.run();
        assert!(
            shows(&harness, tab.label()),
            "{} is on the strip",
            tab.label()
        );
    }
    assert!(shows(&harness, "Repository"), "Settings shows its sections");
    assert!(!shows(&harness, "Welcome to salvajocs"));
}

#[test]
fn first_run_modal_shown_without_config() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));

    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    for label in ["Location", "Password", "Compression (new repository)"] {
        assert!(shows(&harness, label), "{label} is asked on the first run");
    }
}
