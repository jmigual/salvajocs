//! Headless checks of the Backup tab.

mod common;

use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{
    env, harness, init_repo, local_config, record_opened, repo_dir, run_settled, run_until,
    run_until_backed_up, run_until_ready, save_config, shows,
};
use eframe::egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::layout::{Base, Root, RootKind};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{self, CustomGame, Integration, Manifest, parse_yaml};
use salvajocs_core::retention::{self, Retention};
use salvajocs_core::selection::{Change, Pending};
use salvajocs_core::slug::slug;
use salvajocs_core::storage::{RepoLocation, Storage};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// A Wine prefix under `dir` with a `steamuser` home and a `user.reg` holding the key of Alpha.
fn make_prefix(dir: &Path) -> PathBuf {
    let prefix = dir.join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    fs::write(
        prefix.join("user.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n\n[Software\\\\Alpha] 1700000000\n#time=1d9\n\"Level\"=dword:00000005\n",
    )
    .unwrap();
    prefix
}

/// A custom game in `prefix` whose saves are the files below `AppData/Roaming/<name>`.
fn custom_game(name: &str, registry: bool, prefix: &Path) -> CustomGame {
    let mut yaml = format!("{name}:\n  files:\n    <winAppData>/{name}: {{}}\n");
    if registry {
        yaml += &format!("  registry:\n    HKEY_CURRENT_USER/Software/{name}: {{}}\n");
    }
    CustomGame {
        name: name.into(),
        files: Vec::new(),
        entry: parse_yaml(&yaml).unwrap().games.remove(name).unwrap(),
        integration: Integration::Override,
        wine_prefix: Some(prefix.to_path_buf()),
        pairs: Vec::new(),
        skips: Vec::new(),
    }
}

/// Writes a save file of `content` for `name` in `prefix`.
fn write_save(prefix: &Path, name: &str, content: &str) {
    let dir = prefix
        .join("drive_c/users/steamuser/AppData/Roaming")
        .join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("save.dat"), content).unwrap();
}

/// A manifest download that fails, as it does without a network.
fn unreachable(_cache_dir: &Path) -> Result<Freshness, FetchError> {
    Err(FetchError::Io(io::Error::other("offline")))
}

/// An app with the repository open and the Backup tab in front, over an empty manifest, with
/// `roots` and `games` in the config.
fn open_tab(roots: Vec<Root>, games: Vec<CustomGame>) -> (Harness<'static, App>, Env, TempDir) {
    open_tab_with(roots, games, true)
}

/// Like [`open_tab`]; without `cached_manifest` there is no manifest cache and the download
/// fails.
fn open_tab_with(
    roots: Vec<Root>,
    games: Vec<CustomGame>,
    cached_manifest: bool,
) -> (Harness<'static, App>, Env, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let (harness, env) = open_app(&tmp, &repository, roots, games, cached_manifest);
    (harness, env, tmp)
}

/// Like [`open_tab_with`], with its config and cache in `tmp` and the existing repository at
/// `repository`, as another machine sharing it.
fn open_app(
    tmp: &TempDir,
    repository: &str,
    roots: Vec<Root>,
    games: Vec<CustomGame>,
    cached_manifest: bool,
) -> (Harness<'static, App>, Env) {
    start_app(env(tmp), repository, roots, games, cached_manifest)
}

/// Like [`open_app`], on `env`.
fn start_app(
    mut env: Env,
    repository: &str,
    roots: Vec<Root>,
    games: Vec<CustomGame>,
    cached_manifest: bool,
) -> (Harness<'static, App>, Env) {
    if cached_manifest {
        fs::create_dir_all(&env.cache_dir).unwrap();
        fs::write(
            env.cache_dir
                .join(salvajocs_core::manifest::fetch::JSON_FILE),
            manifest::to_json(&Manifest::default()).unwrap(),
        )
        .unwrap();
    } else {
        env.refresh_manifest = unreachable;
    }
    let mut config = local_config(repository);
    config.roots = roots;
    config.custom_games = games;
    save_config(&env, &config);
    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    run_settled(&mut harness);
    (harness, env)
}

/// A prefix in `scratch` with two games: Alpha has a registry key, Beta does not.
fn alpha_beta(scratch: &TempDir) -> (PathBuf, Vec<CustomGame>) {
    let prefix = make_prefix(scratch.path());
    write_save(&prefix, "Alpha", "abc");
    write_save(&prefix, "Beta", "defgh");
    let games = vec![
        custom_game("Alpha", true, &prefix),
        custom_game("Beta", false, &prefix),
    ];
    (prefix, games)
}

