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
use eframe::egui::epaint::Shape;
use eframe::egui::{Key, Pos2, Rect, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable, by};
use salvajocs_core::backup::backup_games;
use salvajocs_core::config::AppConfig;
use salvajocs_core::launcher::{Launcher, LauncherPrefix};
use salvajocs_core::layout::{Base, Context, LinuxFolders, PrefixOrigin};
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
    let entry = parse_yaml(&yaml).unwrap().games.remove(name).unwrap();
    let mut game = CustomGame::new(name.into(), entry, Integration::Override);
    game.wine_prefix = wine_prefix.map(Path::to_path_buf);
    game.pairs = pairs;
    game
}

/// A scan of `title` with `files` as `(base, tail)` and `roots`; each file holds its tail.
fn scan(tmp: &TempDir, title: &str, files: &[(Base, &str)], roots: &[(Base, &str)]) -> GameScan {
    let files = files
        .iter()
        .map(|&(base, tail)| {
            // Real titles hold characters Windows refuses in a folder name, such as ':'.
            let folder: String = title.chars().filter(|c| c.is_alphanumeric()).collect();
            let real = tmp.path().join("src").join(folder).join(tail);
            fs::create_dir_all(real.parent().unwrap()).unwrap();
            fs::write(&real, tail).unwrap();
            let modified = fs::metadata(&real).unwrap().modified().ok();
            FoundFile::new(base, tail.into(), real, tail.len() as u64, modified)
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
    let mut environment = Environment::default();
    environment.native = Some(Context::Linux(LinuxFolders::new(
        "me".into(),
        home(tmp),
        under(&home(tmp), ".local/share"),
        home(tmp).join(".config"),
    )));
    env.environment = Some(environment);
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
    let location = RepoLocation::new(repository.clone(), Default::default());
    let mut storage = Storage::open(&location, "").unwrap();
    backup_games(
        &mut storage,
        snapshots,
        &tmp.path().join("fixture-stage"),
        tags,
        None,
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

/// Waits for the plan, which starts by itself, or for its "Choose folders…" button.
fn preview(harness: &mut Harness<'static, App>) {
    run_until(harness, "the plan", |h| {
        shows(h, "Files to write") || h.query_by_label("Choose folders…").is_some()
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

/// Opens the plan's folder `folder` and waits for the hover text of its file row `row` to name
/// `target`: where that file goes.
fn goes_to(harness: &mut Harness<'static, App>, folder: &str, row: &str, target: &Path) {
    let open = format!("Open {folder}");
    if harness.query_by_label(&open).is_some() {
        click(harness, &open);
    }
    harness.get_by_label(row).hover();
    let want = target.display().to_string();
    run_until(harness, "where the file goes", move |h| {
        shows_path(h, &want)
    });
}

/// Opens the override field of the plan's file `key`, types `target` and applies it.
fn apply_override(harness: &mut Harness<'static, App>, key: &str, target: &Path) {
    click(harness, &format!("Override {key}"));
    let field = format!("New target of {key}");
    replace_text(harness, &field, target.to_str().unwrap());
    click(harness, &format!("Apply the new target of {key}"));
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
    alpha_with(tmp, |_| {})
}

/// Like [`alpha`], with `adjust` applied to the app's environment.
fn alpha_with(
    tmp: &TempDir,
    adjust: impl FnOnce(&mut Env),
) -> (Harness<'static, App>, Env, PathBuf) {
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
    let (harness, env) = open_tab_with(tmp, vec![game], &[snapshot], &[], adjust);
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

/// The key of Alpha's `save.dat` in the plan's tree: where it lands.
const ALPHA_SAVE: &str = "WinAppData/Alpha/save.dat";

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
    click(&mut harness, "Choose folders…");
    (harness, env)
}

#[test]
fn restore_preview_lists_writes_and_skips() {
    let tmp = tempfile::tempdir().unwrap();
    let mut opened = None;
    let (mut harness, _env, saves) = alpha_with(&tmp, |env| opened = Some(record_opened(env)));
    fs::create_dir_all(&saves).unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);

    assert!(shows(&harness, "Files to write (2)"));
    // Each file is listed under the folder it goes into, which opens where they land.
    click(&mut harness, "Open WinAppData/Alpha");
    for row in ["save.dat, new", "slot2.dat, new"] {
        assert!(harness.query_by_label(row).is_some(), "{row}");
    }
    click(&mut harness, "Show WinAppData/Alpha in the file manager");
    assert_eq!(*opened.unwrap().lock().unwrap(), [saves]);
    assert!(shows(&harness, "Skipped (1)"));
    assert!(shows(
        &harness,
        "WinAppData/Stray/x.dat: outside the game's save roots"
    ));
    assert!(!shows(&harness, "Cross-build restore"));
    assert!(!shows(&harness, "Where should the saves"));
}

#[test]
fn restore_plan_says_into_which_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);

    // Native Linux is ticked too, but no file of this Windows snapshot goes there.
    let prefix = tmp.path().join("pfx").display().to_string();
    assert!(shows_path(&harness, &format!("Into the prefix {prefix}")));
    assert!(!shows(&harness, "native Linux and"));
}

/// The rect of the text drawn for `text` in the last frame and whether it was elided to "…".
fn drawn_text(harness: &Harness<'static, App>, text: &str) -> Option<(Rect, bool)> {
    fn find(shape: &Shape, text: &str) -> Option<(Rect, bool)> {
        match shape {
            Shape::Text(drawn) if drawn.galley.text() == text => {
                Some((drawn.visual_bounding_rect(), drawn.galley.elided))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
            _ => None,
        }
    }
    (harness.output().shapes.iter()).find_map(|clipped| find(&clipped.shape, text))
}

const LONG_TITLE: &str = "Librarian: Tidy Up the Archive and More";

/// The Restore tab listing [`LONG_TITLE`] and Terraria.
fn long_title(tmp: &TempDir) -> Harness<'static, App> {
    let games = [LONG_TITLE, "Terraria"].map(|title| {
        // The save folder drops the ':', which Windows refuses in a name.
        let folder = title.replace(':', "");
        let tail = format!("{folder}/save.dat");
        scan(
            tmp,
            title,
            &[(Base::WinAppData, &tail)],
            &[(Base::WinAppData, &folder)],
        )
    });
    let (mut harness, _env) = open_tab(tmp, Vec::new(), &games);
    run_settled(&mut harness);
    harness
}

#[test]
fn restore_sidebar_truncates_a_long_title_after_its_checkbox() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = long_title(&tmp);

    let sidebar = harness.get_by_label("Load backups").rect();
    let checkbox = harness
        .get_by_label(&format!("Include {LONG_TITLE}"))
        .rect();
    let row = harness.get_by_label(LONG_TITLE).rect();
    assert!(
        row.left() >= checkbox.right(),
        "the title starts after its checkbox"
    );
    assert!(
        row.right() <= sidebar.right() + 0.5,
        "the row stays in the sidebar"
    );
    let (long, elided) = drawn_text(&harness, LONG_TITLE).expect("the title is drawn");
    assert!(elided, "the long title is cut short");
    assert!(
        long.right() <= sidebar.right(),
        "the text stays in the sidebar"
    );
    let (short, elided) = drawn_text(&harness, "Terraria").expect("the short title is drawn");
    assert!(!elided);
    let gap = short.left() - harness.get_by_label("Terraria").rect().left();
    assert!(gap < 16.0, "a short title is left-aligned: {gap}");
    assert!(
        (long.left() - row.left() - gap).abs() < 2.0,
        "the long and the short title start at the same offset"
    );

    harness.get_by_label(LONG_TITLE).hover();
    run_until(&mut harness, "the full title on hover", |h| {
        h.query_all_by_label(LONG_TITLE).count() > 1
    });
}

#[test]
fn restore_sidebar_widens_by_dragging_its_edge() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = long_title(&tmp);

    let before = harness.get_by_label("Load backups").rect();
    // The panel's edge lies past the sidebar's 12 px inner margin.
    let edge = Pos2::new(before.right() + 12.0, before.center().y + 200.0);
    let target = edge + Vec2::new(900.0, 0.0);
    harness.hover_at(edge);
    harness.step();
    harness.drag_at(edge);
    harness.step();
    harness.hover_at(target);
    harness.step();
    harness.drop_at(target);
    run_settled(&mut harness);

    let after = harness.get_by_label("Load backups").rect();
    assert!(after.width() > before.width() + 100.0, "the sidebar widens");
    // Dragged far past the cap, the sidebar leaves the plan its 600 px of the 1000 px window.
    assert!(
        after.right() + 12.0 <= 1000.0 - 600.0,
        "the sidebar stops at its cap"
    );
    let (_, elided) = drawn_text(&harness, LONG_TITLE).expect("the title is drawn");
    assert!(!elided, "the whole title shows in the wider sidebar");
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
    assert!(shows(&harness, "Choose a snapshot."));
    assert!(!shows(&harness, "Files to write"));

    click(&mut harness, "Snapshot");
    let option = harness.query_all_by_label_contains(", pre-restore").next();
    option.expect("the safety backup is listed").click();
    preview(&mut harness);
    assert!(value(&harness, "Snapshot").ends_with(", pre-restore"));
    assert!(!disabled(&harness, "Restore files"));
}

/// Where this process reads and downloads the single game's snapshot.
fn work_dir(env: &Env) -> PathBuf {
    let pid = std::process::id().to_string();
    env.cache_dir.join("restore-work").join(pid)
}

#[test]
fn restore_shows_the_plan_when_a_game_is_selected() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);

    assert!(shows(&harness, "Files to write (2)"));
    assert!(!work_dir(&env).exists(), "nothing is downloaded");
}

