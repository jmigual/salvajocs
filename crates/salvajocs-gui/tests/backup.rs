//! Headless checks of the Backup tab.

mod common;

use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{
    env, harness, init_repo, init_repo_with, local_config, record_opened, replace_text, repo_dir,
    run_settled, run_until, run_until_backed_up, run_until_ready, save_config, shows, shows_path,
};
use eframe::egui::Vec2;
use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::backup::backup_games;
use salvajocs_core::config::{AppConfig, ListSort, SortKey};
use salvajocs_core::launcher::{Launcher, LauncherGame, LauncherPrefix};
use salvajocs_core::layout::{Base, Root, RootKind};
use salvajocs_core::manifest::fetch::{FetchError, Freshness, JSON_FILE};
use salvajocs_core::manifest::{
    self, CustomFile, CustomGame, Integration, Manifest, Pair, parse_yaml,
};
use salvajocs_core::retention::{self, Retention};
use salvajocs_core::scan::{FoundFile, GameScan};
use salvajocs_core::selection::{Change, GameSettings, Pending};
use salvajocs_core::slug::slug;
use salvajocs_core::storage::{Compression, RepoLocation, Storage};
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
    let entry = parse_yaml(&yaml).unwrap().games.remove(name).unwrap();
    let mut game = CustomGame::new(name.into(), entry, Integration::Override);
    game.wine_prefix = Some(prefix.to_path_buf());
    game
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
    // The custom games' first read rewrites the config once (ADR-0028).
    let config_dir = env.config_dir.clone();
    run_until(&mut harness, "the custom games to be read", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| config.custom_games_repository.is_some())
    });
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
    // A rescan shows the old report until it ends, and its progress bar asks for no frames.
    run_until(harness, "the scan to finish", |h| {
        shows(h, found) && !shows(h, "Scanning") && !shows(h, "Checked")
    });
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
fn back_up_is_in_the_window_bottom_bar() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    let back_up = harness.get_by_label("Back up 2 games").rect();
    let status = harness.get_by_label_contains("Manifest:").rect();
    assert!(
        (status.center().y - back_up.center().y).abs() < 1.0 && status.right() <= back_up.left(),
        "{back_up:?} is on the row of {status:?}, to its right"
    );
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
fn a_game_found_in_a_launcher_prefix_shows_the_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "Alpha", "abc");
    let mut env = env(&tmp);
    let mut game = LauncherGame::default();
    game.title = Some("ALPHA™".to_owned());
    let found = LauncherPrefix::new(Launcher::Lutris, prefix.clone(), Some(game));
    env.environment
        .as_mut()
        .unwrap()
        .launcher_prefixes
        .push(found);
    fs::create_dir_all(&env.cache_dir).unwrap();
    let manifest = parse_yaml("Alpha:\n  files:\n    <winAppData>/Alpha: {}\n").unwrap();
    fs::write(
        env.cache_dir.join(JSON_FILE),
        manifest::to_json(&manifest).unwrap(),
    )
    .unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    run_settled(&mut harness);

    scan(&mut harness, "1 game found");

    assert!(shows(
        &harness,
        &format!("Wine prefix: {}", prefix.display())
    ));
}