fn scan(harness: &mut Harness<'static, App>, found: &str) {
    harness.get_by_label("Scan for games").click();
    run_until(harness, "the scan to finish", |h| shows(h, found));
    // What to back up may still be loading; its spinner keeps the frames coming until it is done.
    run_settled(harness);
}

#[test]
fn backup_scan_job_lists_games_with_file_counts() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    assert!(
        !shows(&harness, "games found"),
        "nothing is scanned unasked"
    );

    scan(&mut harness, "2 games found");

    assert!(shows(&harness, "Alpha"));
    assert!(shows(&harness, "Beta"));
    assert!(shows(&harness, "1 file, 3 B, plus registry keys"));
    assert!(shows(&harness, "1 file, 5 B"));
    assert!(shows(&harness, "Back up 2 games"));
}

#[test]
fn a_game_found_in_a_wine_prefix_shows_the_prefix() {
    let scratch = tempfile::tempdir().unwrap();
    let (prefix, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);

    scan(&mut harness, "2 games found");

    let line = format!("Wine prefix: {}", prefix.display());
    assert_eq!(harness.query_all_by_label(&line).count(), 2);
}

#[test]
fn backup_scan_problems_are_shown() {
    let scratch = tempfile::tempdir().unwrap();
    let prefix = make_prefix(scratch.path());
    fs::remove_dir(prefix.join("drive_c/users/steamuser")).unwrap();
    for user in ["alice", "bob"] {
        fs::create_dir(prefix.join("drive_c/users").join(user)).unwrap();
    }
    let roots = vec![Root {
        path: prefix,
        kind: RootKind::WinePrefix,
    }];
    let (mut harness, _env, _tmp) = open_tab(roots, Vec::new());

    scan(&mut harness, "0 games found");

    assert!(shows(&harness, "Scan problems (1)"));
    assert!(shows(&harness, "several users (alice, bob)"));
}

#[test]
fn backup_reports_new_then_unchanged_per_game() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");

    click(&mut harness, "Beta");
    assert!(shows(&harness, "Back up 1 game"));
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    assert!(!shows(&harness, "Beta:"), "a deselected game is left out");
    let stage = env.cache_dir.join("stage");
    assert!(stage.join(slug("Alpha")).exists());
    assert!(!stage.join(slug("Beta")).exists());

    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: up to date");
    assert!(!shows(&harness, ": backed up"));
}

#[test]
fn backup_failing_game_does_not_stop_the_others() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    // A file where Alpha's staging dir belongs makes staging it fail.
    fs::create_dir_all(env.cache_dir.join("stage")).unwrap();
    fs::write(env.cache_dir.join("stage").join(slug("Alpha")), "").unwrap();

    harness.get_by_label("Back up 2 games").click();
    run_until_backed_up(&mut harness, "Beta: backed up");

    assert!(shows(&harness, "Alpha: failed:"));
}

#[test]
fn backup_selection_survives_a_rescan() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "Beta");
    assert!(shows(&harness, "Back up 1 game"));

    harness.get_by_label("Scan for games").click();
    harness.step();
    run_until(&mut harness, "the rescan to finish", |h| {
        !shows(h, "Scanning")
    });

    assert!(shows(&harness, "Back up 1 game"), "Beta stays deselected");
}

#[test]
fn backup_without_a_manifest_still_scans_custom_games() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab_with(Vec::new(), games, false);
    assert!(shows(&harness, "only custom games are scanned"));

    scan(&mut harness, "2 games found");
}