#[test]
fn restore_files_says_why_it_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    assert!(disabled(&harness, "Restore files"));
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(!disabled(&harness, "Restore files"));

    // Another build drops the plan, which was made for the old targets, and plans again.
    click(&mut harness, "Native Linux build");
    preview(&mut harness);
    assert!(!disabled(&harness, "Restore files"));

    click(&mut harness, "Windows build in a Wine/Proton prefix");
    assert!(!shows(&harness, "Files to write"));
    assert!(disabled(&harness, "Restore files"));
    assert!(shows(&harness, "Choose where to restore."));
}

#[test]
fn restore_switching_snapshots_shows_only_the_last_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, older) = native(&tmp, "Alpha", &["Alpha/old.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha], &[older]);
    // Another session backs the game up again after the app loaded the repository's index.
    let (_, newer) = native(&tmp, "Alpha", &["Alpha/new.dat"]);
    let stage = tmp.path().join("fixture-stage");
    backup_games(&mut open_repo(&tmp), &[newer], &stage, &[], None, |_| {}).unwrap();
    click(&mut harness, "Load backups");
    select(&mut harness, "Alpha");
    preview(&mut harness);

    // The older snapshot, then at once the newer one, before the first plan can end.
    for nth in [1, 0] {
        harness.get_by_label("Snapshot").click();
        harness.step();
        harness.step();
        let mut options = harness.query_all_by_label_contains(" UTC, ");
        options.nth(nth).expect("both snapshots are listed").click();
        drop(options);
        harness.step();
    }
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));

    click(&mut harness, "Open XdgData/Alpha");
    assert!(shows(&harness, "new.dat"));
    assert!(!shows(&harness, "old.dat"));
}

