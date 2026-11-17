//! Headless checks of the Restore tab and its mapping dialog.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{
    env, harness, init_repo, local_config, replace_text, repo_dir, run_settled, run_until,
    run_until_ready, save_config, shows,
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
use salvajocs_core::scan::{Environment, FoundFile, GameScan};
use salvajocs_core::storage::{RepoLocation, Storage};
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
    let mut env = env(tmp);
    env.environment = Some(Environment {
        native: Some(Context::Linux(LinuxFolders {
            user: "me".into(),
            home: home(tmp),
            xdg_data: under(&home(tmp), ".local/share"),
            xdg_config: home(tmp).join(".config"),
        })),
        steam_roots: Vec::new(),
    });
    fs::create_dir_all(&env.cache_dir).unwrap();
    fs::write(
        env.cache_dir.join("manifest.json"),
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
    // A new dialog lays its button row out once it has measured it.
    run_settled(harness);
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
    assert!(shows_path(&harness, &format!("{}", target.display())));
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
    assert!(shows_path(&harness, &format!("{}", in_prefix.display())));
    assert!(!shows(&harness, "Cross-build restore"));

    click(&mut harness, "Windows build in a Wine/Proton prefix");
    preview(&mut harness);

    let native = under(&home(&tmp), ".config/unity3d/Co/Uni/save.dat");
    assert!(shows_path(&harness, &format!("{}", native.display())));
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
    assert!(shows_path(&harness, &format!("{}", target.display())));
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
    assert!(shows_path(&harness, &format!("{}", target.display())));
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