#[test]
fn backup_includes_a_save_made_since_the_scan() {
    let scratch = tempfile::tempdir().unwrap();
    let (prefix, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    let saves = prefix.join("drive_c/users/steamuser/AppData/Roaming/Alpha");
    fs::write(saves.join("later.dat"), "xy").unwrap();

    harness.get_by_label("Back up 2 games").click();
    run_until(&mut harness, "the list to follow the backup", |h| {
        shows(h, "2 files, 5 B, plus registry keys")
    });

    let staged = env
        .cache_dir
        .join("stage")
        .join(slug("Alpha"))
        .join("files")
        .join(Base::WinAppData.dir_name())
        .join("Alpha/later.dat");
    assert!(staged.exists(), "{}", staged.display());
}

/// Whether the checkbox `label` is checked.
fn checked(harness: &Harness<'static, App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().toggled() == Some(Toggled::True)
}

/// Clicks `label` and runs frames until the screen settles. The pending file write a choice
/// starts asks for no frames, so it may still run; a Back up click waits for it.
fn click(harness: &mut Harness<'static, App>, label: &str) {
    harness.get_by_label(label).click();
    run_settled(harness);
}

/// The unsaved changes in the pending file of `env`, by title, for its only repository.
fn pending_changes(env: &Env) -> std::collections::BTreeMap<String, Vec<Change>> {
    let pending = Pending::load(&env.config_dir).unwrap();
    pending
        .repositories
        .into_values()
        .next()
        .unwrap_or_default()
}

/// Runs frames until the pending file of `env` holds `changes` for `title`.
fn run_until_pending(
    harness: &mut Harness<'static, App>,
    env: &Env,
    title: &str,
    changes: &[Change],
) {
    run_until(harness, "the choice to be written", |_| {
        Pending::load(&env.config_dir).is_ok_and(|pending| {
            let mut games = pending.repositories.into_values();
            games
                .next()
                .is_some_and(|games| games.get(title).is_some_and(|c| c == changes))
        })
    });
}

/// Stores a settings snapshot of a newer format for `title` in the repository of `tmp`, as a
/// newer version on another machine would.
fn write_newer_settings(tmp: &TempDir, title: &str) {
    let dir = tmp.path().join("newer-settings");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("settings.json"), r#"{"format": 99}"#).unwrap();
    let slug = slug(title);
    let tag = format!("settings:{slug}");
    open_repo(tmp)
        .backup_dir(
            &dir,
            &format!("/salvajocs-settings/{slug}"),
            &["salvajocs-settings", &tag],
            title,
        )
        .unwrap()
        .unwrap();
}

/// The repository of `tmp`, opened apart from the app.
fn open_repo(tmp: &TempDir) -> Storage {
    let location = RepoLocation {
        repository: repo_dir(tmp),
        options: Default::default(),
    };
    Storage::open(&location, "").unwrap()
}

/// Two games without registry keys: Alpha has `save.dat` and `sub/x.dat`, Beta `save.dat`.
fn alpha_with_sub_and_beta(scratch: &TempDir) -> (PathBuf, Vec<CustomGame>) {
    let prefix = make_prefix(scratch.path());
    write_save(&prefix, "Alpha", "abc");
    write_save(&prefix, "Alpha/sub", "xy");
    write_save(&prefix, "Beta", "defgh");
    let games = vec![
        custom_game("Alpha", false, &prefix),
        custom_game("Beta", false, &prefix),
    ];
    (prefix, games)
}

/// Runs frames until the backup job has ended and Beta was backed up.
fn back_up_until_beta(harness: &mut Harness<'static, App>, button: &str) {
    harness.get_by_label(button).click();
    harness.step();
    run_until_backed_up(harness, "Beta: backed up");
}

#[test]
fn backup_saves_the_choices_for_other_machines() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, tmp) = open_tab(Vec::new(), games.clone());
    scan(&mut harness, "2 games found");
    click(&mut harness, "Beta");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    assert!(
        pending_changes(&env).is_empty(),
        "the saved changes left the file"
    );

    let other = tempfile::tempdir().unwrap();
    let (mut second, _) = open_app(&other, &repo_dir(&tmp), Vec::new(), games, true);
    scan(&mut second, "2 games found");

    assert!(!checked(&second, "Beta"));
    assert!(checked(&second, "Alpha"));
    assert!(shows(&second, "Back up 1 game"));
}

#[test]
fn a_choice_waits_in_the_pending_file_until_a_backup() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut first, env, tmp) = open_tab(Vec::new(), games);
    scan(&mut first, "2 games found");
    click(&mut first, "Beta");
    run_until_pending(&mut first, &env, "Beta", &[Change::Skip(true)]);
    drop(first);
    let saved = open_repo(&tmp).snapshots("salvajocs-settings").unwrap();
    assert!(
        saved.is_empty(),
        "nothing is saved to the repository before a backup"
    );

    let mut restarted = harness(env);
    run_until_ready(&mut restarted);
    run_settled(&mut restarted);
    scan(&mut restarted, "2 games found");
    assert!(!checked(&restarted, "Beta"));
}