#[test]
fn restore_prefix_typing_waits_for_enter() {
    let tmp = tempfile::tempdir().unwrap();
    let mut opened = None;
    let (mut harness, _env, _) = alpha_with(&tmp, |env| opened = Some(record_opened(env)));
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let other = tmp.path().join("other-pfx");
    fs::create_dir_all(other.join("drive_c/users/steamuser")).unwrap();
    fs::write(other.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let saves = under(&other, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    fs::create_dir_all(&saves).unwrap();

    replace_text(&mut harness, "Prefix", other.to_str().unwrap());
    run_settled(&mut harness);
    assert!(shows(&harness, "Press Enter to use this prefix."));
    assert!(!shows(&harness, "Files to write"));

    harness.key_press(Key::Enter);
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));
    click(&mut harness, "Show WinAppData/Alpha in the file manager");
    assert_eq!(*opened.unwrap().lock().unwrap(), [saves]);
}

#[test]
fn restore_downloads_before_the_safety_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    let target = saves.join("save.dat");
    fs::create_dir_all(&saves).unwrap();
    fs::write(&target, "old").unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);

    let alpha = backups_of(&tmp, "Alpha");
    open_repo(&tmp).forget(&[&alpha[0]]).unwrap();
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the refusal", |h| {
        shows(h, "Nothing was restored")
    });

    assert!(shows(&harness, "the backup cannot be fetched"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    assert!(snapshots(&tmp, "pre-restore").is_empty());
}

#[test]
fn restore_plan_labels_each_file_and_compares_an_override_afresh() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    fs::create_dir_all(&saves).unwrap();
    fs::write(saves.join("save.dat"), "Alpha/save.dat").unwrap();
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");
    fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    fs::write(&elsewhere, "other").unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");

    // The same bytes as the backup's: unchanged, so no icon.
    assert!(
        harness
            .query_all_by_label("1 new, 1 unchanged")
            .next()
            .is_some()
    );
    assert!(harness.query_by_label("save.dat").is_some());
    assert!(harness.query_by_label("New: not on this PC yet").is_some());
    assert!(
        harness
            .query_by_label("Changed: will be overwritten")
            .is_none()
    );

    apply_override(&mut harness, ALPHA_SAVE, &elsewhere);
    assert!(
        harness
            .query_all_by_label("1 new, 1 changed")
            .next()
            .is_some()
    );
    assert!(harness.query_by_label("save.dat, changed").is_some());
    assert!(shows_path(
        &harness,
        &format!("{} (overridden for this restore)", elsewhere.display())
    ));

    // Clearing it puts the planned target back, compared afresh.
    click(&mut harness, &format!("Clear the override of {ALPHA_SAVE}"));
    assert!(harness.query_by_label("save.dat").is_some());
    assert!(!shows(&harness, "overridden for this restore"));
    assert!(
        harness
            .query_all_by_label("1 new, 1 unchanged")
            .next()
            .is_some()
    );
}

