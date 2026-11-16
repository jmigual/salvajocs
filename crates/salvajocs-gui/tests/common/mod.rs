//! What every headless test of the window needs: an app on temp directories with a fake
//! keyring and manifest download, a harness, and a way to wait for worker threads.

// Each test binary compiles this separately, so a helper only some of them use reads as dead
// code in the rest.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};
use salvajocs_gui::env::{MemorySecrets, SecretStore};
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// A manifest download that never touches the network.
fn offline(_cache_dir: &Path) -> Result<Freshness, FetchError> {
    Ok(Freshness::NotModified)
}

/// An environment inside `tmp` with an empty fake keyring.
pub fn env(tmp: &TempDir) -> Env {
    env_with(tmp, Arc::new(MemorySecrets::new()))
}

/// An environment inside `tmp` using `secrets`.
pub fn env_with(tmp: &TempDir, secrets: Arc<dyn SecretStore>) -> Env {
    Env {
        config_dir: tmp.path().join("config"),
        cache_dir: tmp.path().join("cache"),
        secrets,
        refresh_manifest: offline,
    }
}

/// Where the tests keep their repository.
pub fn repo_dir(tmp: &TempDir) -> String {
    tmp.path().join("repo").to_str().unwrap().to_string()
}

/// Saves `config` as the app's config file.
pub fn save_config(env: &Env, config: &AppConfig) {
    config.save(&env.config_dir).unwrap();
}

/// A config for a local repository without a password.
pub fn local_config(repository: &str) -> AppConfig {
    AppConfig {
        repository: repository.to_string(),
        repository_options: Vec::new(),
        password: PasswordMode::None,
        roots: Vec::new(),
        custom_games: Vec::new(),
    }
}

/// Creates an empty repository without a password at `repository`.
pub fn init_repo(repository: &str) {
    let location = RepoLocation {
        repository: repository.to_string(),
        options: Default::default(),
    };
    Storage::init(&location, "", Compression::Default).unwrap();
}

/// Drives the app the way eframe does, for as many frames as the test needs.
pub fn harness(env: Env) -> Harness<'static, App> {
    Harness::builder()
        .with_size(Vec2::new(1000.0, 900.0))
        .build_ui_state(
            |ui, app| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            },
            App::new(env),
        )
}

/// Whether any node's label contains `text`, which is how these tests ask "is this on screen".
pub fn shows(harness: &Harness<'static, App>, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

/// Runs frames until `ready` holds, or fails saying what never happened.
///
/// `step`, not `run`: a screen with a job in flight keeps asking to be redrawn, which is what
/// `run` waits to stop.
pub fn run_until(
    harness: &mut Harness<'static, App>,
    what: &str,
    ready: impl Fn(&Harness<'static, App>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if ready(harness) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs frames until the startup jobs are done: the repository is open and the manifest loaded.
pub fn run_until_ready(harness: &mut Harness<'static, App>) {
    run_until(harness, "the repository to open", |h| {
        !shows(h, "Opening the repository") && !shows(h, "Manifest: loading")
    });
}