/// Like [`open_tab`] without roots, recording the folders the app is asked to open.
fn open_tab_recording(
    games: Vec<CustomGame>,
) -> (Harness<'static, App>, Arc<Mutex<Vec<PathBuf>>>, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut env = env(&tmp);
    let opened = record_opened(&mut env);
    let (harness, _) = start_app(env, &repository, Vec::new(), games, true);
    (harness, opened, tmp)
}

#[test]
fn a_folder_button_opens_the_folder_on_disk() {
    let scratch = tempfile::tempdir().unwrap();
    let (prefix, games) = alpha_with_sub_and_beta(&scratch);
    let (mut harness, opened, _tmp) = open_tab_recording(games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "2 files, 5 B");

    click(&mut harness, "Show WinAppData in the file manager");
    click(&mut harness, "Open WinAppData");
    click(&mut harness, "Show Alpha in the file manager");

    let roaming = prefix.join("drive_c/users/steamuser/AppData/Roaming");
    assert_eq!(
        *opened.lock().unwrap(),
        [roaming.clone(), roaming.join("Alpha")]
    );
}

#[test]
fn a_merged_case_variant_folder_opens_the_first_variant() {
    let scratch = tempfile::tempdir().unwrap();
    let prefix = make_prefix(scratch.path());
    let alpha = prefix.join("drive_c/users/steamuser/AppData/Roaming/Alpha");
    for (dir, file) in [("Sub", "a.dat"), ("sub", "b.dat")] {
        fs::create_dir_all(alpha.join(dir)).unwrap();
        fs::write(alpha.join(dir).join(file), "x").unwrap();
    }
    let (mut harness, opened, _tmp) =
        open_tab_recording(vec![custom_game("Alpha", false, &prefix)]);
    scan(&mut harness, "1 game found");
    click(&mut harness, "2 files, 2 B");
    click(&mut harness, "Open WinAppData");
    click(&mut harness, "Open Alpha");

    click(&mut harness, "Show Sub in the file manager");

    assert_eq!(*opened.lock().unwrap(), [alpha.join("Sub")]);
}

#[test]
fn an_excluded_folder_is_left_out_of_the_backup() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_with_sub_and_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "2 files, 5 B");
    click(&mut harness, "Open WinAppData");
    click(&mut harness, "Open Alpha");
    click(&mut harness, "sub");
    assert!(shows(&harness, "1 file, 3 B, 1 excluded"));

    back_up_until_beta(&mut harness, "Back up 2 games");

    assert!(shows(&harness, "Alpha: backed up"));
    let staged = env
        .cache_dir
        .join("stage")
        .join(slug("Alpha"))
        .join("files")
        .join(Base::WinAppData.dir_name())
        .join("Alpha");
    assert!(staged.join("save.dat").exists(), "{}", staged.display());
    assert!(!staged.join("sub").exists(), "{}", staged.display());
}

#[test]
fn a_game_with_every_file_excluded_gets_no_snapshot() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_with_sub_and_beta(&scratch);
    let (mut harness, _env, tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "2 files, 5 B");
    click(&mut harness, "WinAppData");
    assert!(shows(&harness, "0 files, 0 B, 2 excluded"));

    back_up_until_beta(&mut harness, "Back up 1 game");

    assert!(!shows(&harness, "Alpha:"));
    let tag = format!("game:{}", slug("Alpha"));
    assert!(open_repo(&tmp).snapshots(&tag).unwrap().is_empty());
}

#[test]
fn a_pending_file_of_a_newer_version_is_never_written_over() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let pending = tmp.path().join("config/pending-selection.json");
    fs::create_dir_all(pending.parent().unwrap()).unwrap();
    let newer = r#"{"format": 99, "repositories": {}}"#;
    fs::write(&pending, newer).unwrap();
    let (mut harness, _env) = open_app(&tmp, &repository, Vec::new(), games, true);
    scan(&mut harness, "2 games found");

    assert!(shows(
        &harness,
        "pending-selection.json in the config folder cannot be read"
    ));
    assert!(harness.get_by_label("Beta").accesskit_node().is_disabled());
    back_up_until_beta(&mut harness, "Back up 2 games");

    assert!(harness.get_by_label("Beta").accesskit_node().is_disabled());
    click(&mut harness, "Beta");
    assert_eq!(fs::read_to_string(&pending).unwrap(), newer);
}