#[test]
fn a_big_plan_draws_only_its_folders_until_one_is_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let tails: Vec<String> = (["a", "b"].iter())
        .flat_map(|dir| (0..100).map(move |file| format!("Big/{dir}/file{file:03}.dat")))
        .collect();
    let tails: Vec<&str> = tails.iter().map(String::as_str).collect();
    let (big, snapshot) = native(&tmp, "Big", &tails);
    let (mut harness, _env) = open_tab(&tmp, vec![big], &[snapshot]);
    select(&mut harness, "Big");
    preview(&mut harness);
    let rows = |harness: &Harness<'static, App>| {
        let rows = (harness.query_all_by_label_contains("file0"))
            .filter(|node| node.accesskit_node().role() == Role::Label)
            .count();
        let buttons = harness.query_all_by_label_contains("Override ").count();
        (rows, buttons)
    };

    assert!(shows(&harness, "Files to write (200)"));
    assert!(shows(&harness, "200 files"));
    assert_eq!(rows(&harness), (0, 0));

    click(&mut harness, "Open XdgData/Big");
    assert!(harness.query_by_label("Open a").is_some());
    assert_eq!(rows(&harness), (0, 0));

    click(&mut harness, "Open a");
    assert_eq!(rows(&harness), (100, 100));
    assert!(harness.query_by_label("file000.dat, new").is_some());
}

#[test]
fn override_on_one_row_shows_one_field() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    assert!(!shows(&harness, "New target of "));

    click(&mut harness, &format!("Override {ALPHA_SAVE}"));
    let fields: Vec<_> = (harness.query_all_by_label_contains("New target of "))
        .filter(|node| node.accesskit_node().role() == Role::TextInput)
        .collect();
    assert_eq!(fields.len(), 1);
    drop(fields);
    let field = format!("New target of {ALPHA_SAVE}");
    assert!(harness.query_by_label(&field).is_some());

    // A second click closes it again.
    click(&mut harness, &format!("Override {ALPHA_SAVE}"));
    assert!(!shows(&harness, "New target of "));
}

/// The key of Alpha's `slot2.dat` in the plan's tree.
const ALPHA_SLOT2: &str = "WinAppData/Alpha/slot2.dat";

#[test]
fn a_cleared_override_restores_to_the_planned_target() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");
    click(&mut harness, "Open WinAppData/Alpha");

    apply_override(&mut harness, ALPHA_SAVE, &elsewhere);
    assert!(shows(&harness, "1 file overridden"));
    click(&mut harness, &format!("Clear the override of {ALPHA_SAVE}"));
    assert!(!shows(&harness, "overridden"));

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));
    let restored = fs::read_to_string(saves.join("save.dat")).unwrap();
    assert_eq!(restored, "Alpha/save.dat");
    assert!(!elsewhere.exists());
}

#[test]
fn an_override_onto_a_directory_will_fail_until_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    let dir = under(tmp.path(), "elsewhere/slot2.dat");
    fs::create_dir_all(&dir).unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");

    apply_override(&mut harness, ALPHA_SLOT2, &dir);
    assert!(harness.query_by_label("slot2.dat, will fail").is_some());
    assert!(harness.query_by_label("save.dat, new").is_some());

    click(
        &mut harness,
        &format!("Clear the override of {ALPHA_SLOT2}"),
    );
    assert!(harness.query_by_label("slot2.dat, new").is_some());
    assert!(!shows(&harness, "Will fail"));
}

#[cfg(unix)]
#[test]
fn an_override_onto_an_unreadable_file_blocks_the_restore_until_cleared() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    let locked = under(tmp.path(), "elsewhere/slot2.dat");
    put(&locked, "old");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");

    apply_override(&mut harness, ALPHA_SLOT2, &locked);
    assert!(shows(&harness, "Nothing will be restored"));
    assert!(disabled(&harness, "Restore files"));

    click(
        &mut harness,
        &format!("Clear the override of {ALPHA_SLOT2}"),
    );
    assert!(!shows(&harness, "Nothing will be restored"));
    assert!(harness.query_by_label("slot2.dat, new").is_some());
    assert!(!disabled(&harness, "Restore files"));
}

#[test]
fn a_new_plan_drops_the_overrides_and_open_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");
    click(&mut harness, "Open WinAppData/Alpha");
    apply_override(&mut harness, ALPHA_SAVE, &elsewhere);
    click(&mut harness, &format!("Override {ALPHA_SLOT2}"));
    let field = format!("New target of {ALPHA_SLOT2}");
    replace_text(&mut harness, &field, elsewhere.to_str().unwrap());
    assert!(disabled(&harness, "Restore files"));

    // Other targets plan again.
    click(&mut harness, "Native Linux build");
    preview(&mut harness);
    if harness.query_by_label("Open WinAppData/Alpha").is_some() {
        click(&mut harness, "Open WinAppData/Alpha");
    }

    assert!(harness.query_by_label("save.dat, new").is_some());
    assert!(!shows(&harness, "overridden"));
    assert!(!shows(&harness, "New target of "));
    assert!(!shows(&harness, "is not applied"));
    assert!(!disabled(&harness, "Restore files"));
}

