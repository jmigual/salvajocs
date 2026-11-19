//! Headless checks of the Restore tab and its mapping dialog.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{
    env, harness, init_repo, local_config, record_opened, replace_text, repo_dir, run_settled,
    run_until, run_until_ready, save_config, shows,
};
use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable, by};
use salvajocs_core::backup::backup_games;
use salvajocs_core::config::AppConfig;
use salvajocs_core::layout::{Base, Context, LinuxFolders};
use salvajocs_core::manifest::{
    self, CustomGame, Integration, Manifest, Pair, RegistryRule, parse_yaml,
};
use salvajocs_core::scan::{Environment, FoundFile, GameScan, RegistrySource};
use salvajocs_core::selection::{self, Change};
use salvajocs_core::storage::{RepoLocation, SnapshotInfo, Storage};
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};
use tempfile::TempDir;

/// `rel`'s `/`-separated components below `base`, so a path shows as the restore builds it on
/// every OS.
fn under(base: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// The Linux home of the tests' machine.
fn home(tmp: &TempDir) -> PathBuf {
    tmp.path().join("home")
}

/// A Wine prefix with a `steamuser` home.
fn make_prefix(tmp: &TempDir) -> PathBuf {
    let prefix = tmp.path().join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    prefix
}

/// A custom game that replaces the manifest entry with `files`, each save-tagged.
fn custom(name: &str, files: &[&str], wine_prefix: Option<&Path>, pairs: Vec<Pair>) -> CustomGame {
    let mut yaml = format!("{name}:\n  files:\n");
    for file in files {
        yaml += &format!("    {file}: {{tags: [save]}}\n");
    }
    CustomGame {
        name: name.into(),
        files: Vec::new(),
        entry: parse_yaml(&yaml).unwrap().games.remove(name).unwrap(),
        integration: Integration::Override,
        wine_prefix: wine_prefix.map(Path::to_path_buf),
        pairs,
        skips: Vec::new(),
    }
}

/// A scan of `title` with `files` as `(base, tail)` and `roots`; each file holds its tail.
fn scan(tmp: &TempDir, title: &str, files: &[(Base, &str)], roots: &[(Base, &str)]) -> GameScan {
    let files = files
        .iter()
        .map(|&(base, tail)| {
            let real = tmp.path().join("src").join(title).join(tail);
            fs::create_dir_all(real.parent().unwrap()).unwrap();
            fs::write(&real, tail).unwrap();
            FoundFile {
                base,
                tail: tail.into(),
                size: tail.len() as u64,
                modified: fs::metadata(&real).unwrap().modified().ok(),
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
        prefixes: Vec::new(),
        unreadable: Vec::new(),
    }
}

/// An app on a Linux machine whose home is in `tmp`, with `snapshots` backed up, `games` in
/// the config and the Restore tab showing the list.
fn open_tab(
    tmp: &TempDir,
    games: Vec<CustomGame>,
    snapshots: &[GameScan],
) -> (Harness<'static, App>, Env) {
    open_tab_tagged(tmp, games, snapshots, &[])
}

/// Like [`open_tab`], with `tags` added to the snapshots.
fn open_tab_tagged(
    tmp: &TempDir,
    games: Vec<CustomGame>,
    snapshots: &[GameScan],
    tags: &[&str],
) -> (Harness<'static, App>, Env) {
    open_tab_with(tmp, games, snapshots, tags, |_| {})
}

/// Like [`open_tab_tagged`], with `adjust` applied to the app's environment.
fn open_tab_with(
    tmp: &TempDir,
    games: Vec<CustomGame>,
    snapshots: &[GameScan],
    tags: &[&str],
    adjust: impl FnOnce(&mut Env),
) -> (Harness<'static, App>, Env) {
    let mut env = env(tmp);
    env.environment = Some(Environment {
        native: Some(Context::Linux(LinuxFolders {
            user: "me".into(),
            home: home(tmp),
            xdg_data: under(&home(tmp), ".local/share"),
            xdg_config: home(tmp).join(".config"),
        })),
        steam_roots: Vec::new(),
        launcher_prefixes: Vec::new(),
        launcher_problems: Vec::new(),
    });
    adjust(&mut env);
    fs::create_dir_all(&env.cache_dir).unwrap();
    fs::write(
        env.cache_dir
            .join(salvajocs_core::manifest::fetch::JSON_FILE),
        manifest::to_json(&Manifest::default()).unwrap(),
    )
    .unwrap();
    let repository = repo_dir(tmp);
    init_repo(&repository);
    let location = RepoLocation {
        repository: repository.clone(),
        options: Default::default(),
    };
    let mut storage = Storage::open(&location, "").unwrap();
    backup_games(
        &mut storage,
        snapshots,
        &tmp.path().join("fixture-stage"),
        tags,
        false,
        |_| {},
    )
    .unwrap();
    drop(storage);
    let mut config = local_config(&repository);
    config.custom_games = games;
    save_config(&env, &config);
    let mut harness = harness(env.clone());
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    harness.get_by_label("Load backups").click();
    let first = snapshots[0].title.clone();
    run_until(&mut harness, "the list", move |h| shows(h, &first));
    (harness, env)
}

/// Selects `title` and waits for its default targets.
fn select(harness: &mut Harness<'static, App>, title: &str) {
    harness.get_by_label(title).click();
    run_until(harness, "the default targets", |h| {
        shows(h, "Restore into:")
    });
}

/// Previews the restore and waits for the plan or the mapping dialog.
fn preview(harness: &mut Harness<'static, App>) {
    harness.get_by_label("Preview").click();
    run_until(harness, "the preview", |h| {
        shows(h, "Files to write") || shows(h, "Where should the saves")
    });
}

/// Like `shows`, but `\` counts as `/`: on Windows a path joined from `/`-separated parts
/// shows both separators.
fn shows_path(harness: &Harness<'static, App>, text: &str) -> bool {
    let want = text.replace('\\', "/");
    let shown = |node: &AccessKitNode<'_>| {
        let label = match node.role() {
            Role::Label => node.value(),
            _ => node.label(),
        };
        label.is_some_and(|label| label.replace('\\', "/").contains(&want))
    };
    harness.query_all(by().predicate(shown)).next().is_some()
}

/// The text of the label right after the label `source`, in screen order: in a plan row, where
/// that file goes. `\` reads as `/`, as in `shows_path`.
fn target_of(harness: &Harness<'static, App>, source: &str) -> Option<String> {
    let labels: Vec<String> = harness
        .query_all(by().role(Role::Label))
        .filter_map(|node| node.accesskit_node().value())
        .map(|text| text.replace('\\', "/"))
        .collect();
    let row = labels.iter().position(|text| text == source)?;
    labels.get(row + 1).cloned()
}

fn disabled(harness: &Harness<'static, App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

fn value(harness: &Harness<'static, App>, label: &str) -> String {
    let node = harness.get_by_label(label);
    node.accesskit_node().value().unwrap_or_default()
}

fn click(harness: &mut Harness<'static, App>, label: &str) {
    harness.get_by_label(label).click();
    run_settled(harness);
}

/// The custom games in the saved config, once `ready` holds; saving is a background job.
fn run_until_saved(
    harness: &mut Harness<'static, App>,
    env: &Env,
    ready: impl Fn(&[CustomGame]) -> bool,
) -> Vec<CustomGame> {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the config to be saved", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| ready(&config.custom_games))
    });
    AppConfig::load(&env.config_dir)
        .unwrap()
        .unwrap()
        .custom_games
}

fn pair(linux: &str, windows: &str) -> Pair {
    Pair {
        linux: linux.into(),
        windows: windows.into(),
    }
}

/// Alpha, a Windows build in the prefix `pfx`, with two saves and a file outside its root.
fn alpha(tmp: &TempDir) -> (Harness<'static, App>, Env, PathBuf) {
    let prefix = make_prefix(tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let snapshot = scan(
        tmp,
        "Alpha",
        &[
            (Base::WinAppData, "Alpha/save.dat"),
            (Base::WinAppData, "Alpha/slot2.dat"),
            (Base::WinAppData, "Stray/x.dat"),
        ],
        &[(Base::WinAppData, "Alpha")],
    );
    let (harness, env) = open_tab(tmp, vec![game], &[snapshot]);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    (harness, env, saves)
}

/// Uni, a Unity game backed up from its Windows build, optionally with a prefix of its own. Its
/// `PlayerPrefs` live in the registry on Windows.
fn unity(tmp: &TempDir, prefix: Option<&Path>) -> Harness<'static, App> {
    let mut game = custom("Uni", &["<winLocalAppDataLow>/Co/Uni"], prefix, Vec::new());
    let prefs = "HKEY_CURRENT_USER/Software/Co/Uni".to_owned();
    game.entry.registry.insert(prefs, RegistryRule::default());
    let snapshot = scan(
        tmp,
        "Uni",
        &[(Base::WinLocalAppDataLow, "Co/Uni/save.dat")],
        &[(Base::WinLocalAppDataLow, "Co/Uni")],
    );
    open_tab(tmp, vec![game], &[snapshot]).0
}

/// Mix, backed up from its Windows build: a Unity root and a root no rule knows. Two Linux saves
/// leave no auto-pair and are the candidates.
fn mix(tmp: &TempDir) -> (Harness<'static, App>, Env) {
    let files = [
        "<winLocalAppDataLow>/Co/Mix",
        "<winAppData>/Mix",
        "<xdgData>/Mix",
        "<xdgConfig>/MixCfg",
    ];
    let game = custom("Mix", &files, None, Vec::new());
    let snapshot = scan(
        tmp,
        "Mix",
        &[
            (Base::WinLocalAppDataLow, "Co/Mix/b.sav"),
            (Base::WinAppData, "Mix/a.sav"),
        ],
        &[
            (Base::WinLocalAppDataLow, "Co/Mix"),
            (Base::WinAppData, "Mix"),
        ],
    );
    let (mut harness, env) = open_tab(tmp, vec![game], &[snapshot]);
    select(&mut harness, "Mix");
    preview(&mut harness);
    (harness, env)
}

#[test]
fn restore_preview_lists_writes_and_skips() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);

    assert!(shows(&harness, "Files to write (2)"));
    // Each file's row names its own target.
    for file in ["save.dat", "slot2.dat"] {
        let target = saves.join(file).display().to_string().replace('\\', "/");
        let source = format!("WinAppData/Alpha/{file}");
        assert_eq!(target_of(&harness, &source), Some(target), "{source}");
    }
    assert!(shows(&harness, "Skipped (1)"));
    assert!(shows(
        &harness,
        "WinAppData/Stray/x.dat: outside the game's save roots"
    ));
    assert!(!shows(&harness, "Cross-build restore"));
    assert!(!shows(&harness, "Where should the saves"));
}