#[test]
fn backup_scan_problems_are_shown() {
    let scratch = tempfile::tempdir().unwrap();
    let prefix = make_prefix(scratch.path());
    fs::remove_dir(prefix.join("drive_c/users/steamuser")).unwrap();
    for user in ["alice", "bob"] {
        fs::create_dir(prefix.join("drive_c/users").join(user)).unwrap();
    }
    let roots = vec![Root::new(prefix, RootKind::WinePrefix)];
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

/// The role of the one node whose label holds `text`.
fn role_of(harness: &Harness<'static, App>, text: &str) -> Role {
    harness.get_by_label_contains(text).accesskit_node().role()
}

/// Runs frames until the comparison with the last backups is done.
fn run_until_compared(harness: &mut Harness<'static, App>) {
    run_until(harness, "the comparison to finish", |h| {
        !shows(h, "Comparing with the last backups")
    });
    run_settled(harness);
}

#[test]
fn a_rescan_shows_what_differs_from_the_last_backup() {
    let scratch = tempfile::tempdir().unwrap();
    let (prefix, games) = alpha_beta(&scratch);
    let alpha = prefix.join("drive_c/users/steamuser/AppData/Roaming/Alpha");
    fs::create_dir_all(alpha.join("sub")).unwrap();
    fs::write(alpha.join("sub/old.dat"), "xy").unwrap();
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    run_until_compared(&mut harness);
    assert_eq!(harness.query_all_by_label("No backup yet").count(), 2);
    assert!(
        (harness.query_all_by_label("No backup yet"))
            .all(|node| node.accesskit_node().role() == Role::Image)
    );

    // A skipped game says nothing about the next backup.
    click(&mut harness, "Beta");
    assert_eq!(harness.query_all_by_label("No backup yet").count(), 1);
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    run_until_compared(&mut harness);
    // One check whose tooltip promises the skip.
    let check = "No changes since the backup from ";
    assert!(shows(&harness, check));
    assert!(
        (harness
            .get_by_label_contains(check)
            .accesskit_node()
            .label())
        .is_some_and(|label| label.ends_with("; no backup will be made"))
    );
    assert_eq!(role_of(&harness, check), Role::Image);

    fs::write(alpha.join("save.dat"), "abcd").unwrap();
    fs::remove_file(alpha.join("sub/old.dat")).unwrap();
    fs::write(alpha.join("new.dat"), "n").unwrap();
    scan(&mut harness, "2 games found");
    run_until_compared(&mut harness);
    let chips = "1 new, 1 changed, 1 removed since the backup from ";
    assert_eq!(role_of(&harness, chips), Role::Image);

    click(&mut harness, "2 files, 5 B, plus registry keys");
    click(&mut harness, "Open WinAppData/Alpha");
    click(&mut harness, "Open sub");
    for row in ["new.dat, new", "save.dat, changed", "old.dat, removed"] {
        assert_eq!(harness.query_all_by_label(row).count(), 1, "{row}");
    }
    for icon in [
        "New: not in the backup",
        "Changed since the backup",
        "Removed: the next backup will not hold it",
    ] {
        let role = harness.get_by_label(icon).accesskit_node().role();
        assert_eq!(role, Role::Image, "{icon}");
    }

    // An unticked file that the backup holds will be removed from the next one.
    click(&mut harness, "save.dat, changed");
    assert_eq!(
        role_of(&harness, "1 new, 2 removed since the backup from "),
        Role::Image
    );
    assert_eq!(harness.query_all_by_label("save.dat, removed").count(), 1);
    // `old.dat`'s and `save.dat`'s, which is still on disk.
    let removed = "Removed: the next backup will not hold it";
    assert_eq!(harness.query_all_by_label(removed).count(), 2);
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
        !shows(h, "Scanning") && !shows(h, "Checked")
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
    let location = RepoLocation::new(repo_dir(tmp), Default::default());
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
    // The custom games went to the repository when it opened (ADR-0028); no selection did.
    let saved = salvajocs_core::selection::load_all(&mut open_repo(&tmp)).unwrap();
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

    click(&mut harness, "Show WinAppData/Alpha in the file manager");
    click(&mut harness, "Open WinAppData/Alpha");
    click(&mut harness, "Show sub in the file manager");

    let alpha = prefix.join("drive_c/users/steamuser/AppData/Roaming/Alpha");
    assert_eq!(*opened.lock().unwrap(), [alpha.clone(), alpha.join("sub")]);
}

#[test]
fn dismiss_stays_on_screen_beside_a_long_notice() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_with_sub_and_beta(&scratch);
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut env = env(&tmp);
    env.open_folder = Arc::new(|_| Err(io::Error::other("no file manager here, ".repeat(10))));
    let (mut harness, _) = start_app(env, &repository, Vec::new(), games, true);
    scan(&mut harness, "2 games found");
    click(&mut harness, "2 files, 5 B");
    click(&mut harness, "Show WinAppData/Alpha in the file manager");
    run_until(&mut harness, "the notice", |h| shows(h, "Could not open"));

    harness.set_size(Vec2::new(400.0, 900.0));
    run_settled(&mut harness);

    let right = harness.get_by_label("Dismiss").rect().max.x;
    assert!(right <= 400.0, "Dismiss is on screen");
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

    click(
        &mut harness,
        "Show WinAppData/Alpha/Sub in the file manager",
    );

    assert_eq!(*opened.lock().unwrap(), [alpha.join("Sub")]);
}

#[test]
fn ticking_a_merged_row_clears_a_saved_choice_on_its_upper_folder() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_with_sub_and_beta(&scratch);
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let env = env(&tmp);
    // `WinAppData` is the upper folder of the row `WinAppData/Alpha`.
    let excluded = Change::Path {
        node: "WinAppData".into(),
        included: false,
    };
    let mut pending = Pending::default();
    pending
        .games_mut(&local_config(&repository))
        .insert("Alpha".into(), vec![excluded.clone()]);
    pending.save(&env.config_dir).unwrap();
    let (mut harness, env) = start_app(env, &repository, Vec::new(), games, true);
    scan(&mut harness, "2 games found");
    click(&mut harness, "0 files, 0 B, 2 excluded");
    assert!(!checked(&harness, "WinAppData/Alpha"));

    click(&mut harness, "WinAppData/Alpha");

    let ticked = Change::Path {
        node: "WinAppData".into(),
        included: true,
    };
    run_until_pending(
        &mut harness,
        &env,
        "Alpha",
        &[excluded.clone(), ticked.clone()],
    );
    let mut settings = GameSettings::default();
    settings.apply_changes(&[excluded, ticked]);
    assert!(settings.paths.is_empty(), "{:?}", settings.paths);
}

