//! Headless checks of the Backup tab.

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use common::{
    env, harness, init_repo, local_config, repo_dir, run_settled, run_until, run_until_ready,
    save_config, shows,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use salvajocs_core::layout::{Base, Root, RootKind};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::manifest::{self, CustomGame, Integration, Manifest, parse_yaml};
use salvajocs_core::slug::slug;
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
    let mut env = env(&tmp);
    if cached_manifest {
        fs::create_dir_all(&env.cache_dir).unwrap();
        fs::write(
            env.cache_dir.join("manifest.json"),
            manifest::to_json(&Manifest::default()).unwrap(),
        )
        .unwrap();
    } else {
        env.refresh_manifest = unreachable;
    }
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.roots = roots;
    config.custom_games = games;
    save_config(&env, &config);
    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    run_settled(&mut harness);
    (harness, env, tmp)
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
    assert!(shows(&harness, "1 files, 3 B, registry: yes"));
    assert!(shows(&harness, "1 files, 5 B, registry: no"));
    assert!(shows(&harness, "Back up selected (2)"));
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

    harness.get_by_label("Beta").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Back up selected (1)"));
    harness.get_by_label("Back up selected (1)").click();
    run_until(&mut harness, "the backup to finish", |h| {
        shows(h, "Alpha: new snapshot")
    });
    assert!(!shows(&harness, "Beta:"), "a deselected game is left out");
    assert_eq!(
        fs::read_dir(env.cache_dir.join("stage")).unwrap().count(),
        1
    );

    harness.get_by_label("Back up selected (1)").click();
    run_until(&mut harness, "the second backup to finish", |h| {
        shows(h, "Alpha: unchanged")
    });
    assert!(!shows(&harness, "new snapshot"));
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

    harness.get_by_label("Back up selected (2)").click();
    run_until(&mut harness, "the backup to finish", |h| {
        shows(h, "Beta: new snapshot")
    });

    assert!(shows(&harness, "Alpha: failed:"));
}

#[test]
fn backup_selection_survives_a_rescan() {
    let scratch = tempfile::tempdir().unwrap();
    let (_, games) = alpha_beta(&scratch);
    let (mut harness, _env, _tmp) = open_tab(Vec::new(), games);
    scan(&mut harness, "2 games found");
    harness.get_by_label("Beta").click();
    run_settled(&mut harness);
    assert!(shows(&harness, "Back up selected (1)"));

    harness.get_by_label("Scan for games").click();
    harness.step();
    run_until(&mut harness, "the rescan to finish", |h| {
        !shows(h, "Scanning")
    });

    assert!(
        shows(&harness, "Back up selected (1)"),
        "Beta stays deselected"
    );
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

    harness.get_by_label("Back up selected (2)").click();
    run_until(&mut harness, "the list to follow the backup", |h| {
        shows(h, "2 files, 5 B, registry: yes")
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