#[test]
fn restore_only_safety_backups_preselect_no_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let snapshot = scan(
        &tmp,
        "Alpha",
        &[(Base::WinAppData, "Alpha/save.dat")],
        &[(Base::WinAppData, "Alpha")],
    );
    let (mut harness, _env) = open_tab_tagged(&tmp, vec![game], &[snapshot], &["pre-restore"]);
    select(&mut harness, "Alpha");

    assert_eq!(value(&harness, "Snapshot"), "Choose a snapshot");
    assert!(disabled(&harness, "Preview"));

    click(&mut harness, "Snapshot");
    let option = harness.query_all_by_label_contains(", pre-restore").next();
    option.expect("the safety backup is listed").click();
    run_settled(&mut harness);
    assert!(value(&harness, "Snapshot").ends_with(", pre-restore"));
    assert!(!disabled(&harness, "Preview"));
}

#[test]
fn restore_apply_disabled_until_plan_loaded() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    assert!(disabled(&harness, "Restore files"));
    select(&mut harness, "Alpha");
    assert!(disabled(&harness, "Restore files"));

    preview(&mut harness);
    assert!(!disabled(&harness, "Restore files"));

    // Another build drops the plan, which was made for the old targets.
    click(&mut harness, "Native Linux build");
    assert!(!shows(&harness, "Files to write"));
    assert!(disabled(&harness, "Restore files"));
}