#[test]
fn a_backup_follows_choices_saved_by_another_machine_since_it_loaded() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut first, _, tmp) = open_tab(Vec::new(), games.clone());
    let other = tempfile::tempdir().unwrap();
    let (mut second, second_env) = open_app(&other, &repo_dir(&tmp), Vec::new(), games, true);
    scan(&mut first, "2 games found");
    scan(&mut second, "2 games found");

    click(&mut first, "Beta");
    first.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut first, "Alpha: backed up");
    assert!(checked(&second, "Beta"), "the second machine loaded before");
    second.get_by_label("Back up 2 games").click();
    second.step();
    run_until_backed_up(&mut second, "Alpha: ");

    assert!(!shows(&second, "Beta:"));
    assert!(
        !second_env
            .cache_dir
            .join("stage")
            .join(slug("Beta"))
            .exists()
    );
    assert!(!checked(&second, "Beta"), "the checkboxes follow");
}

#[test]
fn back_up_right_after_a_click_saves_that_click() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");

    harness.get_by_label("Beta").click();
    harness.step();
    harness.step();
    harness.get_by_label("Back up 1 game").click();
    harness.step();
    run_until_backed_up(&mut harness, "Alpha: backed up");

    assert!(!shows(&harness, "Beta:"));
    assert!(pending_changes(&env).is_empty(), "the click was saved");
}

#[test]
fn choices_are_written_while_another_tab_is_in_front() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");

    // Two changes in one frame: the second write waits for the first.
    harness.get_by_label("Deselect all").click();
    harness.step();
    harness.state_mut().set_tab(Tab::Settings);

    run_until_pending(&mut harness, &env, "Beta", &[Change::Skip(true)]);
    assert_eq!(pending_changes(&env)["Alpha"], [Change::Skip(true)]);
}

#[test]
fn closing_the_app_writes_the_queued_choices() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");

    harness.get_by_label("Deselect all").click();
    harness.step();
    eframe::App::on_exit(harness.state_mut(), None);

    let changes = pending_changes(&env);
    assert_eq!(changes["Alpha"], [Change::Skip(true)]);
    assert_eq!(changes["Beta"], [Change::Skip(true)]);
}

#[test]
fn a_game_whose_choices_cannot_be_read_is_backed_up_whole() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    write_newer_settings(&tmp, "Beta");
    let (mut harness, env) = open_app(&tmp, &repository, Vec::new(), games, true);
    scan(&mut harness, "2 games found");

    assert!(shows(&harness, "What to back up cannot be read"));
    assert!(harness.get_by_label("Beta").accesskit_node().is_disabled());
    assert!(checked(&harness, "Beta"));
    back_up_until_beta(&mut harness, "Back up 2 games");

    let staged = env
        .cache_dir
        .join("stage")
        .join(slug("Beta"))
        .join("files")
        .join(Base::WinAppData.dir_name())
        .join("Beta/save.dat");
    assert!(staged.exists(), "{}", staged.display());
}

#[test]
fn a_change_to_choices_another_version_replaced_stays_pending() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "Beta");
    write_newer_settings(&tmp, "Beta");

    harness.get_by_label("Back up 1 game").click();
    harness.step();
    run_until_backed_up(&mut harness, "Alpha: backed up");

    assert!(shows(&harness, "What to back up was not saved for Beta"));
    assert_eq!(pending_changes(&env)["Beta"], [Change::Skip(true)]);
}

#[test]
fn an_unchecked_registry_key_is_left_out_of_the_backup() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "1 file, 3 B, plus registry keys");
    let key = harness
        .query_all_by_label_contains("Software")
        .next()
        .unwrap()
        .accesskit_node()
        .label()
        .unwrap()
        .to_string();
    click(&mut harness, &key);
    assert!(shows(&harness, "1 file, 3 B"));
    assert!(!shows(&harness, "plus registry keys"));
    let unchecked = [Change::Registry {
        key: key.clone(),
        included: false,
    }];
    run_until_pending(&mut harness, &env, "Alpha", &unchecked);

    back_up_until_beta(&mut harness, "Back up 2 games");

    assert!(shows(&harness, "Alpha: backed up"));
    let stage = env.cache_dir.join("stage").join(slug("Alpha"));
    assert!(stage.join("files").exists());
    assert!(!stage.join("registry.json").exists());
}