#[test]
fn writes_that_land_on_one_file_keep_their_own_icons_and_names() {
    let tmp = tempfile::tempdir().unwrap();
    // Both builds' saves, restored into the native build through a pair: two writes, one target.
    let pairs = vec![pair("<xdgData>/Dup", "<winAppData>/Dup")];
    let game = custom("Dup", &["<xdgData>/Dup", "<winAppData>/Dup"], None, pairs);
    let snapshot = scan(
        &tmp,
        "Dup",
        &[
            (Base::XdgData, "Dup/save.dat"),
            (Base::WinAppData, "Dup/save.dat"),
        ],
        &[(Base::XdgData, "Dup"), (Base::WinAppData, "Dup")],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    let elsewhere = under(tmp.path(), "elsewhere/save.dat");
    put(&elsewhere, "other");
    select(&mut harness, "Dup");
    preview(&mut harness);
    assert!(shows(&harness, "Files to write (2)"));
    click(&mut harness, "Open XdgData/Dup");
    let key = "XdgData/Dup/save.dat";
    for nth in [1, 2] {
        let button = format!("Override {key} ({nth})");
        assert!(harness.query_by_label(&button).is_some(), "{button}");
    }

    apply_override(&mut harness, &format!("{key} (2)"), &elsewhere);

    assert_eq!(harness.query_all_by_label("save.dat, new").count(), 1);
    assert_eq!(harness.query_all_by_label("save.dat, changed").count(), 1);
    assert!(
        harness
            .query_all_by_label("1 new, 1 changed")
            .next()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn restore_files_waits_for_a_target_the_safety_backup_cannot_read() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha(&tmp);
    let locked = saves.join("save.dat");
    fs::create_dir_all(&saves).unwrap();
    fs::write(&locked, "old").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }
    select(&mut harness, "Alpha");
    preview(&mut harness);
    let blocked = shows(&harness, "Nothing will be restored");
    let listed = shows_path(&harness, &locked.display().to_string());
    let disabled_while_blocked = disabled(&harness, "Restore files");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(blocked && listed && disabled_while_blocked);
    click(&mut harness, "Try again");
    preview(&mut harness);
    assert!(!shows(&harness, "Nothing will be restored"));
    assert!(!disabled(&harness, "Restore files"));
}

/// Gives a dir back its mode when dropped, so a failing test does not leave it locked.
#[cfg(unix)]
struct Unlock(PathBuf, u32);

#[cfg(unix)]
impl Drop for Unlock {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1));
    }
}

/// Makes the repository's data packs unreadable until the result is dropped; `None` when they
/// stay readable, as for root, so there is nothing to test.
#[cfg(unix)]
fn lock_data(tmp: &TempDir) -> Option<Unlock> {
    use std::os::unix::fs::PermissionsExt;
    // The data packs hold `salvajocs.json`, which the plan reads; they are never cached.
    let data = Path::new(&repo_dir(tmp)).join("data");
    let mode = fs::metadata(&data).unwrap().permissions().mode();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o000)).unwrap();
    let unlock = Unlock(data.clone(), mode);
    fs::read_dir(&data).is_err().then_some(unlock)
}

#[cfg(unix)]
#[test]
fn a_failed_plan_offers_try_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    let Some(unlock) = lock_data(&tmp) else {
        return;
    };
    select(&mut harness, "Alpha");
    run_until(&mut harness, "the plan to fail", |h| {
        h.query_by_label("Try again").is_some()
    });
    drop(unlock);
    assert!(shows(&harness, "The plan failed"));
    assert!(disabled(&harness, "Restore files"));

    click(&mut harness, "Try again");
    preview(&mut harness);
    assert!(shows(&harness, "Files to write (2)"));
    assert!(!disabled(&harness, "Restore files"));
}

#[cfg(unix)]
#[test]
fn try_again_after_a_failed_read_fills_in_the_backups_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic])
    });
    let Some(unlock) = lock_data(&tmp) else {
        return;
    };
    select(&mut harness, "Tower Wizard");
    run_until(&mut harness, "the plan to fail", |h| {
        h.query_by_label("Try again").is_some()
    });
    drop(unlock);

    click(&mut harness, "Try again");
    preview(&mut harness);
    assert!(!shows(&harness, "Cross-build restore"));
    let folder = format!("WinAppData/{TOWER}");
    goes_to(
        &mut harness,
        &folder,
        "save.tres, new",
        &tower_save(&generic),
    );
}