#[test]
fn restore_fetch_preview_restore_reports() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, saves) = alpha(&tmp);
    fs::create_dir_all(&saves).unwrap();
    fs::write(saves.join("save.dat"), "old").unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(shows(&harness, "Safety backup: snapshot"));
    let target = saves.join("save.dat");
    assert!(shows_path(
        &harness,
        &format!("{}: restored", target.display())
    ));
    assert!(shows_path(
        &harness,
        &format!("{}: restored", saves.join("slot2.dat").display())
    ));
    assert_eq!(fs::read_to_string(&target).unwrap(), "Alpha/save.dat");
    assert_eq!(
        fs::read_to_string(saves.join("slot2.dat")).unwrap(),
        "Alpha/slot2.dat"
    );
    assert!(
        !saves.with_file_name("Stray").exists(),
        "a skipped file stays out"
    );
    // The safety backup was staged apart from the backup mirror.
    assert!(env.cache_dir.join("restore-stage").exists());
    assert!(!env.cache_dir.join("stage").exists());

    // The reloaded list offers the safety backup, but not as the default.
    run_until(&mut harness, "the list to reload", |h| {
        !disabled(h, "Load backups")
    });
    select(&mut harness, "Alpha");
    assert!(!value(&harness, "Snapshot").contains("pre-restore"));
    click(&mut harness, "Snapshot");
    assert!(shows(&harness, ", pre-restore"));
}

#[test]
fn restore_preview_per_file_override_changes_one_write() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");

    replace_text(
        &mut harness,
        "Override target 1",
        elsewhere.to_str().unwrap(),
    );
    click(&mut harness, "Override 1");

    assert!(shows_path(
        &harness,
        &format!("{} (overridden for this restore)", elsewhere.display())
    ));
    assert!(!shows_path(
        &harness,
        &saves.join("save.dat").display().to_string()
    ));
    assert!(shows_path(
        &harness,
        &saves.join("slot2.dat").display().to_string()
    ));

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "Alpha/save.dat");
    assert!(!saves.join("save.dat").exists());
    assert!(saves.join("slot2.dat").exists());
}

#[test]
fn restore_waits_for_a_typed_override_to_be_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");

    replace_text(
        &mut harness,
        "Override target 1",
        elsewhere.to_str().unwrap(),
    );
    assert!(disabled(&harness, "Restore files"));
    assert!(shows(&harness, "Override target 1 is not applied"));

    click(&mut harness, "Override 1");
    assert!(!disabled(&harness, "Restore files"));
    assert!(!shows(&harness, "is not applied"));
}

#[test]
fn restore_override_with_a_relative_path_shows_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);

    replace_text(&mut harness, "Override target 2", "relative/save.dat");
    click(&mut harness, "Override 2");

    assert!(shows(&harness, "not an absolute path"));
    assert!(!shows(&harness, "overridden for this restore"));
}

#[cfg(unix)]
#[test]
fn restore_preview_flags_an_absolute_target() {
    let tmp = tempfile::tempdir().unwrap();
    let saves = tmp.path().join("abs/Abso");
    let template = saves.to_str().unwrap().to_owned();
    let rest = template.trim_start_matches('/').to_owned();
    let game = custom("Abso", &[&template], None, Vec::new());
    let tail = format!("{rest}/save.dat");
    let snapshot = scan(
        &tmp,
        "Abso",
        &[(Base::Absolute, &tail)],
        &[(Base::Absolute, &rest)],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    select(&mut harness, "Abso");
    preview(&mut harness);

    let target = saves.join("save.dat");
    assert!(shows(
        &harness,
        &format!("{} (absolute path)", target.display())
    ));
}

#[test]
fn restore_engine_rule_mapping_needs_no_dialog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = unity(&tmp, None);
    select(&mut harness, "Uni");
    preview(&mut harness);

    assert!(!shows(&harness, "Where should the saves"));
    let target = under(&home(&tmp), ".config/unity3d/Co/Uni/save.dat");
    assert!(shows_path(&harness, &target.display().to_string()));
    assert!(shows(&harness, "Cross-build restore"));
    assert!(shows(&harness, "Note: Unity settings (PlayerPrefs)"));
}

