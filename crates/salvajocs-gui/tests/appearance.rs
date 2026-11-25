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
use salvajocs_core::layout::{Base, Context, Hashes, LinuxFolders, Root, RootKind};
use salvajocs_core::manifest::{
    CustomFile, CustomGame, GameEntry, Integration, Manifest, Pair, parse_yaml, to_json,
};
use salvajocs_core::registry::RegistryExport;
use salvajocs_core::scan::{Environment, FoundFile, GameScan};
use salvajocs_core::sync;
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
    let mut environment = Environment::default();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "deck".into(),
        home.clone(),
        home.join(".local/share"),
        home.join(".config"),
    )));
    environment
}

/// An environment in `tmp` on the made-up machine, with the manifest cached.
fn demo_env(tmp: &TempDir) -> Env {
    let mut env = env(tmp);
    env.environment = Some(deck());
    fs::create_dir_all(&env.cache_dir).unwrap();
    fs::write(
        env.cache_dir
            .join(salvajocs_core::manifest::fetch::JSON_FILE),
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

/// A Wine prefix under `DEMO_ROOT`, a fixed path the screens can show, that holds the saves of
/// five games and the registry key of one. The caller owns `DEMO_ROOT`.
fn windows_prefix() -> PathBuf {
    let prefix = Path::new(DEMO_ROOT).join("Games/Heroic/Prefixes/default/pfx");
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

/// The Backup tab after a scan and a comparison with a backup of every game from another
/// machine, which this PC has synced: one save changed since, so its game is ahead with a
/// `changed` file, and Hades is behind, as the other machine has newer saves. `text`, the
/// contents of Hollow Knight's `settings.dat` before and after the backup, adds a changed text
/// file at fixed times. The comparison has to count `changed` files. The temp folder and the demo
/// repository's turn stay alive with the harness.
fn compared_backup_tab(
    theme: Theme,
    height: f32,
    changed: usize,
    text: Option<(&str, &str)>,
) -> (Harness<'static, App>, TempDir, MutexGuard<'static, ()>) {
    let (turn, repository) = demo_repository();
    let tmp = tempfile::tempdir().unwrap();
    let env = demo_env(&tmp);
    let prefix = windows_prefix();
    let knight = prefix.join("drive_c/users/steamuser/AppData/LocalLow/Team Cherry/Hollow Knight");
    let settings = knight.join("settings.dat");
    if let Some((before, _)) = text {
        fs::write(&settings, before).unwrap();
        set_modified(&settings, 1_794_301_950);
    }
    let mut config = local_config(&repository);
    config.roots = vec![Root::new(prefix.clone(), RootKind::WinePrefix)];
    save_config(&env, &config);
    // Forged, so the comparison names a made-up host and time rather than this machine's.
    let manifest = parse_yaml(MANIFEST).unwrap();
    let report = salvajocs_core::scan::scan(&manifest, &config, &deck(), |_, _| {});
    // Hades is the game another machine has backed up since this one synced: the base is an
    // older backup of the saves as they are here, the newest has a changed save.
    let newer_hades = scan(
        &tmp,
        "Hades",
        &[
            (
                Base::WinDocuments,
                "Saved Games/Hades/Profile1.sav",
                2_480_000,
            ),
            (
                Base::WinDocuments,
                "Saved Games/Hades/Profile2.sav",
                2_330_000,
            ),
        ],
        &[(Base::WinDocuments, "Saved Games/Hades")],
    );
    let mut snapshots: Vec<Forged> = (report.games.iter())
        .map(|game| Forged {
            game: if game.title == "Hades" {
                &newer_hades
            } else {
                game
            },
            host: "steamdeck",
            time: "2026-11-14T21:30:00Z",
            pre_restore: false,
        })
        .collect();
    let hades = (report.games.iter())
        .find(|game| game.title == "Hades")
        .unwrap();
    snapshots.push(Forged {
        game: hades,
        host: "desktop-win",
        time: "2026-11-12T09:00:00Z",
        pre_restore: false,
    });
    let stored = forge(&tmp, &repository, &snapshots);
    // This PC synced each game with its newest backup, but Hades with the older one.
    let (older, games) = stored.split_last().unwrap();
    let updates = report.games.iter().zip(games).map(|(game, (id, _))| {
        let slug = salvajocs_core::slug::slug(&game.title);
        let base = if game.title == "Hades" {
            sync::Base::new(older.0.clone(), Some(older.1.clone()))
        } else {
            sync::Base::new(id.clone(), None)
        };
        (slug, base)
    });
    sync::record(&env.config_dir, &config.secret_scope(), updates).unwrap();
    write_file(&knight.join("user3.dat"), 18_600);
    if let Some((_, after)) = text {
        fs::write(&settings, after).unwrap();
        set_modified(&settings, 1_794_854_405);
    }
    let mut harness = shot_harness(env, theme, height);
    run_until_ready(&mut harness);
    run_settled(&mut harness);

    harness.get_by_label("Scan for games").click();
    run_until(&mut harness, "the scan", |h| shows(h, "5 games found"));
    run_until(&mut harness, "the comparison", |h| {
        shows(
            h,
            &format!("{changed} changed since the backup from steamdeck"),
        ) && shows(h, "steamdeck has newer saves")
    });
    (harness, tmp, turn)
}

/// Sets the modification time of `path` to `secs` seconds after the Unix epoch.
fn set_modified(path: &Path, secs: u64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

/// The Backup tab with the changed save of Hollow Knight open in its folder.
fn backup_tab(dir: &Path, theme: Theme, name: &str) {
    let (mut harness, _tmp, _turn) = compared_backup_tab(theme, 750.0, 1, None);
    harness.get_by_label("Terraria").click();
    harness
        .get_by_label("3 files, 807.2 KiB, plus registry keys")
        .click();
    run_settled(&mut harness);
    harness
        .get_by_label("Open WinLocalAppDataLow/Team Cherry/Hollow Knight")
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

/// The Details dialog of a changed text save of Hollow Knight on the Backup tab.
#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn details() {
    let Some(dir) = output_dir() else { return };
    let before = "volume=0.8\nlanguage=en\nfullscreen=false\nautosave=true\nslot=2\n";
    let after = "volume=0.6\nlanguage=en\nfullscreen=true\nautosave=true\nslot=2\n";
    let (mut harness, _tmp, _turn) =
        compared_backup_tab(Theme::Dark, 900.0, 2, Some((before, after)));
    harness.get_by_label("Terraria").click();
    harness.get_by_label_contains("4 files, ").click();
    run_settled(&mut harness);
    harness
        .get_by_label("Open WinLocalAppDataLow/Team Cherry/Hollow Knight")
        .click();
    run_settled(&mut harness);
    harness.get_by_label("Details of settings.dat").click();
    run_until(&mut harness, "the details", |h| shows(h, "Location"));
    capture(&mut harness, &dir, "details");
    fs::remove_dir_all(DEMO_ROOT).unwrap();
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
            let modified = fs::metadata(&real).unwrap().modified().ok();
            FoundFile::new(base, tail.into(), real, size as u64, modified)
        })
        .collect();
    let mut scan = GameScan::default();
    scan.title = title.into();
    scan.files = files;
    scan.roots = roots
        .iter()
        .map(|&(base, rest)| (base, rest.into()))
        .collect();
    scan
}

/// Writes `snapshots` into the repository at `repository` with the host and time they name, and
/// returns each one's id and staged hash list. `Storage` stamps this machine's host and the
/// current time, so rustic is used directly.
fn forge(tmp: &TempDir, repository: &str, snapshots: &[Forged]) -> Vec<(String, Hashes)> {
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
    let mut forged_ids = Vec::new();
    for (index, forged) in snapshots.iter().enumerate() {
        let slug = salvajocs_core::slug::slug(&forged.game.title);
        let staged = tmp.path().join("forge").join(index.to_string());
        let hashes = stage(forged.game, &RegistryExport::default(), &staged).unwrap();
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
        let stored = repo.backup(&options, &source, snapshot).unwrap();
        forged_ids.push((stored.id.to_hex().to_string(), hashes));
    }
    forged_ids
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

/// An app in `env` with `snapshots` in its repository, the Restore tab in front and the list
/// loaded.
fn restore_app(
    env: Env,
    tmp: &TempDir,
    snapshots: &[Forged],
    first: &str,
    height: f32,
) -> Harness<'static, App> {
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
    let mut harness = restore_app(demo_env(&tmp), &tmp, &snapshots, "Hades", 880.0);

    harness.get_by_label("Hollow Knight").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));
    // The plan's folder open shows its files, each with its Override button.
    harness
        .get_by_label("Open XdgConfig/unity3d/Team Cherry/Hollow Knight")
        .click();
    run_settled(&mut harness);
    // The picker open shows the snapshots of every host and the safety backup.
    harness.get_by_label("Snapshot").click();
    capture(&mut harness, &dir, "restore");
}