#[test]
fn an_excluded_folder_is_left_out_of_the_backup() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_with_sub_and_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "2 files, 5 B");
    click(&mut harness, "Open WinAppData/Alpha");
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
    click(&mut harness, "WinAppData/Alpha");
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

#[test]
fn removing_old_backups_forgets_the_plan_of_a_chosen_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&repo_dir(&tmp));
    keep_last_one(&tmp);

    let (mut harness, _) = back_up_a_change(&tmp, |harness| {
        harness.state_mut().set_tab(Tab::Restore);
        run_settled(harness);
        harness.get_by_label("Load backups").click();
        run_until(harness, "the list", |h| shows(h, "Alpha"));
        harness.get_by_label("Alpha").click();
        run_until(harness, "the plan", |h| shows(h, "Files to write"));
        harness.state_mut().set_tab(Tab::Backup);
        run_settled(harness);
    });
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);

    assert!(
        !shows(&harness, "Files to write"),
        "the snapshot may be gone"
    );
    assert!(shows(&harness, "Load backups"));
}

#[test]
fn a_new_backup_lists_the_restore_tab_again_and_keeps_its_choice() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "Beta");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    click(&mut harness, "Load backups");
    run_until(&mut harness, "the list", |h| {
        h.query_by_label("Include Alpha").is_some()
    });
    harness.get_by_label("Alpha").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    assert!(harness.query_by_label("Include Beta").is_none());

    harness.state_mut().set_tab(Tab::Backup);
    run_settled(&mut harness);
    click(&mut harness, "Beta");
    click(&mut harness, "Alpha");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Beta: backed up");
    harness.state_mut().set_tab(Tab::Restore);

    run_until(&mut harness, "the new game in the list", |h| {
        h.query_by_label("Include Beta").is_some()
    });
    assert!(shows(&harness, "Restore into:"), "Alpha stays chosen");
}

/// Stores a retention policy that keeps each game's last backup in the repository of `tmp`.
fn keep_last_one(tmp: &TempDir) {
    let mut policy = Retention::default();
    policy.keep_last = NonZeroU32::new(1);
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

/// Backs up the game `G` of the Linux build into the repository of `tmp`, with this machine's
/// host, as a dual boot would: `<home>/G/save.dat` holding `content`.
fn back_up_linux_build(tmp: &TempDir, content: &str) {
    let real = tmp.path().join("linux/G/save.dat");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, content).unwrap();
    let mut scan = GameScan::default();
    scan.title = "G".into();
    let size = content.len() as u64;
    scan.files = vec![FoundFile::new(
        Base::Home,
        "G/save.dat".into(),
        real,
        size,
        None,
    )];
    scan.roots = vec![(Base::Home, "G".into())];
    let stage = tmp.path().join("linux-stage");
    backup_games(&mut open_repo(tmp), &[scan], &stage, &[], None, |_| {}).unwrap();
}

/// The custom game `G` of the Windows build in `prefix`, from the manifest-style `yaml` of its
/// entry.
fn windows_build(yaml: &str, prefix: &Path) -> CustomGame {
    let entry = parse_yaml(yaml).unwrap().games.remove("G").unwrap();
    let mut game = CustomGame::new("G".into(), entry, Integration::Override);
    game.wine_prefix = Some(prefix.to_path_buf());
    game
}

/// `G` with the row `<home>/G` ↔ `<winAppData>/G`, in `prefix`, plus `entry` (YAML lines below
/// the title).
fn paired(entry: &str, prefix: &Path) -> CustomGame {
    let mut game = windows_build(&format!("G:\n{entry}"), prefix);
    game.files = vec![CustomFile {
        linux: Some("<home>/G".into()),
        windows: Some("<winAppData>/G".into()),
    }];
    game
}

/// `G` in `prefix` with two Linux save folders, so no auto-pair, and no rows: as Factorio has
/// it, so `<home>/G` of the Linux build has no known place in the Windows build.
fn like_factorio(prefix: &Path) -> CustomGame {
    let os = |os: &str| format!("      tags: [save]\n      when:\n        - os: {os}\n");
    let yaml = format!(
        "G:\n  files:\n    <home>/G:\n{}    <home>/G2:\n{}    <winAppData>/G:\n{}",
        os("linux"),
        os("linux"),
        os("windows")
    );
    windows_build(&yaml, prefix)
}