#[test]
fn restore_prefix_found_but_user_picks_native_linux_mapping() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let mut harness = unity(&tmp, Some(&prefix));
    select(&mut harness, "Uni");
    let in_prefix = under(
        &prefix,
        "drive_c/users/steamuser/AppData/LocalLow/Co/Uni/save.dat",
    );

    // The found prefix is the default, next to the native build.
    preview(&mut harness);
    assert!(shows_path(&harness, &in_prefix.display().to_string()));
    assert!(!shows(&harness, "Cross-build restore"));

    click(&mut harness, "Windows build in a Wine/Proton prefix");
    preview(&mut harness);

    let native = under(&home(&tmp), ".config/unity3d/Co/Uni/save.dat");
    assert!(shows_path(&harness, &native.display().to_string()));
    assert!(!shows_path(&harness, &in_prefix.display().to_string()));
    assert!(shows(&harness, "Cross-build restore"));
}

#[test]
fn restore_cross_build_warning_shown() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    // A native Linux snapshot restored into the Windows build through a saved pair.
    let pairs = vec![pair("<xdgData>/Pairy", "<winAppData>/Pairy")];
    let game = custom("Pairy", &["<xdgData>/Pairy"], Some(&prefix), pairs);
    let snapshot = scan(
        &tmp,
        "Pairy",
        &[(Base::XdgData, "Pairy/save.dat")],
        &[(Base::XdgData, "Pairy")],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    select(&mut harness, "Pairy");
    click(&mut harness, "Native Linux build");
    preview(&mut harness);

    assert!(shows(&harness, "Cross-build restore"));
    let target = under(
        &prefix,
        "drive_c/users/steamuser/AppData/Roaming/Pairy/save.dat",
    );
    assert!(shows_path(&harness, &target.display().to_string()));
}

#[test]
fn restore_mapping_dialog_prefills_known_roots_and_blocks_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env) = mix(&tmp);

    assert!(shows(&harness, "Where should the saves of Mix go?"));
    assert!(shows(&harness, "<winLocalAppDataLow>/Co/Mix (1 file)"));
    assert!(shows(&harness, "<winAppData>/Mix (1 file)"));
    assert_eq!(value(&harness, "Target 1"), "<xdgConfig>/unity3d/Co/Mix");
    assert_eq!(value(&harness, "Target 2"), "");
    assert!(
        disabled(&harness, "Use these folders"),
        "the unknown root has no choice"
    );
    assert!(disabled(&harness, "Restore files"));

    replace_text(&mut harness, "Target 1", "<xdgData>/Other");
    assert_eq!(value(&harness, "Target 1"), "<xdgData>/Other");
    assert!(disabled(&harness, "Use these folders"));

    click(&mut harness, "Skip 2");
    assert!(!disabled(&harness, "Use these folders"));
    click(&mut harness, "Skip 2");
    assert!(disabled(&harness, "Use these folders"));
    // The manifest's Linux saves are offered for the unknown root.
    click(&mut harness, "Candidates 2");
    click(&mut harness, "<xdgConfig>/MixCfg");
    assert_eq!(value(&harness, "Target 2"), "<xdgConfig>/MixCfg");
    assert!(!disabled(&harness, "Use these folders"));
}

#[test]
fn restore_dialog_offers_a_prefix_instead() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env) = mix(&tmp);

    click(&mut harness, "Restore into a Wine prefix instead");

    assert!(!shows(&harness, "Where should the saves"));
    let toggled = |label| harness.get_by_label(label).accesskit_node().toggled();
    assert_eq!(toggled("Native Linux build"), Some(Toggled::False));
    let prefix = toggled("Windows build in a Wine/Proton prefix");
    assert_eq!(prefix, Some(Toggled::True));
}

#[test]
fn restore_dialog_confirm_saves_all_roots_as_pairs() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env) = mix(&tmp);
    replace_text(&mut harness, "Target 2", "<xdgData>/Mix");

    harness.get_by_label("Use these folders").click();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write (2)"));

    assert!(!shows(&harness, "Where should the saves"));
    let target = under(&home(&tmp), ".local/share/Mix/a.sav");
    assert!(shows_path(&harness, &target.display().to_string()));
    let games = run_until_saved(&mut harness, &env, |games| {
        games.iter().any(|game| !game.pairs.is_empty())
    });
    assert_eq!(
        games[0].pairs,
        [
            pair("<xdgConfig>/unity3d/Co/Mix", "<winLocalAppDataLow>/Co/Mix"),
            pair("<xdgData>/Mix", "<winAppData>/Mix"),
        ]
    );
}

#[test]
fn restore_dialog_skip_is_saved() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env) = mix(&tmp);
    click(&mut harness, "Skip 2");

    harness.get_by_label("Use these folders").click();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write (1)"));

    let games = run_until_saved(&mut harness, &env, |games| {
        games.iter().any(|game| !game.skips.is_empty())
    });
    assert_eq!(games[0].skips, ["<winAppData>/Mix"]);
}

#[test]
fn restore_typed_template_that_does_not_classify_shows_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env) = mix(&tmp);
    // A real path, and a template of the snapshot's own OS: neither is a Linux template.
    for typed in ["/home/me/Mix", "<winAppData>/Mix"] {
        replace_text(&mut harness, "Target 2", typed);
        harness.get_by_label("Use these folders").click();
        run_until(&mut harness, "the refusal", |h| {
            shows(h, "is not a placeholder template of this OS")
        });

        assert!(shows(&harness, "Where should the saves of Mix go?"));
        assert!(!shows(&harness, "Files to write"));
    }
    let games = AppConfig::load(&env.config_dir)
        .unwrap()
        .unwrap()
        .custom_games;
    assert!(games[0].pairs.is_empty(), "nothing is saved: {games:?}");

    // Editing the row clears its error.
    replace_text(&mut harness, "Target 2", "<xdgData>/Mix");
    assert!(!shows(&harness, "is not a placeholder template"));
}

