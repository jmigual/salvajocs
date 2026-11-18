//! What every headless test of the window needs: an app on temp directories with a fake
//! keyring and manifest download, a harness, and a way to wait for worker threads.

// Each test binary compiles this separately, so a helper only some of them use reads as dead
// code in the rest.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::secret::{SecretError, SecretKey};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};
use salvajocs_gui::env::SecretStore;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// A keyring that keeps secrets in memory.
#[derive(Default)]
pub struct MemorySecrets(Mutex<Vec<(String, SecretKey, String)>>);

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, scope: &str, key: &SecretKey) -> Result<Option<String>, SecretError> {
        let entries = self.0.lock().unwrap();
        Ok(entries
            .iter()
            .find(|(s, k, _)| s == scope && k == key)
            .map(|(_, _, value)| value.clone()))
    }

    fn set(&self, scope: &str, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        self.delete(scope, key)?;
        let mut entries = self.0.lock().unwrap();
        entries.push((scope.to_string(), key.clone(), value.to_string()));
        Ok(())
    }

    fn delete(&self, scope: &str, key: &SecretKey) -> Result<(), SecretError> {
        let mut entries = self.0.lock().unwrap();
        entries.retain(|(s, k, _)| !(s == scope && k == key));
        Ok(())
    }
}

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
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || false,
        run_installer: |_| panic!("tests never start an installer"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
    }
}

/// Makes the folder picker of `env` return `path`, as if the user chose it.
pub fn pick(env: &mut Env, path: PathBuf) {
    env.pick_folder = Arc::new(move || Some(path.clone()));
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
        check_updates: true,
    }
}

/// Creates an empty repository without a password at `repository`.
pub fn init_repo(repository: &str) {
    init_repo_with(repository, "", Compression::Default);
}

/// Creates an empty repository at `repository`.
pub fn init_repo_with(repository: &str, password: &str, compression: Compression) {
    let location = RepoLocation {
        repository: repository.to_string(),
        options: Default::default(),
    };
    Storage::init(&location, password, compression).unwrap();
}

/// Where the tests keep a second repository.
pub fn other_repo_dir(tmp: &TempDir) -> String {
    tmp.path().join("other").to_str().unwrap().to_string()
}

/// Replaces the text of the field named `label`, which `type_text` alone would append to.
pub fn replace_text(harness: &mut Harness<'static, App>, label: &str, text: &str) {
    let field = harness.get_by_label(label);
    field.focus();
    harness.key_combination_modifiers(Modifiers::COMMAND, &[Key::A]);
    harness.get_by_label(label).type_text(text);
    run_settled(harness);
}

/// Drives the app the way eframe does, for as many frames as the test needs.
pub fn harness(env: Env) -> Harness<'static, App> {
    // Tall enough that the longest page, Settings, needs no scrolling: clicks land on the center
    // of a widget, which must be on screen.
    harness_sized(env, Vec2::new(1000.0, 1400.0))
}

/// Like `harness`, in a window of `size`.
pub fn harness_sized(env: Env, size: Vec2) -> Harness<'static, App> {
    Harness::builder().with_size(size).build_ui_state(
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

/// Runs frames until the results show `result` and the backup job has ended, the cleanup after
/// the games included.
pub fn run_until_backed_up(harness: &mut Harness<'static, App>, result: &str) {
    run_until(harness, "the backup to finish", |h| {
        shows(h, result) && !shows(h, "Backed up ") && !shows(h, "Removing old backups")
    });
}

/// Runs frames until the screen stops asking to be redrawn.
///
/// Unlike `run`, a job in flight (a config save, say) is waited out instead of panicking when
/// it outlasts the step budget.
pub fn run_settled(harness: &mut Harness<'static, App>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while harness.run_ok().is_none() {
        assert!(Instant::now() < deadline, "timed out waiting to settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs frames until the startup jobs are done: the repository is open and the manifest loaded.
pub fn run_until_ready(harness: &mut Harness<'static, App>) {
    run_until(harness, "the repository to open", |h| {
        !shows(h, "Opening the repository") && !shows(h, "Manifest: loading")
    });
}