/// The label of the one node whose label contains `text`.
fn label_of(harness: &Harness<'static, App>, text: &str) -> String {
    let node = harness.get_by_label_contains(text);
    node.accesskit_node().label().unwrap_or_default().to_owned()
}

#[test]
fn a_backup_from_the_other_build_compares_through_the_folder_pairs() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abc");
    let game = paired("  files: {}\n", &prefix);
    let (mut harness, env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);
    let config_file = env.config_dir.join("config.yaml");
    let config = fs::read(&config_file).unwrap();

    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);

    let label = label_of(&harness, "No changes since the backup from ");
    assert!(
        label.ends_with(
            "; no backup will be made. The backup is from the other build; its folders were \
             matched to this machine's."
        ),
        "{label}"
    );
    assert!(!shows(&harness, "removed"));
    assert_eq!(fs::read(&config_file).unwrap(), config, "nothing is saved");

    // Skipped by the mapped content: rustic alone stores another tree for this host's parent.
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "G: up to date");
}

#[test]
fn a_folder_of_the_other_build_with_no_known_place_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abc");
    let game = like_factorio(&prefix);
    let (mut harness, _env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);

    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);

    let label = label_of(&harness, "1 new, 1 removed since the backup from ");
    assert!(
        label.contains(
            "The newest backup is from the other build; these folders have no known place here: \
             <home>/G."
        ),
        "{label}"
    );
    click(&mut harness, "1 file, 3 B");
    click(&mut harness, "Open Home/G");
    click(&mut harness, "Open WinAppData/G");
    assert!(shows(&harness, "save.dat, removed"));
    assert!(shows(&harness, "save.dat, new"));
    let why = "Removed: the newest backup is from the other build, and this file has no place \
               here (its folder is unknown, or another file takes its path)";
    assert_eq!(harness.query_all_by_label(why).count(), 1);

    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "G: backed up");
}

#[test]
fn a_registry_key_of_one_build_promises_a_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abc");
    // `make_prefix`'s key, which the Linux build has no counterpart of.
    let game = paired(
        "  registry:\n    HKEY_CURRENT_USER/Software/Alpha: {}\n",
        &prefix,
    );
    let (mut harness, _env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);

    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);

    let sentence = "No changes to the files since the backup from ";
    assert!(
        label_of(&harness, sentence).contains("the registry keys are not compared"),
        "{}",
        label_of(&harness, sentence)
    );
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "G: backed up");
}

#[test]
fn choose_folders_saves_the_choice_and_compares_again() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abc");
    let game = like_factorio(&prefix);
    let (mut harness, env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    let label = label_of(&harness, "1 new, 1 removed since the backup from ");
    assert!(label.ends_with("Choose folders… places them."), "{label}");

    click(&mut harness, "Choose folders…");
    assert!(shows(&harness, "Where should the saves of G go?"));
    replace_text(&mut harness, "Target 1", "not a template");
    click(&mut harness, "Use these folders");
    assert!(shows(&harness, "is not a placeholder template of this OS"));
    replace_text(&mut harness, "Target 1", "<winAppData>/G");
    click(&mut harness, "Use these folders");
    run_until(
        &mut harness,
        "the comparison through the chosen folder",
        |h| shows(h, "No changes since the backup from "),
    );
    run_settled(&mut harness);

    assert!(!shows(&harness, "Choose folders…"));
    let pair = Pair {
        linux: "<home>/G".into(),
        windows: "<winAppData>/G".into(),
    };
    let config_dir = env.config_dir.clone();
    run_until(&mut harness, "the choice to be saved", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| config.custom_games[0].pairs == [pair.clone()])
    });
}

#[test]
fn cancelling_choose_folders_saves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abc");
    let game = like_factorio(&prefix);
    let (mut harness, env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);
    let config_file = env.config_dir.join("config.yaml");
    let config = fs::read(&config_file).unwrap();
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);

    click(&mut harness, "Choose folders…");
    replace_text(&mut harness, "Target 1", "<winAppData>/G");
    click(&mut harness, "Cancel");
    run_settled(&mut harness);

    assert!(!shows(&harness, "Where should the saves of G go?"));
    assert!(shows(&harness, "Choose folders…"));
    assert_eq!(fs::read(&config_file).unwrap(), config, "nothing is saved");
}

/// The folder of the saves of the game `name` in `prefix`.
fn save_dir(prefix: &Path, name: &str) -> PathBuf {
    (prefix.join("drive_c/users/steamuser/AppData/Roaming")).join(name)
}