/// A new session on the tests' repository.
fn open_repo(tmp: &TempDir) -> Storage {
    let location = RepoLocation {
        repository: repo_dir(tmp),
        options: Default::default(),
    };
    Storage::open(&location, "").unwrap()
}

/// The snapshots tagged `tag` in the tests' repository, newest first.
fn snapshots(tmp: &TempDir, tag: &str) -> Vec<SnapshotInfo> {
    open_repo(tmp).snapshots(tag).unwrap()
}

/// The ids of the game backups labelled `title`, newest first.
fn backups_of(tmp: &TempDir, title: &str) -> Vec<String> {
    let snapshots = snapshots(tmp, "salvajocs").into_iter();
    snapshots
        .filter(|snapshot| snapshot.label == title)
        .map(|snapshot| snapshot.id)
        .collect()
}

/// Alpha with a safety backup, a newer backup and saved file choices, and Beta with one backup,
/// listed in the Restore tab with Alpha chosen.
fn two_games(tmp: &TempDir) -> Harness<'static, App> {
    let alpha = scan(
        tmp,
        "Alpha",
        &[(Base::WinAppData, "Alpha/save.dat")],
        &[(Base::WinAppData, "Alpha")],
    );
    let beta = scan(
        tmp,
        "Beta",
        &[(Base::WinAppData, "Beta/save.dat")],
        &[(Base::WinAppData, "Beta")],
    );
    let (mut harness, _env) = open_tab(tmp, Vec::new(), &[beta]);
    let mut storage = open_repo(tmp);
    let stage = tmp.path().join("fixture-stage");
    let safety = ["pre-restore"];
    backup_games(
        &mut storage,
        std::slice::from_ref(&alpha),
        &stage,
        &safety,
        false,
        |_| {},
    )
    .unwrap();
    // Same length as the first contents, so the scan's size still holds.
    fs::write(&alpha.files[0].real, "ALPHA/SAVE.DAT").unwrap();
    backup_games(&mut storage, &[alpha], &stage, &[], false, |_| {}).unwrap();
    let skip = [Change::Skip(true)];
    selection::save(&mut storage, "Alpha", &skip, &tmp.path().join("settings")).unwrap();
    drop(storage);
    click(&mut harness, "Load backups");
    select(&mut harness, "Alpha");
    harness
}

/// Clicks Delete in the confirm dialog and waits for the list to reload.
fn confirm_delete(harness: &mut Harness<'static, App>, result: &str) {
    harness.get_by_label("Delete").click();
    run_until(harness, "the delete", |h| shows(h, result));
    run_settled(harness);
}

#[test]
fn deleting_a_snapshot_removes_only_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    let alpha = backups_of(&tmp, "Alpha");
    assert_eq!(alpha.len(), 2);
    let beta = backups_of(&tmp, "Beta");

    // The second, older one is the safety backup, not the preselected newest.
    click(&mut harness, "Snapshot");
    harness.get_by_label_contains(", pre-restore").click();
    run_settled(&mut harness);
    click(&mut harness, "Delete snapshot");
    assert!(shows(&harness, "Delete 1 backup of Alpha?"));
    // The dialog lists the chosen snapshot only; the picker shows its text as a value.
    assert_eq!(harness.query_all_by_label_contains(" UTC, ").count(), 1);
    assert!(shows(&harness, ", pre-restore"));
    assert!(shows(&harness, "for every computer. It cannot be undone."));
    assert!(!shows(&harness, "Your file choices for this game are kept"));
    confirm_delete(&mut harness, "Removed 1 backup");

    assert_eq!(backups_of(&tmp, "Alpha"), alpha[..1]);
    assert_eq!(backups_of(&tmp, "Beta"), beta);
    select(&mut harness, "Alpha");
    click(&mut harness, "Snapshot");
    // The open list holds the one snapshot left.
    assert_eq!(harness.query_all_by_label_contains(" UTC, ").count(), 1);
}

#[test]
fn deleting_all_backups_of_a_game_keeps_its_file_choices_and_other_games() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    let settings = snapshots(&tmp, "salvajocs-settings");
    assert_eq!(settings.len(), 1);
    let beta = backups_of(&tmp, "Beta");

    click(&mut harness, "Delete all backups of Alpha");
    assert!(shows(&harness, "Delete 2 backups of Alpha?"));
    assert!(shows(&harness, ", pre-restore"));
    assert!(shows(&harness, "Your file choices for this game are kept"));
    confirm_delete(&mut harness, "Removed 2 backups");

    assert!(backups_of(&tmp, "Alpha").is_empty());
    assert_eq!(backups_of(&tmp, "Beta"), beta);
    assert_eq!(snapshots(&tmp, "salvajocs-settings"), settings);
    assert!(!shows(&harness, "Alpha"));
    assert!(shows(&harness, "Beta"));
}

#[test]
fn cancelling_a_delete_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    let before = snapshots(&tmp, "salvajocs");

    click(&mut harness, "Delete all backups of Alpha");
    assert!(harness.get_by_label("Cancel").is_focused());
    click(&mut harness, "Cancel");

    assert!(!shows(&harness, "It cannot be undone"));
    assert_eq!(snapshots(&tmp, "salvajocs"), before);
}