#[test]
fn restore_needs_mapping_waits_behind_choose_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env) = mix(&tmp);
    assert!(shows(&harness, "Where should the saves of Mix go?"));

    click(&mut harness, "Cancel");
    assert!(!shows(&harness, "Where should the saves"));
    assert!(shows(
        &harness,
        "This backup has 1 folder from the other build without a place here."
    ));
    assert!(disabled(&harness, "Restore files"));

    click(&mut harness, "Choose folders…");
    assert!(shows(&harness, "Where should the saves of Mix go?"));
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
    click(&mut harness, "Open WinAppData/Alpha");

    apply_override(&mut harness, ALPHA_SAVE, &elsewhere);

    assert!(shows_path(
        &harness,
        &format!("{} (overridden for this restore)", elsewhere.display())
    ));
    assert!(!shows(&harness, "New target of "), "the field closes");

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
    click(&mut harness, "Open WinAppData/Alpha");

    click(&mut harness, &format!("Override {ALPHA_SAVE}"));
    let field = format!("New target of {ALPHA_SAVE}");
    replace_text(&mut harness, &field, elsewhere.to_str().unwrap());
    assert!(disabled(&harness, "Restore files"));
    let unapplied = format!("The new target of {ALPHA_SAVE} is not applied");
    assert!(shows(&harness, &unapplied));

    // Closing the field drops what was typed.
    click(&mut harness, &format!("Override {ALPHA_SAVE}"));
    assert!(!disabled(&harness, "Restore files"));
    click(&mut harness, &format!("Override {ALPHA_SAVE}"));
    assert_eq!(value(&harness, &field), "");
    replace_text(&mut harness, &field, elsewhere.to_str().unwrap());
    assert!(shows(&harness, &unapplied));

    click(
        &mut harness,
        &format!("Apply the new target of {ALPHA_SAVE}"),
    );
    assert!(!disabled(&harness, "Restore files"));
    assert!(!shows(&harness, "is not applied"));
}

#[test]
fn restore_override_with_a_relative_path_shows_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");

    apply_override(&mut harness, ALPHA_SAVE, Path::new("relative/save.dat"));

    assert!(shows(&harness, "not an absolute path"));
    assert!(!shows(&harness, "overridden for this restore"));
    assert!(harness.query_by_label("save.dat, new").is_some());
    assert!(disabled(&harness, "Restore files"));
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

    // The file lands under the `Absolute` folder, by its full path.
    let folder = format!("Absolute/{rest}");
    goes_to(
        &mut harness,
        &folder,
        "save.dat, new",
        &saves.join("save.dat"),
    );
}

#[test]
fn restore_engine_rule_mapping_needs_no_dialog() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = unity(&tmp, None);
    select(&mut harness, "Uni");
    preview(&mut harness);

    assert!(!shows(&harness, "Where should the saves"));
    assert!(shows(&harness, "Cross-build restore"));
    assert!(shows(&harness, "Note: Unity settings (PlayerPrefs)"));
    let target = under(&home(&tmp), ".config/unity3d/Co/Uni/save.dat");
    goes_to(
        &mut harness,
        "XdgConfig/unity3d/Co/Uni",
        "save.dat, new",
        &target,
    );
}

#[test]
fn restore_prefix_found_but_user_picks_native_linux_mapping() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let mut harness = unity(&tmp, Some(&prefix));
    select(&mut harness, "Uni");
    let (in_prefix, native) = ("WinLocalAppDataLow/Co/Uni", "XdgConfig/unity3d/Co/Uni");

    // The found prefix is the default, next to the native build.
    preview(&mut harness);
    assert!(!shows(&harness, "Cross-build restore"));
    let target = under(
        &prefix,
        "drive_c/users/steamuser/AppData/LocalLow/Co/Uni/save.dat",
    );
    goes_to(&mut harness, in_prefix, "save.dat, new", &target);

    click(&mut harness, "Windows build in a Wine/Proton prefix");
    preview(&mut harness);

    assert!(!shows(&harness, &format!("Open {in_prefix}")));
    assert!(!shows(&harness, &format!("Close {in_prefix}")));
    assert!(shows(&harness, "Cross-build restore"));
    let target = under(&home(&tmp), ".config/unity3d/Co/Uni/save.dat");
    goes_to(&mut harness, native, "save.dat, new", &target);
}

/// Tower Wizard's save folder below `<winAppData>`.
const TOWER: &str = "Godot/app_userdata/Tower Wizard";