/// The tab of the game `Alpha` after a backup of its file `save.dat` holding `before` and a
/// scan that finds it holding `after`, with the file's folder open: the file is a changed row.
/// The scratch folders stay alive with the harness.
fn alpha_changed(
    before: &[u8],
    after: &[u8],
) -> (Harness<'static, App>, Env, [TempDir; 2], PathBuf) {
    let scratch = tempfile::tempdir().unwrap();
    // As long as a path below the temp folder on Windows, which once pushed Close out of sight.
    let long = "a folder whose name makes the path of the save wider than the details dialog";
    let prefix = make_prefix(&scratch.path().join(long));
    let save = save_dir(&prefix, "Alpha").join("save.dat");
    fs::create_dir_all(save.parent().unwrap()).unwrap();
    fs::write(&save, before).unwrap();
    let games = vec![custom_game("Alpha", false, &prefix)];
    let (mut harness, env, tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    fs::write(&save, after).unwrap();
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    // Its size reads in KiB for a long file.
    harness.get_by_label_contains("1 file, ").click();
    run_settled(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    (harness, env, [scratch, tmp], save)
}

/// Clicks the Details button of `save.dat` and waits until the dialog shows `text` and has
/// settled: a centered dialog moves while it grows, so a click needs its final place.
fn open_details(harness: &mut Harness<'static, App>, text: &str) {
    harness.get_by_label("Details of save.dat").click();
    run_until(harness, "the details", |h| shows(h, text));
    run_settled(harness);
}

#[test]
fn details_show_that_only_line_endings_differ() {
    let (mut harness, _env, _dirs, save) = alpha_changed(b"a\nb\n", b"a\r\nb\r\n");

    open_details(&mut harness, "Only the line endings differ.");

    assert!(shows(&harness, "2 Windows (CRLF), 0 Linux (LF)"));
    assert!(shows(&harness, "0 Windows (CRLF), 2 Linux (LF)"));
    assert!(shows(&harness, "Backup ("));
    assert!(shows_path(&harness, &save.display().to_string()));
    let local = "58055bdcc73787eb88c78d36f0b4939e9c5dc1c3ad17e25cc85a6833cf1a0cab";
    let backup = "911169ddaaf146aff539f58c26c489af3b892dff0fe283c1c264c65ae5aa59a2";
    assert!(shows(&harness, local));
    assert!(shows(&harness, backup));
    assert!(shows(&harness, "(from its hash list)"));
    assert!(!shows(&harness, "Reading the backup"));
}

#[test]
fn details_show_the_changed_lines() {
    let (mut harness, _env, _dirs, _save) = alpha_changed(b"one\ntwo\nthree\n", b"one\n2\nthree\n");

    open_details(&mut harness, "− two");

    assert!(shows(&harness, "+ 2"));
    assert!(shows(&harness, "  one"), "context lines are shown");
    assert!(!shows(&harness, "Only the line endings differ."));
    let backup = "b6285c57e8797db5d4c51c80d6f11938afda9b11c6a003549709189e9b4b92a2";
    let local = "bc85caa9b61bcf3a54ccfc800e2b0eda6c11fa5df4e0481c896d31cda2462eb2";
    assert!(shows(&harness, backup) && shows(&harness, local));
    // The backup keeps the file's path below `files/`, which is also its key here.
    assert!(shows(&harness, "WinAppData/Alpha/save.dat"));

    click(&mut harness, "Close details");
    run_settled(&mut harness);
    assert!(!shows(&harness, "Backup ("));
}

#[test]
fn a_long_diff_stops_at_a_thousand_lines_and_counts_the_rest() {
    let lines = |prefix: &str| {
        (0..600)
            .map(|n| format!("{prefix}{n}\n"))
            .collect::<String>()
    };
    let (mut harness, _env, _dirs, _save) =
        alpha_changed(lines("a").as_bytes(), lines("b").as_bytes());

    open_details(&mut harness, "200 more lines not shown.");

    // 600 removed lines, then added ones up to the 1000th line drawn.
    assert!(shows(&harness, "− a599"));
    assert!(shows(&harness, "+ b399"));
    assert!(!shows(&harness, "+ b400"));
    // The button is in sight below the diff, not after it.
    click(&mut harness, "Close details");
    run_settled(&mut harness);
    assert!(!shows(&harness, "more lines not shown."));
}

#[test]
fn a_file_over_the_cap_is_shown_on_request() {
    // 16 characters a line, a little over 10 MiB in all, with one line different.
    let before = "0123456789abcdef\n".repeat(620_000);
    let mut after = before.clone();
    after.replace_range(17 * 310_000..17 * 310_000 + 16, "changed_line_xyz");
    let (mut harness, _env, _dirs, _save) = alpha_changed(before.as_bytes(), after.as_bytes());

    open_details(&mut harness, "Show anyway");
    assert!(shows(&harness, "Not shown: a copy is over"));
    assert!(!shows(&harness, "+ changed_line_xyz"));

    click(&mut harness, "Show anyway");
    run_until(&mut harness, "the diff", |h| shows(h, "+ changed_line_xyz"));
    assert!(shows(&harness, "− 0123456789abcdef"));
    assert!(!shows(&harness, "Show anyway"));
    assert!(!shows(&harness, "Not shown: a copy is over"));
}

#[test]
fn a_missing_final_newline_shows_in_the_details() {
    let (mut harness, _env, _dirs, _save) = alpha_changed(b"a\nb\n", b"a\nb");

    open_details(&mut harness, "(no newline at end)");

    assert_eq!(
        harness
            .query_all_by_label_contains("(no newline at end)")
            .count(),
        1
    );
}

#[test]
fn details_say_a_binary_file_is_not_text() {
    let (mut harness, _env, _dirs, _save) = alpha_changed(b"\0\x01\x02", b"\0\x01\x02\x03");

    open_details(&mut harness, "Not shown: not text.");

    assert!(!shows(&harness, "Windows (CRLF)"));
    assert!(shows(&harness, "SHA-256 of This PC"));
}

#[test]
fn only_changed_files_have_a_details_button() {
    let scratch = tempfile::tempdir().unwrap();
    let prefix = make_prefix(scratch.path());
    let dir = save_dir(&prefix, "Alpha");
    fs::create_dir_all(&dir).unwrap();
    for name in ["same.dat", "changed.dat", "removed.dat"] {
        fs::write(dir.join(name), "abc").unwrap();
    }
    let games = vec![custom_game("Alpha", false, &prefix)];
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    fs::write(dir.join("changed.dat"), "abcd").unwrap();
    fs::write(dir.join("new.dat"), "n").unwrap();
    fs::remove_file(dir.join("removed.dat")).unwrap();
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    click(&mut harness, "3 files, 8 B");
    click(&mut harness, "Open WinAppData/Alpha");
    for row in [
        "new.dat, new",
        "changed.dat, changed",
        "removed.dat, removed",
    ] {
        assert_eq!(harness.query_all_by_label(row).count(), 1, "{row}");
    }
    // Not on the new file, the removed one and the unchanged one.
    assert_eq!(harness.query_all_by_label_contains("Details of").count(), 1);
    let button = harness.get_by_label("Details of changed.dat");
    assert_eq!(button.accesskit_node().role(), Role::Button);

    // Unticked, the changed file is a removed one.
    click(&mut harness, "changed.dat, changed");
    assert_eq!(harness.query_all_by_label_contains("Details of").count(), 0);
}

#[test]
fn details_show_a_read_error() {
    let (mut harness, _env, dirs, _save) = alpha_changed(b"abc\n", b"abcd\n");
    // The repository lost its data packs since the comparison.
    fs::remove_dir_all(Path::new(&repo_dir(&dirs[1])).join("data")).unwrap();

    open_details(&mut harness, "Could not read the backup");

    assert!(!shows(&harness, "Reading the backup"));
    assert!(!shows(&harness, "SHA-256 of"));
    click(&mut harness, "Close details");
    run_settled(&mut harness);
    assert!(!shows(&harness, "Could not read the backup"));
}

#[test]
fn details_of_the_other_build_name_its_path_in_the_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    back_up_linux_build(&tmp, "abc");
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "G", "abcd");
    let game = paired("  files: {}\n", &prefix);
    let (mut harness, _env) = open_app(&tmp, &repository, Vec::new(), vec![game], true);
    scan(&mut harness, "1 game found");
    run_until_compared(&mut harness);
    click(&mut harness, "1 file, 4 B");
    click(&mut harness, "Open WinAppData/G");

    harness.get_by_label("Details of save.dat").click();
    run_until(&mut harness, "the details", |h| shows(h, "Location"));

    // This PC's key, and the other build's own path in the backup.
    assert!(shows(&harness, "WinAppData/G/save.dat"));
    assert!(shows(&harness, "Home/G/save.dat"));
}