/// A custom game with its saves in `<xdgData>/<name>`, and a scan of it with the `tails` below
/// `<xdgData>`; each file holds its tail.
fn native(tmp: &TempDir, name: &str, tails: &[&str]) -> (CustomGame, GameScan) {
    let game = custom(name, &[&format!("<xdgData>/{name}")], None, Vec::new());
    let files: Vec<_> = tails.iter().map(|tail| (Base::XdgData, *tail)).collect();
    let snapshot = scan(tmp, name, &files, &[(Base::XdgData, name)]);
    (game, snapshot)
}

/// Where the native games' saves go.
fn xdg_data(tmp: &TempDir) -> PathBuf {
    under(&home(tmp), ".local/share")
}

/// Writes `contents` to `path`, creating its parent directories.
fn put(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Clicks `button` in the sidebar and waits for the confirm of the batch.
fn plan_batch(harness: &mut Harness<'static, App>, button: &str) {
    harness.get_by_label(button).click();
    run_until(harness, "the batch confirm", |h| {
        h.query_by_label("Cancel").is_some()
    });
}

/// Where this process's batches fetch their snapshots.
fn batch_dir(env: &Env) -> PathBuf {
    let pid = std::process::id().to_string();
    env.cache_dir.join("restore-batch").join(pid)
}

/// Sets the modification time of the dir `dir` to a day and a half ago.
fn age(dir: &Path) {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_BACKUP_SEMANTICS, without which Windows opens no directory.
        options.write(true).custom_flags(0x0200_0000);
    }
    let old = SystemTime::now() - Duration::from_secs(36 * 60 * 60);
    options.open(dir).unwrap().set_modified(old).unwrap();
}

/// Clicks `button` in the confirm and waits for the run to end and the list to reload.
fn run_batch(harness: &mut Harness<'static, App>, button: &str, result: &str) {
    harness.get_by_label(button).click();
    run_until(harness, "the batch", |h| {
        shows(h, result) && !disabled(h, "Load backups")
    });
}

/// A custom game backed up from its Windows build, with one root no rule knows, and a scan of
/// it. Two Linux saves leave no auto-pair, so the batch asks where it goes.
fn unmapped(tmp: &TempDir, name: &str) -> (CustomGame, GameScan) {
    let files = [
        format!("<winAppData>/{name}"),
        format!("<xdgData>/{name}"),
        format!("<xdgConfig>/{name}Cfg"),
    ];
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let game = custom(name, &files, None, Vec::new());
    let tail = format!("{name}/a.sav");
    let snapshot = scan(
        tmp,
        name,
        &[(Base::WinAppData, &tail)],
        &[(Base::WinAppData, name)],
    );
    (game, snapshot)
}

/// Waits for the mapping dialog of `title`.
fn run_until_dialog(harness: &mut Harness<'static, App>, title: &str) {
    let heading = format!("Where should the saves of {title} go?");
    run_until(harness, "the mapping dialog", move |h| shows(h, &heading));
}

#[test]
fn batch_asks_for_each_game_that_needs_mapping_and_skips_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (kilo, kilo_scan) = unmapped(&tmp, "Kilo");
    let (lima, lima_scan) = unmapped(&tmp, "Lima");
    let (mut harness, env) = open_tab(&tmp, vec![kilo, lima], &[kilo_scan, lima_scan]);

    click(&mut harness, "Select all");
    harness.get_by_label("Restore 2 games").click();
    run_until_dialog(&mut harness, "Kilo");
    assert!(
        harness
            .query_by_label("Restore into a Wine prefix instead")
            .is_none()
    );
    replace_text(&mut harness, "Target 1", "<xdgData>/Kilo");
    harness.get_by_label("Use these folders").click();
    run_until_dialog(&mut harness, "Lima");
    click(&mut harness, "Skip this game");

    assert!(shows(
        &harness,
        "Lima: skipped in the folder mapping dialog"
    ));
    // The confirm tree shows where the files land, not the snapshot's Windows folders.
    assert!(harness.query_by_label("Open XdgData/Kilo").is_some());
    let snapshot_side = |node: &AccessKitNode<'_>| {
        node.label()
            .is_some_and(|label| label.starts_with("Open WinAppData"))
    };
    assert!(
        harness
            .query_all(by().predicate(snapshot_side))
            .next()
            .is_none()
    );
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");
    let kilo_save = under(&xdg_data(&tmp), "Kilo/a.sav");
    assert_eq!(fs::read_to_string(kilo_save).unwrap(), "Kilo/a.sav");
    assert!(!xdg_data(&tmp).join("Lima").exists());
    let games = run_until_saved(&mut harness, &env, |games| {
        games.iter().any(|game| !game.pairs.is_empty())
    });
    let kilo = games.iter().find(|game| game.name == "Kilo").unwrap();
    assert_eq!(kilo.pairs, [pair("<xdgData>/Kilo", "<winAppData>/Kilo")]);
    let lima = games.iter().find(|game| game.name == "Lima").unwrap();
    assert!(lima.pairs.is_empty() && lima.skips.is_empty(), "{lima:?}");
}