/// A Bottles bottle `name` below the tests' home, with a `steamuser` home.
fn bottle(tmp: &TempDir, name: &str) -> PathBuf {
    let bottle = under(&home(tmp), ".local/share/bottles/bottles").join(name);
    fs::create_dir_all(bottle.join("drive_c/users/steamuser")).unwrap();
    fs::write(bottle.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    bottle
}

/// Makes Bottles report `bottles`, each named after its folder.
fn found_bottles(env: &mut Env, bottles: &[&Path]) {
    let environment = env.environment.as_mut().unwrap();
    for path in bottles {
        let mut prefix = LauncherPrefix::new(Launcher::Bottles, path.to_path_buf(), None);
        prefix.name = path.file_name().unwrap().to_str().unwrap().into();
        environment.launcher_prefixes.push(prefix);
    }
}

/// Tower Wizard, a Godot game backed up from its Windows build, from the bottle `from`.
fn tower(tmp: &TempDir, from: Option<&str>) -> (CustomGame, GameScan) {
    let template = format!("<winAppData>/{TOWER}");
    let game = custom("Tower Wizard", &[&template], None, Vec::new());
    let tail = format!("{TOWER}/save.tres");
    let files = [(Base::WinAppData, tail.as_str())];
    let mut snapshot = scan(tmp, "Tower Wizard", &files, &[(Base::WinAppData, TOWER)]);
    snapshot.prefix_origin = from.map(|name| PrefixOrigin::Bottles(name.into()));
    (game, snapshot)
}

/// Where Tower Wizard's save goes in `bottle`.
fn tower_save(bottle: &Path) -> PathBuf {
    let rel = format!("drive_c/users/steamuser/AppData/Roaming/{TOWER}/save.tres");
    under(bottle, &rel)
}

#[test]
fn restore_fills_in_the_prefix_the_backup_came_from() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic])
    });
    select(&mut harness, "Tower Wizard");
    preview(&mut harness);

    assert!(!shows(&harness, "Cross-build restore"));
    let folder = format!("WinAppData/{TOWER}");
    goes_to(
        &mut harness,
        &folder,
        "save.tres, new",
        &tower_save(&generic),
    );
}

#[test]
fn restore_finds_the_prefix_that_holds_the_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = bottle(&tmp, "Empty");
    let generic = bottle(&tmp, "Generic-Game");
    put(&tower_save(&generic), "older");
    let (game, snapshot) = tower(&tmp, None);
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&empty, &generic])
    });
    select(&mut harness, "Tower Wizard");
    preview(&mut harness);

    assert!(!shows(&harness, "Cross-build restore"));
    let folder = format!("WinAppData/{TOWER}");
    goes_to(
        &mut harness,
        &folder,
        "save.tres, changed",
        &tower_save(&generic),
    );
}

/// Opens the snapshot list and picks its `nth` snapshot, newest first.
fn choose_snapshot(harness: &mut Harness<'static, App>, nth: usize) {
    harness.get_by_label("Snapshot").click();
    harness.step();
    harness.step();
    let mut options = harness.query_all_by_label_contains(" UTC, ");
    options.nth(nth).expect("the snapshot is listed").click();
    drop(options);
    harness.step();
}

#[test]
fn restore_snapshot_switch_keeps_an_edited_pick() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let other = bottle(&tmp, "Other");
    let (game, older) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[older], &[], |env| {
        found_bottles(env, &[&generic, &other])
    });
    let (_, newer) = tower(&tmp, Some("Other"));
    let stage = tmp.path().join("fixture-stage");
    backup_games(&mut open_repo(&tmp), &[newer], &stage, &[], None, |_| {}).unwrap();
    click(&mut harness, "Load backups");
    select(&mut harness, "Tower Wizard");
    let shown = |prefix: &Path| {
        let want = prefix.display().to_string();
        move |h: &Harness<'static, App>| value(h, "Prefix") == want
    };
    run_until(&mut harness, "the newest snapshot's bottle", shown(&other));

    // Untouched, the pick follows the snapshot.
    choose_snapshot(&mut harness, 1);
    run_until(&mut harness, "the older snapshot's bottle", shown(&generic));
    preview(&mut harness);

    // Changed by the user, it stays.
    click(&mut harness, "Native Linux build");
    choose_snapshot(&mut harness, 0);
    preview(&mut harness);
    assert_eq!(value(&harness, "Prefix"), generic.display().to_string());
    let native = harness.get_by_label("Native Linux build");
    assert_eq!(native.accesskit_node().toggled(), Some(Toggled::False));
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
    goes_to(&mut harness, "WinAppData/Pairy", "save.dat, new", &target);
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
    goes_to(&mut harness, "XdgData/Mix", "a.sav, new", &target);
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
    let location = RepoLocation::new(repo_dir(tmp), Default::default());
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
        None,
        |_| {},
    )
    .unwrap();
    // Same length as the first contents, so the scan's size still holds.
    fs::write(&alpha.files[0].real, "ALPHA/SAVE.DAT").unwrap();
    backup_games(&mut storage, &[alpha], &stage, &[], None, |_| {}).unwrap();
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
    let (mut harness, _env) = open_tab(&tmp, vec![beta, kilo], &[beta_scan, kilo_scan]);
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
    assert!(
        batches.join("99999999").exists(),
        "planning removes nothing"
    );
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    assert!(!batches.join("99999999").exists());
    assert!(batches.join("keep/x").exists());
    assert!(
        !batch_dir(&env).exists(),
        "the downloaded snapshots are removed"
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
fn batch_planning_downloads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, snapshot) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[snapshot]);

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");

    assert!(harness.query_all_by_label("1 new").next().is_some());
    assert!(!batch_dir(&env).exists());
}