#[test]
fn the_games_list_keeps_its_scroll_when_another_tab_is_shown() {
    let scratch = tempfile::tempdir().unwrap();
    let prefix = make_prefix(scratch.path());
    let names: Vec<String> = (0..40).map(|n| format!("Game {n:02}")).collect();
    let games = names
        .iter()
        .map(|name| {
            write_save(&prefix, name, "abc");
            custom_game(name, false, &prefix)
        })
        .collect();
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "40 games found");

    let (first, last) = (&names[0], &names[39]);
    let top_of = |harness: &Harness<'static, App>, title: &str| {
        harness.get_by_label(title).accesskit_node().raw_bounds()
    };
    harness.get_by_label(last).scroll_to_me();
    run_settled(&mut harness);
    let scrolled = top_of(&harness, last).unwrap().y0;

    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    harness.state_mut().set_tab(Tab::Backup);
    run_settled(&mut harness);

    assert_eq!(top_of(&harness, last).unwrap().y0, scrolled);
    assert!(top_of(&harness, first).unwrap().y0 < 0.0);
}

/// The titles among `titles`, top to bottom as their checkboxes show.
fn shown_order<'a>(harness: &Harness<'static, App>, titles: &[&'a str]) -> Vec<&'a str> {
    let mut titles = titles.to_vec();
    let top = |title: &str| {
        let node = harness.get_by_role_and_label(Role::CheckBox, title);
        node.rect().min.y
    };
    titles.sort_by(|a, b| top(a).total_cmp(&top(b)));
    titles
}

/// Picks `key` in the Sort by dropdown.
fn sort_by(harness: &mut Harness<'static, App>, key: &str) {
    harness
        .get_by_role_and_label(Role::ComboBox, "Sort by")
        .click();
    run_settled(harness);
    harness.get_by_label(key).click();
    run_settled(harness);
}

/// Runs frames until `count` games show the space their backups use, worked out.
fn run_until_space_shown(harness: &mut Harness<'static, App>, count: usize) {
    run_until(harness, "the space used", |h| {
        h.query_all_by_label_contains("All backups use").count() == count
            && !shows(h, "All backups use …")
    });
    run_settled(harness);
}

#[test]
fn sort_by_space_used_puts_the_largest_first_and_the_unbacked_last() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    // Uncompressed, so the bigger save takes more space.
    init_repo_with(&repository, "", Compression::Off);
    let prefix = make_prefix(tmp.path());
    write_save(&prefix, "Alpha", "abc");
    write_save(&prefix, "Beta", &"x".repeat(64 * 1024));
    write_save(&prefix, "Gamma", "g");
    let games = ["Alpha", "Beta", "Gamma"].map(|name| custom_game(name, false, &prefix));
    let (mut harness, _env) = open_app(&tmp, &repository, Vec::new(), games.into(), true);
    scan(&mut harness, "3 games found");
    click(&mut harness, "Gamma");
    harness.get_by_label("Back up 2 games").click();
    run_until_backed_up(&mut harness, "Beta: backed up");
    run_until_space_shown(&mut harness, 2);
    let titles = ["Alpha", "Beta", "Gamma"];
    assert_eq!(shown_order(&harness, &titles), titles, "by name");

    sort_by(&mut harness, "Space used");
    assert_eq!(shown_order(&harness, &titles), ["Beta", "Alpha", "Gamma"]);
    // A checkbox acts on its own game, not on the game at its place in the scan.
    click(&mut harness, "Alpha");
    assert!(!checked(&harness, "Alpha"));
    assert!(checked(&harness, "Beta"));
    assert!(shows(&harness, "Back up 1 game"));

    harness.get_by_label("Largest first").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Smallest first"));
    assert_eq!(shown_order(&harness, &titles), ["Alpha", "Beta", "Gamma"]);
}