/// Backs up Alpha alone, changes its save and backs it up again, in an app over the existing
/// repository of `tmp`, calling `between` in between. Returns the app and how many snapshots of
/// Alpha the repository holds.
fn back_up_a_change(
    tmp: &TempDir,
    between: impl FnOnce(&mut Harness<'static, App>),
) -> (Harness<'static, App>, usize) {
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "Alpha", "abc");
    let games = vec![custom_game("Alpha", false, &prefix)];
    let (mut harness, _) = open_app(tmp, &repo_dir(tmp), Vec::new(), games, true);
    scan(&mut harness, "1 game found");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    assert!(!shows(&harness, "Removed"), "one backup is all there is");
    between(&mut harness);

    write_save(&prefix, "Alpha", "changed");
    harness.get_by_label("Back up 1 game").click();
    // The new backup clears the first one's results as it starts.
    run_until(&mut harness, "the second backup to start", |h| {
        !shows(h, "Alpha: backed up")
    });
    run_until_backed_up(&mut harness, "Alpha: backed up");
    let tag = format!("game:{}", slug("Alpha"));
    let left = open_repo(tmp).snapshots(&tag).unwrap().len();
    (harness, left)
}

#[test]
fn a_backup_removes_the_old_backups_the_policy_forgets() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));
    keep_last_one(&tmp);

    let (harness, left) = back_up_a_change(&tmp, |_| {});

    assert_eq!(left, 1);
    assert!(shows(&harness, "Removed 1 old backup"));
}

#[test]
fn removing_old_backups_drops_the_loaded_restore_list() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));
    keep_last_one(&tmp);

    let (mut harness, _) = back_up_a_change(&tmp, |harness| {
        harness.state_mut().set_tab(Tab::Restore);
        run_settled(harness);
        harness.get_by_label("Load backups").click();
        run_until(harness, "the list", |h| shows(h, "Alpha"));
        harness.state_mut().set_tab(Tab::Backup);
        run_settled(harness);
    });
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);

    assert!(!shows(&harness, "Alpha"), "the listed backup may be gone");
    assert!(shows(&harness, "Load backups"));
}

/// Stores a retention policy that keeps each game's last backup in the repository of `tmp`.
fn keep_last_one(tmp: &TempDir) {
    let policy = Retention {
        keep_last: NonZeroU32::new(1),
        keep_within: None,
    };
    retention::save(&mut open_repo(tmp), &policy, &tmp.path().join("policy")).unwrap();
}

#[test]
fn a_failed_game_does_not_stop_the_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));
    keep_last_one(&tmp);
    let (prefix, games) = alpha_beta(&tmp);
    let (mut harness, env) = open_app(&tmp, &repo_dir(&tmp), Vec::new(), games, true);
    scan(&mut harness, "2 games found");
    // A file where Alpha's staging dir belongs makes staging it fail.
    fs::create_dir_all(env.cache_dir.join("stage")).unwrap();
    fs::write(env.cache_dir.join("stage").join(slug("Alpha")), "").unwrap();
    harness.get_by_label("Back up 2 games").click();
    run_until_backed_up(&mut harness, "Beta: backed up");

    write_save(&prefix, "Beta", "changed");
    harness.get_by_label("Back up 2 games").click();
    run_until(&mut harness, "the second backup to start", |h| {
        !shows(h, "Beta: backed up")
    });
    run_until_backed_up(&mut harness, "Beta: backed up");

    assert!(shows(&harness, "Alpha: failed:"));
    assert!(shows(&harness, "Removed 1 old backup"));
}

#[test]
fn without_a_policy_every_backup_stays() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));

    let (harness, left) = back_up_a_change(&tmp, |_| {});

    assert_eq!(left, 2);
    assert!(!shows(&harness, "Removed"));
    assert!(!shows(&harness, "Cleanup did not finish"));
}

#[test]
fn an_unreadable_policy_removes_nothing_and_warns() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));
    let dir = tmp.path().join("newer-policy");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("retention.json"), r#"{"format": 99}"#).unwrap();
    let tags = ["salvajocs-settings", "settings:retention"];
    open_repo(&tmp)
        .backup_dir(
            &dir,
            "/salvajocs-settings/retention",
            &tags,
            "Retention policy",
        )
        .unwrap()
        .unwrap();

    let (harness, left) = back_up_a_change(&tmp, |_| {});

    assert_eq!(left, 2);
    assert!(shows(&harness, "The cleanup rules could not be read"));
    assert!(!shows(&harness, "Cleanup did not finish"));
}