#[test]
fn cancelling_a_batch_dialog_restores_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (kilo, kilo_scan) = unmapped(&tmp, "Kilo");
    let (mut harness, env) = open_tab(&tmp, vec![beta, kilo], &[beta_scan, kilo_scan]);
    let beta_save = under(&xdg_data(&tmp), "Beta/save.dat");
    put(&beta_save, "old");

    click(&mut harness, "Select all");
    harness.get_by_label("Restore 2 games").click();
    run_until_dialog(&mut harness, "Kilo");
    click(&mut harness, "Cancel");

    assert!(!shows(&harness, "Where should the saves"));
    assert!(!shows(&harness, "Plan"));
    assert!(!disabled(&harness, "Restore 2 games"));
    assert_eq!(fs::read_to_string(&beta_save).unwrap(), "old");
    assert!(!xdg_data(&tmp).join("Kilo").exists());
    assert!(snapshots(&tmp, "pre-restore").is_empty());
    assert!(
        !batch_dir(&env).exists(),
        "the fetched snapshots are removed"
    );
}

#[test]
fn cancelling_a_later_dialog_keeps_the_folders_confirmed_before() {
    let tmp = tempfile::tempdir().unwrap();
    let (kilo, kilo_scan) = unmapped(&tmp, "Kilo");
    let (lima, lima_scan) = unmapped(&tmp, "Lima");
    let (mut harness, env) = open_tab(&tmp, vec![kilo, lima], &[kilo_scan, lima_scan]);

    click(&mut harness, "Select all");
    harness.get_by_label("Restore 2 games").click();
    run_until_dialog(&mut harness, "Kilo");
    replace_text(&mut harness, "Target 1", "<xdgData>/Kilo");
    harness.get_by_label("Use these folders").click();
    run_until_dialog(&mut harness, "Lima");
    click(&mut harness, "Cancel");

    let games = run_until_saved(&mut harness, &env, |games| {
        games.iter().any(|game| !game.pairs.is_empty())
    });
    let kilo = games.iter().find(|game| game.name == "Kilo").unwrap();
    assert_eq!(kilo.pairs, [pair("<xdgData>/Kilo", "<winAppData>/Kilo")]);
    assert!(!shows(&harness, "Where should the saves"));
    assert!(!xdg_data(&tmp).join("Kilo").exists(), "nothing is written");
    assert!(!xdg_data(&tmp).join("Lima").exists());
    assert!(snapshots(&tmp, "pre-restore").is_empty());
}

#[test]
fn batch_restores_every_ticked_game_behind_its_own_safety_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    let tails = ["Alpha/save.dat", "Beta/save.dat"];
    for tail in tails {
        put(&under(&xdg_data(&tmp), tail), "old");
    }
    // What an earlier session left a day and a half ago, and a dir salvajocs did not make.
    let batches = env.cache_dir.join("restore-batch");
    put(&batches.join("99999999/0/files/x"), "stale");
    age(&batches.join("99999999"));
    put(&batches.join("keep/x"), "not ours");
    age(&batches.join("keep"));

    click(&mut harness, "Include Alpha");
    click(&mut harness, "Include Beta");
    plan_batch(&mut harness, "Restore 2 games");
    assert!(!batches.join("99999999").exists());
    assert!(batches.join("keep/x").exists());
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    assert!(
        !batch_dir(&env).exists(),
        "the fetched snapshots are removed"
    );
    for tail in tails {
        let restored = fs::read_to_string(under(&xdg_data(&tmp), tail)).unwrap();
        assert_eq!(restored, tail);
    }
    let mut safety: Vec<String> = (snapshots(&tmp, "pre-restore").into_iter())
        .map(|snapshot| snapshot.label)
        .collect();
    safety.sort();
    assert_eq!(safety, ["Alpha", "Beta"], "one safety backup per game");
    let include = harness.get_by_label("Include Alpha");
    let toggled = include.accesskit_node().toggled();
    assert_eq!(toggled, Some(Toggled::False), "the ticks are cleared");
}

#[test]
fn planning_keeps_the_batch_dir_of_another_running_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, snapshot) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[snapshot]);
    // Another salvajocs plans a batch right now.
    let other = env.cache_dir.join("restore-batch/99999998/0/files/x");
    put(&other, "live");

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");

    assert_eq!(fs::read_to_string(&other).unwrap(), "live");
    assert!(batch_dir(&env).exists());
}

#[test]
fn a_single_restore_replaces_the_result_of_a_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, snapshot) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha], &[snapshot]);
    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");

    select(&mut harness, "Alpha");
    preview(&mut harness);
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(!shows(&harness, "Restored 1 of 1 game"));
}

#[test]
fn batch_restores_the_other_games_when_wine_blocks_one() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let source = tmp.path().join("source-pfx");
    put(
        &source.join("user.reg"),
        "WINE REGISTRY Version 2\n\n[Software\\\\Co\\\\Reg] 1\n#time=0\n\"v\"=\"saved\"\n",
    );
    let mut reg = custom("Reg", &["<winAppData>/Reg"], Some(&prefix), Vec::new());
    let key = "HKEY_CURRENT_USER/Software/Co/Reg".to_owned();
    reg.entry.registry.insert(key, RegistryRule::default());
    let mut reg_scan = scan(
        &tmp,
        "Reg",
        &[(Base::WinAppData, "Reg/save.dat")],
        &[(Base::WinAppData, "Reg")],
    );
    reg_scan.registry_keys = vec![r"HKEY_CURRENT_USER\Software\Co\Reg".into()];
    reg_scan.registry_source = Some(RegistrySource::WinePrefix(source));
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) =
        open_tab_with(&tmp, vec![reg, beta], &[reg_scan, beta_scan], &[], |env| {
            env.wineserver_running = || true
        });

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    assert!(shows(&harness, "Registry: 1 keys into the prefix"));
    run_batch(&mut harness, "Restore 2 games", "Restored 1 of 2 games");

    assert!(shows(&harness, "Close the game and Wine first"));
    let beta_save = under(&xdg_data(&tmp), "Beta/save.dat");
    assert_eq!(fs::read_to_string(beta_save).unwrap(), "Beta/save.dat");
    let reg_save = under(
        &prefix,
        "drive_c/users/steamuser/AppData/Roaming/Reg/save.dat",
    );
    assert!(
        !reg_save.exists(),
        "nothing is written for the refused game"
    );
    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    assert!(!user_reg.contains("saved"), "{user_reg}");
}