#[test]
fn sort_is_saved_and_restored() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    let titles = ["Alpha", "Beta"];
    assert_eq!(shown_order(&harness, &titles), titles);

    harness.get_by_label("A to Z").click();
    run_settled(&mut harness);
    assert_eq!(shown_order(&harness, &titles), ["Beta", "Alpha"]);
    // The config is written in the background, which asks for no frames while it runs.
    let config_dir = env.config_dir.clone();
    run_until(&mut harness, "the sort to be saved", move |_| {
        let saved = AppConfig::load(&config_dir).ok().flatten();
        saved.is_some_and(|saved| saved.backup_sort == ListSort::new(SortKey::Name, true))
    });
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.restore_sort, ListSort::default());
    drop(harness);

    let mut harness = common::harness(env);
    run_until_ready(&mut harness);
    run_settled(&mut harness);
    scan(&mut harness, "2 games found");
    assert!(shows(&harness, "Z to A"));
    assert_eq!(shown_order(&harness, &titles), ["Beta", "Alpha"]);
}

#[test]
fn details_and_folders_act_on_their_own_game_in_a_sorted_list() {
    let scratch = tempfile::tempdir().unwrap();
    let (prefix, games) = alpha_beta(&scratch);
    let (mut harness, opened, _tmp) = open_tab_recording(games);
    scan(&mut harness, "2 games found");
    harness.get_by_label("Back up 2 games").click();
    run_until_backed_up(&mut harness, "Beta: backed up");
    write_save(&prefix, "Beta", "changed");
    scan(&mut harness, "2 games found");
    run_until_compared(&mut harness);
    // Z to A puts Beta, the second game of the scan, first.
    click(&mut harness, "A to Z");
    assert_eq!(shown_order(&harness, &["Alpha", "Beta"]), ["Beta", "Alpha"]);

    click(&mut harness, "1 file, 7 B");
    click(&mut harness, "Show WinAppData/Beta in the file manager");
    let beta = prefix.join("drive_c/users/steamuser/AppData/Roaming/Beta");
    assert_eq!(*opened.lock().unwrap(), std::slice::from_ref(&beta));
    click(&mut harness, "Open WinAppData/Beta");
    harness.get_by_label("Details of save.dat").click();
    run_until(&mut harness, "the details", |h| shows(h, "Backup ("));
    assert!(shows_path(
        &harness,
        &beta.join("save.dat").display().to_string()
    ));
}