#[test]
fn a_batch_run_keeps_the_batch_dir_of_another_running_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, snapshot) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[snapshot]);
    // Another salvajocs restores a batch right now.
    let other = env.cache_dir.join("restore-batch/99999998/0/files/x");
    put(&other, "live");

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");

    assert_eq!(fs::read_to_string(&other).unwrap(), "live");
    assert!(!batch_dir(&env).exists());
}

#[test]
fn a_batch_game_whose_backup_is_gone_fails_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    let tails = ["Alpha/save.dat", "Beta/save.dat"];
    for tail in tails {
        put(&under(&xdg_data(&tmp), tail), "old");
    }

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    let alpha = backups_of(&tmp, "Alpha");
    open_repo(&tmp).forget(&[&alpha[0]]).unwrap();
    run_batch(&mut harness, "Restore 2 games", "Restored 1 of 2 games");

    assert!(shows(&harness, "the backup cannot be fetched"));
    let alpha_save = fs::read_to_string(under(&xdg_data(&tmp), tails[0])).unwrap();
    assert_eq!(alpha_save, "old", "nothing is written for the failed game");
    let beta_save = fs::read_to_string(under(&xdg_data(&tmp), tails[1])).unwrap();
    assert_eq!(beta_save, tails[1]);
    let safety: Vec<String> = (snapshots(&tmp, "pre-restore").into_iter())
        .map(|snapshot| snapshot.label)
        .collect();
    assert_eq!(safety, ["Beta"], "no safety backup for the failed game");
    assert!(!batch_dir(&env).exists(), "the downloads are removed");
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
    let (mut harness, _env) = open_tab(&tmp, vec![alpha], &[snapshot]);
    let saves = xdg_data(&tmp).join("Alpha");
    put(&saves.join("changed.dat"), "other");
    put(&saves.join("same.dat"), "Alpha/same.dat");

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");

    let counts = harness.get_by_label("1 new, 1 changed, 1 unchanged");
    assert_eq!(counts.accesskit_node().role(), Role::Image);
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
}

#[test]
fn batch_restore_uses_the_prefix_the_backup_came_from() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic])
    });

    click(&mut harness, "Include Tower Wizard");
    plan_batch(&mut harness, "Restore 1 game");
    assert!(!shows(&harness, "Cross-build restore"));
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");

    let restored = fs::read_to_string(tower_save(&generic)).unwrap();
    assert_eq!(restored, format!("{TOWER}/save.tres"));
}

#[test]
fn batch_confirm_names_only_the_prefix_the_writes_use() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    // A custom prefix makes the default both native Linux and the prefix.
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let snapshot = scan(
        &tmp,
        "Alpha",
        &[(Base::WinAppData, "Alpha/save.dat")],
        &[(Base::WinAppData, "Alpha")],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");

    let into = format!("Into the prefix {}", prefix.display());
    assert!(shows_path(&harness, &into));
    assert!(!shows(&harness, "native Linux"));
}

#[test]
fn batch_confirm_labels_a_pre_0_6_snapshot_by_size_and_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, _) = native(&tmp, "Alpha", &[]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[beta_scan]);
    // A stage as 0.5.0 made it: no hashes.json.
    let stage = tmp.path().join("old-stage");
    let meta = r#"{"format":1,"title":"Alpha","roots":[["XdgData","Alpha"]]}"#;
    put(&stage.join("salvajocs.json"), meta);
    put(&stage.join("files/XdgData/Alpha/same.dat"), "same");
    let slug = salvajocs_core::slug::slug("Alpha");
    let (as_path, game_tag) = (format!("/salvajocs/{slug}"), format!("game:{slug}"));
    let tags = ["salvajocs", game_tag.as_str()];
    open_repo(&tmp)
        .backup_dir(&stage, &as_path, &tags, "Alpha")
        .unwrap()
        .unwrap();
    // The same bytes with another mtime.
    put(&under(&xdg_data(&tmp), "Alpha/same.dat"), "same");
    click(&mut harness, "Load backups");

    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");
    click(&mut harness, "Open XdgData/Alpha");

    assert!(harness.query_all_by_label("1 changed").next().is_some());
    assert!(harness.query_by_label("same.dat, changed").is_some());
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
    for (icon, files) in [
        ("New: not on this PC yet", 2),
        ("Changed: will be overwritten", 1),
    ] {
        let icons: Vec<_> = harness.query_all_by_label(icon).collect();
        assert_eq!(icons.len(), files, "{icon}");
        assert!(
            icons
                .iter()
                .all(|node| node.accesskit_node().role() == Role::Image)
        );
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
    // Zero counts are left out.
    let counts = harness.get_by_label("1 new");
    assert_eq!(counts.accesskit_node().role(), Role::Image);
    click(&mut harness, "Open XdgData/Alpha");
    assert!(harness.query_by_label("dir.dat, will fail").is_some());
    let will_fail = harness
        .get_by_label_contains("Will fail: ")
        .accesskit_node();
    assert_eq!(will_fail.role(), Role::Image);
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