#[test]
fn a_game_with_only_safety_backups_cannot_be_ticked() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, snapshot) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, _env) = open_tab_tagged(&tmp, vec![alpha], &[snapshot], &["pre-restore"]);

    assert!(disabled(&harness, "Include Alpha"));
    click(&mut harness, "Select all");
    assert!(disabled(&harness, "Restore 0 games"));
}

#[test]
fn batch_confirm_counts_new_changed_and_unchanged_files_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let tails = ["Alpha/new.dat", "Alpha/changed.dat", "Alpha/same.dat"];
    let (alpha, snapshot) = native(&tmp, "Alpha", &tails);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[snapshot]);
    let saves = xdg_data(&tmp).join("Alpha");
    put(&saves.join("changed.dat"), "other");
    put(&saves.join("same.dat"), "Alpha/same.dat");

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");
    assert!(batch_dir(&env).exists());

    assert!(shows(&harness, "1 new, 1 changed, 1 unchanged"));
    assert!(shows(&harness, "Into native Linux"));
    assert!(!saves.join("new.dat").exists());
    let changed = fs::read_to_string(saves.join("changed.dat")).unwrap();
    assert_eq!(changed, "other");
    assert!(snapshots(&tmp, "pre-restore").is_empty());

    click(&mut harness, "Cancel");
    assert!(!shows(&harness, "1 new, 1 changed"));
    assert!(!disabled(&harness, "Restore 1 game"), "the ticks stay");
    assert!(!saves.join("new.dat").exists());
    assert!(snapshots(&tmp, "pre-restore").is_empty());
    assert!(!batch_dir(&env).exists(), "the fetched snapshot is removed");
}

#[test]
fn batch_confirm_tree_labels_each_file_and_toggles_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let tails = ["Alpha/new.dat", "Alpha/changed.dat", "Alpha/same.dat"];
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &tails);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let mut opened = None;
    let (mut harness, _env) = open_tab_with(
        &tmp,
        vec![alpha, beta],
        &[alpha_scan, beta_scan],
        &[],
        |env| opened = Some(record_opened(env)),
    );
    let saves = xdg_data(&tmp).join("Alpha");
    put(&saves.join("changed.dat"), "other");
    put(&saves.join("same.dat"), "Alpha/same.dat");

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    click(&mut harness, "Open XdgData/Alpha");
    click(&mut harness, "Open XdgData/Beta");

    for row in [
        "new.dat, new",
        "changed.dat, changed",
        "same.dat",
        "save.dat, new",
    ] {
        let role = harness.get_by_label(row).accesskit_node().role();
        assert_eq!(role, Role::Label, "{row} is read-only");
    }
    let checkboxes: Vec<String> = (harness.query_all(by().role(Role::CheckBox)))
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    assert!(
        checkboxes.iter().all(|label| label.starts_with("Include ")),
        "only the list's checkboxes: {checkboxes:?}"
    );
    assert!(
        (harness.query_by_label("Show XdgData/Beta in the file manager")).is_none(),
        "Beta's folder does not exist yet"
    );
    click(&mut harness, "Show XdgData/Alpha in the file manager");
    assert_eq!(*opened.unwrap().lock().unwrap(), [saves]);
}

#[cfg(unix)]
#[test]
fn batch_confirm_shows_a_write_that_will_fail_and_a_game_that_will_not_run() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat", "Alpha/dir.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    // A directory where a file goes fails that write only.
    let dir = under(&xdg_data(&tmp), "Alpha/dir.dat");
    fs::create_dir_all(&dir).unwrap();
    // A target the safety backup cannot read fails the whole game.
    let locked = under(&xdg_data(&tmp), "Beta/save.dat");
    put(&locked, "old");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(shows_path(
        &harness,
        &format!("{}: will fail", dir.display())
    ));
    assert!(shows(&harness, "1 new, 0 changed, 0 unchanged"));
    click(&mut harness, "Open XdgData/Alpha");
    assert!(harness.query_by_label("dir.dat, will fail").is_some());
    assert!(harness.query_by_label("save.dat, new").is_some());
    assert!(shows(
        &harness,
        "Beta: will not be restored: the safety backup cannot read"
    ));
    assert!(!disabled(&harness, "Restore 1 game"), "only Alpha runs");
}

#[test]
fn a_game_that_lost_its_last_regular_backup_is_unticked() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    click(&mut harness, "Include Alpha");
    assert!(!disabled(&harness, "Restore 1 game"));

    // The preselected snapshot is Alpha's only regular one; its safety backup stays.
    click(&mut harness, "Delete snapshot");
    confirm_delete(&mut harness, "Removed 1 backup");

    assert!(disabled(&harness, "Include Alpha"));
    let toggled = (harness.get_by_label("Include Alpha"))
        .accesskit_node()
        .toggled();
    assert_eq!(toggled, Some(Toggled::False));
    assert!(disabled(&harness, "Restore 0 games"));
}