#[test]
fn game_line_shows_last_backup_and_space_used() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    run_until_compared(&mut harness);
    assert_eq!(harness.query_all_by_label("No backup.").count(), 2);

    click(&mut harness, "Beta");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    run_until_space_shown(&mut harness, 1);
    let line = harness.get_by_label_contains("All backups use");
    // A label's text is its value.
    let text = line.accesskit_node().value().unwrap_or_default();
    assert!(text.starts_with("Last backup 20"), "{text}");
    assert_eq!(harness.query_all_by_label("No backup.").count(), 1);
}

#[test]
fn deleting_backups_in_the_restore_tab_updates_the_backup_tab() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "Beta");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Alpha: backed up");
    run_until_space_shown(&mut harness, 1);

    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    click(&mut harness, "Load backups");
    run_until(&mut harness, "the list", |h| {
        h.query_by_label("Include Alpha").is_some()
    });
    harness.get_by_label("Alpha").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    click(&mut harness, "Delete all backups of Alpha");
    harness.get_by_label("Delete").click();
    run_until(&mut harness, "the delete", |h| shows(h, "Removed 1 backup"));
    harness.state_mut().set_tab(Tab::Backup);
    run_until(&mut harness, "the comparison again", |h| {
        h.query_all_by_label("No backup.").count() == 2
    });
}

/// The text of the one figures line that says how much space the backups use.
fn space_line(harness: &Harness<'static, App>) -> String {
    let line = harness.get_by_label_contains("All backups use");
    // A label's text is its value.
    line.accesskit_node().value().unwrap_or_default()
}

/// Backs up Beta alone, changes its save, and shows the Restore tab's list with the figures
/// line the Backup tab showed, whose space a restore's safety backup of the change will grow.
fn beta_changed_since_its_backup(scratch: &TempDir) -> (Harness<'static, App>, String, TempDir) {
    let (prefix, games) = alpha_beta(scratch);
    let (mut harness, _env, tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    click(&mut harness, "Alpha");
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(&mut harness, "Beta: backed up");
    run_until_space_shown(&mut harness, 1);
    let before = space_line(&harness);
    write_save(&prefix, "Beta", "changed since the backup");

    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    click(&mut harness, "Load backups");
    run_until(&mut harness, "the list", |h| {
        h.query_by_label("Include Beta").is_some()
    });
    (harness, before, tmp)
}

/// Shows the Backup tab and waits until its figures line differs from `before`, worked out.
fn run_until_space_changed(harness: &mut Harness<'static, App>, before: &str) {
    harness.state_mut().set_tab(Tab::Backup);
    run_until(harness, "the comparison again", |h| {
        let line = space_line(h);
        line != before && !line.contains('…')
    });
}

#[test]
fn a_restore_in_the_restore_tab_updates_the_backup_tab() {
    let scratch = tempfile::tempdir().unwrap();
    let (mut harness, before, _tmp) = beta_changed_since_its_backup(&scratch);
    harness.get_by_label("Beta").click();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Beta"));

    run_until_space_changed(&mut harness, &before);
}

#[test]
fn a_batch_restore_updates_the_backup_tab() {
    let scratch = tempfile::tempdir().unwrap();
    let (mut harness, before, _tmp) = beta_changed_since_its_backup(&scratch);
    click(&mut harness, "Include Beta");
    harness.get_by_label("Restore 1 game").click();
    run_until(&mut harness, "the batch confirm", |h| {
        h.query_by_label("Cancel").is_some()
    });
    harness.get_by_label("Restore 1 game").click();
    run_until(&mut harness, "the batch", |h| {
        shows(h, "Restored 1 of 1 game")
    });

    run_until_space_changed(&mut harness, &before);
}