#[test]
#[ignore = "renders the README screenshots; needs a wgpu adapter and SALVAJOCS_SCREENSHOTS=1"]
fn restore_batch() {
    let Some(dir) = output_dir() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (windows, _) = hollow_knight(&tmp);
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
    let snapshots = [
        Forged {
            game: &stardew,
            host: "desktop-win",
            time: "2026-11-14T10:20:00Z",
            pre_restore: false,
        },
        Forged {
            game: &windows,
            host: "desktop-win",
            time: "2026-11-15T22:10:00Z",
            pre_restore: false,
        },
    ];
    // A home on disk, so Stardew's save is already there and differs; no path of it is shown.
    let home = tmp.path().join("home/deck");
    write_file(
        &home.join(".config/StardewValley/Saves/Ines_372841905/SaveGameInfo"),
        3_100,
    );
    let mut env = demo_env(&tmp);
    let mut environment = deck();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "deck".into(),
        home.clone(),
        home.join(".local/share"),
        home.join(".config"),
    )));
    env.environment = Some(environment);
    let mut harness = restore_app(env, &tmp, &snapshots, "Stardew Valley", 830.0);

    // The confirm of a batch: each ticked game's files as a tree, labelled by what the restore
    // does to them.
    harness.get_by_label("Select all").click();
    run_settled(&mut harness);
    harness.get_by_label("Restore 2 games").click();
    run_until(&mut harness, "the batch confirm", |h| {
        h.query_by_label("Cancel").is_some()
    });
    harness
        .get_by_label("Open XdgConfig/unity3d/Team Cherry/Hollow Knight")
        .click();
    // Opening the first tree moves the second down.
    run_settled(&mut harness);
    harness
        .get_by_label("Open XdgConfig/StardewValley/Saves/Ines_372841905")
        .click();
    capture(&mut harness, &dir, "restore-batch");
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
    let mut harness = restore_app(demo_env(&tmp), &tmp, &snapshots, "Cuphead", 500.0);

    harness.get_by_label("Cuphead").click();
    run_until(&mut harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
    run_until(&mut harness, "the plan", |h| {
        h.query_by_label("Choose folders…").is_some()
    });
    harness.get_by_label("Choose folders…").click();
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
    let row = |linux: Option<&str>, windows: Option<&str>| CustomFile {
        linux: linux.map(Into::into),
        windows: windows.map(Into::into),
    };
    let game = |name: &str,
                files: Vec<CustomFile>,
                prefix: Option<&str>,
                pairs: Vec<Pair>,
                skips: &[&str]| {
        let mut game = CustomGame::new(name.into(), GameEntry::default(), Integration::Extend);
        game.files = files;
        game.wine_prefix = prefix.map(PathBuf::from);
        game.pairs = pairs;
        game.skips = skips.iter().map(|skip| skip.to_string()).collect();
        game
    };
    let mut config = local_config(&repository);
    config.custom_games = vec![
        game(
            "Stardew Valley",
            vec![row(
                Some("<xdgConfig>/StardewValley/Saves"),
                Some("<winAppData>/StardewValley/Saves"),
            )],
            Some("/home/deck/Games/prefixes/stardew"),
            vec![pair(
                "<xdgConfig>/StardewValley",
                "<winAppData>/StardewValley",
            )],
            &[
                "<winAppData>/StardewValley/ErrorLogs",
                "<xdgConfig>/StardewValley/Screenshots",
            ],
        ),
        game(
            "Hollow Knight",
            vec![row(
                None,
                Some("<winLocalAppDataLow>/Team Cherry/Hollow Knight/Mods"),
            )],
            Some("/home/deck/.local/share/Steam/steamapps/compatdata/367520/pfx"),
            Vec::new(),
            &[],
        ),
        game(
            "Spiritfarer",
            vec![row(Some("<xdgConfig>/Spiritfarer/Saves"), None)],
            None,
            Vec::new(),
            &[],
        ),
    ];
    save_config(&env, &config);
    let mut harness = shot_harness(env, Theme::Dark, 1450.0);
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
        Root::new("/home/deck/.local/share/Steam".into(), RootKind::Steam),
        Root::new(
            "/home/deck/Games/prefixes/cuphead".into(),
            RootKind::WinePrefix,
        ),
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
