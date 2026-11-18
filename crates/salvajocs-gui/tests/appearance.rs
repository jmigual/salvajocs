//! Renders each screen of the window to a PNG for the README.
//!
//! Nothing here asserts, and it needs a wgpu adapter, so every test is `#[ignore]` and returns
//! at once unless `SALVAJOCS_SCREENSHOTS` is set: CI runs ignored tests too, without a GPU.
//! The images land in `docs/images/` and are committed:
//!
//! ```text
//! SALVAJOCS_SCREENSHOTS=1 cargo test -p salvajocs --features screenshots --test appearance -- --ignored
//! ```
//!
//! Everything shown is made up: the user and the home folder are set through the `Env` seam, the
//! hosts and times of the fixture snapshots by `forge`, and the saves are written into temp
//! directories. Linux only: the made-up machine and the Settings path are Unix paths.

#![cfg(unix)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{
    env, init_repo, local_config, replace_text, repo_dir, run_settled, run_until, run_until_ready,
    save_config, shows,
};
use eframe::egui::{Theme, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rustic_backend::BackendOptions;
use rustic_core::jiff::Timestamp;
use rustic_core::{
    BackupOptions, Credentials, PathList, Repository, RepositoryOptions, SnapshotOptions,
};
use salvajocs_core::backup::stage;
use salvajocs_core::layout::{Base, Context, LinuxFolders, Root, RootKind};
use salvajocs_core::manifest::{CustomGame, Integration, Manifest, Pair, parse_yaml, to_json};
use salvajocs_core::registry::RegistryExport;
use salvajocs_core::scan::{Environment, FoundFile, GameScan};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// The manifest entries of the games shown, as the ludusavi manifest has them for Windows.
const MANIFEST: &str = r#"
Cuphead:
  files:
    "<home>/AppData/LocalLow/Studio MDHR/Cuphead":
      tags: [save]
      when:
        - os: windows
    "<winAppData>/Cuphead":
      tags: [save]
      when:
        - os: windows
  steam:
    id: 268910
Hades:
  files:
    "<winDocuments>/Saved Games/Hades":
      tags: [config, save]
      when:
        - os: windows
  steam:
    id: 1145360
Hollow Knight:
  files:
    "<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat":
      tags: [save]
      when:
        - os: windows
    "<xdgConfig>/unity3d/Team Cherry/Hollow Knight/*.dat":
      tags: [save]
      when:
        - os: linux
  registry:
    HKEY_CURRENT_USER/Software/Team Cherry/Hollow Knight:
      tags: [config]
  steam:
    id: 367520
Stardew Valley:
  files:
    "<winAppData>/StardewValley/Saves":
      tags: [save]
      when:
        - os: windows
    "<xdgConfig>/StardewValley/Saves":
      tags: [save]
      when:
        - os: linux
  steam:
    id: 413150
Terraria:
  files:
    "<winDocuments>/My Games/Terraria":
      tags: [save]
      when:
        - os: windows
    "<xdgData>/Terraria/Players/*.plr":
      tags: [save]
      when:
        - os: linux
  steam:
    id: 105600
"#;

/// The window width and zoom of every image; each screen picks the height that fits it.
const WIDTH: f32 = 1100.0;
const ZOOM: f32 = 1.25;

/// The directory the images are written to, or `None` (with a note) when they are not asked for.
fn output_dir() -> Option<PathBuf> {
    if std::env::var_os("SALVAJOCS_SCREENSHOTS").is_none() {
        eprintln!("skipped: set SALVAJOCS_SCREENSHOTS=1 to render the README screenshots");
        return None;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images");
    fs::create_dir_all(&dir).expect("the image directory should be creatable");
    Some(dir)
}

/// A window rendered through wgpu, which `common::harness` is not.
fn shot_harness(env: Env, theme: Theme, height: f32) -> Harness<'static, App> {
    Harness::builder()
        .with_size(Vec2::new(WIDTH, height))
        .with_render_cursor(false)
        .with_pixels_per_point(ZOOM)
        .with_theme(theme)
        .wgpu()
        .build_ui_state(
            |ui, app| {
                let mut frame = eframe::Frame::_new_kittest();
                eframe::App::ui(app, ui, &mut frame);
            },
            App::new(env),
        )
}

/// Renders what the harness shows to `<dir>/<name>.png`.
fn capture(harness: &mut Harness<'static, App>, dir: &Path, name: &str) {
    run_settled(harness);
    let image = harness.render().expect("the frame should render");
    let path = dir.join(format!("{name}.png"));
    image
        .save(&path)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
    println!("wrote {}", path.display());
}

/// A Linux machine of the made-up user `deck`, so no scan or preview looks at the real one.
fn deck() -> Environment {
    let home = PathBuf::from("/home/deck");
    Environment {
        native: Some(Context::Linux(LinuxFolders {
            user: "deck".into(),
            xdg_data: home.join(".local/share"),
            xdg_config: home.join(".config"),
            home,
        })),
        steam_roots: Vec::new(),
    }
}

/// An environment in `tmp` on the made-up machine, with the manifest cached.
fn demo_env(tmp: &TempDir) -> Env {
    let mut env = env(tmp);
    env.environment = Some(deck());
    fs::create_dir_all(&env.cache_dir).unwrap();
    fs::write(
        env.cache_dir.join("manifest.json"),
        to_json(&parse_yaml(MANIFEST).unwrap()).unwrap(),
    )
    .unwrap();
    env
}

/// Bytes that stand in for a save file of `len` bytes.
fn filler(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Writes `len` bytes to `path`, creating its directory.
fn write_file(path: &Path, len: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, filler(len)).unwrap();
}

/// A Wine prefix under `tmp` that holds the saves of five games and the registry key of one.
fn windows_prefix(tmp: &TempDir) -> PathBuf {
    let prefix = tmp.path().join("pfx");
    let home = prefix.join("drive_c/users/steamuser");
    let saves = [
        (
            "AppData/LocalLow/Team Cherry/Hollow Knight/user1.dat",
            412_000,
        ),
        (
            "AppData/LocalLow/Team Cherry/Hollow Knight/user2.dat",
            396_000,
        ),
        (
            "AppData/LocalLow/Team Cherry/Hollow Knight/user3.dat",
            18_400,
        ),
        (
            "AppData/Roaming/StardewValley/Saves/Ines_372841905/SaveGameInfo",
            3_100,
        ),
        (
            "AppData/Roaming/StardewValley/Saves/Ines_372841905/Ines_372841905",
            1_240_000,
        ),
        ("Documents/Saved Games/Hades/Profile1.sav", 2_480_000),
        ("Documents/Saved Games/Hades/Profile2.sav", 2_310_000),
        ("Documents/My Games/Terraria/Players/Ines.plr", 96_500),
        (
            "Documents/My Games/Terraria/Worlds/Overgrown.wld",
            5_420_000,
        ),
        (
            "AppData/Roaming/Cuphead/cuphead_player_data_v1_slot_0.sav",
            14_800,
        ),
    ];
    for (tail, len) in saves {
        write_file(&home.join(tail), len);
    }
    fs::write(
        prefix.join("user.reg"),
        "WINE REGISTRY Version 2\n\n#arch=win64\n\n[Software\\\\Team Cherry\\\\Hollow Knight] 1700000000\n#time=1d9\n\"MasterVolume\"=dword:0000000a\n",
    )
    .unwrap();
    prefix
}

/// Where the repository of the screens that show its path lives: a fixed path that names no
/// user, the same in every image.
const DEMO_ROOT: &str = "/tmp/salvajocs-demo";

/// A new, empty repository at `DEMO_ROOT/backups`, held until the guard drops; the tests that
/// show the path take turns on it.
fn demo_repository() -> (MutexGuard<'static, ()>, String) {
    static TURN: Mutex<()> = Mutex::new(());
    let turn = TURN.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = fs::remove_dir_all(DEMO_ROOT);
    let repository = format!("{DEMO_ROOT}/backups");
    init_repo(&repository);
    (turn, repository)
}

/// The Backup tab after a scan and two backups: the first saved everything, then one game's
/// save changed, so the second run reports one game backed up and the rest up to date.
fn backup_tab(dir: &Path, theme: Theme, name: &str) {
    let (_turn, repository) = demo_repository();
    let tmp = tempfile::tempdir().unwrap();
    let env = demo_env(&tmp);
    let prefix = windows_prefix(&tmp);
    let mut config = local_config(&repository);
    config.roots = vec![Root {
        path: prefix.clone(),
        kind: RootKind::WinePrefix,
    }];
    save_config(&env, &config);
    let mut harness = shot_harness(env, theme, 750.0);
    run_until_ready(&mut harness);
    run_settled(&mut harness);

    harness.get_by_label("Scan for games").click();
    run_until(&mut harness, "the scan", |h| shows(h, "5 games found"));
    // What to back up may still be loading.
    run_settled(&mut harness);
    harness.get_by_label("Back up 5 games").click();
    run_until(&mut harness, "the first backup", |h| {
        shows(h, "Terraria: backed up")
    });

    let saves = prefix.join("drive_c/users/steamuser/AppData/Roaming/StardewValley/Saves");
    write_file(&saves.join("Ines_372841905/SaveGameInfo"), 3_300);
    harness.get_by_label("Terraria").click();
    run_settled(&mut harness);
    harness.get_by_label("Back up 4 games").click();
    run_until(&mut harness, "the second backup", |h| {
        shows(h, "Stardew Valley: backed up")
    });
    harness
        .get_by_label("3 files, 807.0 KiB, plus registry keys")
        .click();
    capture(&mut harness, dir, name);
    fs::remove_dir_all(DEMO_ROOT).unwrap();
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn backup_tab_dark() {
    let Some(dir) = output_dir() else { return };
    backup_tab(&dir, Theme::Dark, "backup");
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn backup_tab_light() {
    let Some(dir) = output_dir() else { return };
    backup_tab(&dir, Theme::Light, "hero-light");
}

/// A snapshot to put into the fixture repository.
struct Forged<'a> {
    game: &'a GameScan,
    host: &'a str,
    /// RFC 3339 time of the snapshot.
    time: &'a str,
    pre_restore: bool,
}

/// A scan of `title` holding `files` as `(base, tail, size)`, written below `tmp`.
fn scan(
    tmp: &TempDir,
    title: &str,
    files: &[(Base, &str, usize)],
    roots: &[(Base, &str)],
) -> GameScan {
    let files = files
        .iter()
        .map(|&(base, tail, size)| {
            let real = tmp.path().join("src").join(title).join(tail);
            write_file(&real, size);
            FoundFile {
                base,
                tail: tail.into(),
                size: size as u64,
                real,
            }
        })
        .collect();
    GameScan {
        title: title.into(),
        files,
        roots: roots
            .iter()
            .map(|&(base, rest)| (base, rest.into()))
            .collect(),
        registry_keys: Vec::new(),
        registry_source: None,
        unreadable: Vec::new(),
    }
}

/// Writes `snapshots` into the repository at `repository` with the host and time they name.
/// `Storage` stamps this machine's host and the current time, so rustic is used directly.
fn forge(tmp: &TempDir, repository: &str, snapshots: &[Forged]) {
    let backends = BackendOptions::default()
        .repository(repository)
        .to_backends()
        .unwrap();
    let repo = Repository::new(&RepositoryOptions::default(), &backends)
        .unwrap()
        .open(&Credentials::password(""))
        .unwrap()
        .to_indexed_ids()
        .unwrap();
    for (index, forged) in snapshots.iter().enumerate() {
        let slug = salvajocs_core::slug::slug(&forged.game.title);
        let staged = tmp.path().join("forge").join(index.to_string());
        stage(forged.game, &RegistryExport::default(), &staged).unwrap();
        let mut tags = format!("salvajocs,game:{slug}");
        if forged.pre_restore {
            tags += ",pre-restore";
        }
        let time = forged
            .time
            .parse::<Timestamp>()
            .unwrap()
            .in_tz("UTC")
            .unwrap();
        let snapshot = SnapshotOptions::default()
            .label(forged.game.title.clone())
            .add_tags(&tags)
            .unwrap()
            .host(forged.host.to_string())
            .time(time)
            .to_snapshot()
            .unwrap();
        let options = BackupOptions::default().as_path(PathBuf::from(format!("/salvajocs/{slug}")));
        let source = PathList::from_string(staged.to_str().unwrap())
            .unwrap()
            .sanitize()
            .unwrap();
        repo.backup(&options, &source, snapshot).unwrap();
    }
}

/// Hollow Knight as the Windows build saves it and as the Linux build does.
fn hollow_knight(tmp: &TempDir) -> (GameScan, GameScan) {
    let windows = scan(
        tmp,
        "Hollow Knight",
        &[
            (
                Base::WinLocalAppDataLow,
                "Team Cherry/Hollow Knight/user1.dat",
                412_000,
            ),
            (
                Base::WinLocalAppDataLow,
                "Team Cherry/Hollow Knight/user2.dat",
                396_000,
            ),
            (
                Base::WinLocalAppDataLow,
                "Team Cherry/Hollow Knight/user3.dat",
                18_400,
            ),
        ],
        &[(Base::WinLocalAppDataLow, "Team Cherry/Hollow Knight")],
    );
    let linux = scan(
        tmp,
        "Hollow Knight",
        &[
            (
                Base::XdgConfig,
                "unity3d/Team Cherry/Hollow Knight/user1.dat",
                405_000,
            ),
            (
                Base::XdgConfig,
                "unity3d/Team Cherry/Hollow Knight/user2.dat",
                391_000,
            ),
        ],
        &[(Base::XdgConfig, "unity3d/Team Cherry/Hollow Knight")],
    );
    (windows, linux)
}

/// An app on the made-up Linux machine with `snapshots` in its repository, the Restore tab in
/// front and the list loaded.
fn restore_app(
    tmp: &TempDir,
    snapshots: &[Forged],
    first: &str,
    height: f32,
) -> Harness<'static, App> {
    let env = demo_env(tmp);
    let repository = repo_dir(tmp);
    init_repo(&repository);
    forge(tmp, &repository, snapshots);
    save_config(&env, &local_config(&repository));
    let mut harness = shot_harness(env, Theme::Dark, height);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    harness.get_by_label("Load backups").click();
    run_until(&mut harness, "the list", |h| shows(h, first));
    harness
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn restore_tab() {
    let Some(dir) = output_dir() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (windows, linux) = hollow_knight(&tmp);
    let hades = scan(
        &tmp,
        "Hades",
        &[(
            Base::WinDocuments,
            "Saved Games/Hades/Profile1.sav",
            2_480_000,
        )],
        &[(Base::WinDocuments, "Saved Games/Hades")],
    );
    let stardew = scan(
        &tmp,
        "Stardew Valley",
        &[(
            Base::WinAppData,
            "StardewValley/Saves/Ines_372841905/SaveGameInfo",
            3_300,
        )],
        &[(Base::WinAppData, "StardewValley/Saves")],
    );
    let terraria = scan(
        &tmp,
        "Terraria",
        &[(
            Base::WinDocuments,
            "My Games/Terraria/Players/Ines.plr",
            96_500,
        )],
        &[(Base::WinDocuments, "My Games/Terraria")],
    );
    let snapshots = [
        Forged {
            game: &stardew,
            host: "desktop-win",
            time: "2026-11-14T10:20:00Z",
            pre_restore: false,
        },
        Forged {
            game: &terraria,
            host: "desktop-win",
            time: "2026-11-10T17:45:00Z",
            pre_restore: false,
        },
        Forged {
            game: &hades,
            host: "desktop-win",
            time: "2026-11-09T19:02:00Z",
            pre_restore: false,
        },
        Forged {
            game: &linux,
            host: "steamdeck",
            time: "2026-11-08T21:15:00Z",
            pre_restore: false,
        },
        Forged {
            game: &linux,
            host: "steamdeck",
            time: "2026-11-12T08:30:00Z",
            pre_restore: true,
        },
        Forged {
            game: &windows,
            host: "desktop-win",
            time: "2026-11-15T22:10:00Z",
            pre_restore: false,
        },
        Forged {
            game: &linux,
            host: "steamdeck",
            time: "2026-11-13T18:44:00Z",
            pre_restore: false,
        },
    ];
    let mut harness = restore_app(&tmp, &snapshots, "Hades", 760.0);

    harness.get_by_label("Hollow Knight").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    harness.get_by_label("Preview").click();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));
    // The picker open shows the snapshots of every host and the safety backup.
    harness.get_by_label("Snapshot").click();
    capture(&mut harness, &dir, "restore");
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn mapping_dialog() {
    let Some(dir) = output_dir() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let cuphead = scan(
        &tmp,
        "Cuphead",
        &[
            (
                Base::WinLocalAppDataLow,
                "Studio MDHR/Cuphead/cuphead_player_data_v1_slot_0.sav",
                14_800,
            ),
            (Base::WinAppData, "Cuphead/settings.ini", 900),
        ],
        &[
            (Base::WinLocalAppDataLow, "Studio MDHR/Cuphead"),
            (Base::WinAppData, "Cuphead"),
        ],
    );
    let snapshots = [Forged {
        game: &cuphead,
        host: "desktop-win",
        time: "2026-11-14T20:31:00Z",
        pre_restore: false,
    }];
    let mut harness = restore_app(&tmp, &snapshots, "Cuphead", 500.0);

    harness.get_by_label("Cuphead").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    harness.get_by_label("Preview").click();
    run_until(&mut harness, "the dialog", |h| {
        shows(h, "Where should the saves")
    });
    capture(&mut harness, &dir, "mapping-dialog");
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn custom_games_tab() {
    let Some(dir) = output_dir() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let env = demo_env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let pair = |linux: &str, windows: &str| Pair {
        linux: linux.into(),
        windows: windows.into(),
    };
    let game = |name: &str, yaml: &str, prefix: Option<&str>, pairs: Vec<Pair>, skips: &[&str]| {
        CustomGame {
            name: name.into(),
            files: Vec::new(),
            entry: parse_yaml(yaml).unwrap().games.remove(name).unwrap(),
            integration: Integration::Extend,
            wine_prefix: prefix.map(PathBuf::from),
            pairs,
            skips: skips.iter().map(|skip| skip.to_string()).collect(),
        }
    };
    let mut config = local_config(&repository);
    config.custom_games = vec![
        game(
            "Stardew Valley",
            "Stardew Valley:\n  files:\n    <winAppData>/StardewValley/Saves: {tags: [save]}\n",
            Some("/home/deck/Games/prefixes/stardew"),
            vec![
                pair(
                    "<xdgConfig>/StardewValley/Saves",
                    "<winAppData>/StardewValley/Saves",
                ),
                pair("<xdgConfig>/StardewValley", "<winAppData>/StardewValley"),
            ],
            &[
                "<winAppData>/StardewValley/ErrorLogs",
                "<xdgConfig>/StardewValley/Screenshots",
            ],
        ),
        game(
            "Hollow Knight",
            "Hollow Knight:\n  files:\n    <winLocalAppDataLow>/Team Cherry/Hollow Knight/Mods: {tags: [save]}\n",
            Some("/home/deck/.local/share/Steam/steamapps/compatdata/367520/pfx"),
            vec![pair(
                "<xdgConfig>/unity3d/Team Cherry/Hollow Knight",
                "<winLocalAppDataLow>/Team Cherry/Hollow Knight",
            )],
            &[],
        ),
        game(
            "Spiritfarer",
            "Spiritfarer:\n  files:\n    <home>/.config/Spiritfarer/Saves: {tags: [save]}\n",
            None,
            Vec::new(),
            &[],
        ),
    ];
    save_config(&env, &config);
    let mut harness = shot_harness(env, Theme::Dark, 900.0);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    harness.query_all_by_label("Edit").next().unwrap().click();
    run_settled(&mut harness);
    capture(&mut harness, &dir, "custom-games");
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn settings_tab() {
    let Some(dir) = output_dir() else { return };
    let (_turn, repository) = demo_repository();
    let tmp = tempfile::tempdir().unwrap();
    let env = demo_env(&tmp);
    let mut config = local_config(&repository);
    config.roots = vec![
        Root {
            path: "/home/deck/.local/share/Steam".into(),
            kind: RootKind::Steam,
        },
        Root {
            path: "/home/deck/Games/prefixes/cuphead".into(),
            kind: RootKind::WinePrefix,
        },
    ];
    save_config(&env, &config);
    let mut harness = shot_harness(env, Theme::Dark, 810.0);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Settings);
    capture(&mut harness, &dir, "settings");
    fs::remove_dir_all(DEMO_ROOT).unwrap();
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn first_run_dialog() {
    let Some(dir) = output_dir() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = shot_harness(demo_env(&tmp), Theme::Dark, 600.0);
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });
    replace_text(&mut harness, "Location", "/home/deck/Backups/salvajocs");
    capture(&mut harness, &dir, "first-run");
}

#[test]
fn manifest_fixture_parses() {
    let manifest: Manifest = parse_yaml(MANIFEST).unwrap();
    assert_eq!(manifest.games.len(), 5);
    to_json(&manifest).unwrap();
}
