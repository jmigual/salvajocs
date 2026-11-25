//! Headless checks of the Restore tab and its mapping dialog.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{
    back_up_unsent, click_at, env, harness, init_repo, local_config, pick, record_opened,
    replace_text, repo_dir, run_settled, run_until, run_until_backed_up, run_until_ready,
    save_config, shows, shows_path,
};
use eframe::egui::accesskit::{self, Role, Toggled};
use eframe::egui::epaint::Shape;
use eframe::egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, OutputCommand, Pos2, Rect, TouchPhase,
    Vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable, by};
use salvajocs_core::backup::backup_games;
use salvajocs_core::config::{AppConfig, ListSort, SortKey};
use salvajocs_core::custom_games::{self, Stored};
use salvajocs_core::launcher::{Launcher, LauncherPrefix};
use salvajocs_core::layout::{Base, Context, LinuxFolders, PrefixOrigin};
use salvajocs_core::manifest::{
    self, CustomGame, Integration, Manifest, Pair, RegistryRule, parse_yaml,
};
use salvajocs_core::scan::{Environment, FoundFile, GameScan, RegistrySource};
use salvajocs_core::selection::{self, Change, Pending};
use salvajocs_core::storage::{RepoLocation, SnapshotInfo, Storage};
use salvajocs_core::sync::{self, Base as SyncBase};
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

#[cfg(unix)]
#[test]
fn case_variants_of_one_save_restore_once_into_a_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    // A Linux-side prefix backup can hold both; the prefix has `save.dat`, which both reuse.
    let snapshot = scan(
        &tmp,
        "Alpha",
        &[
            (Base::WinAppData, "Alpha/Save.dat"),
            (Base::WinAppData, "Alpha/save.dat"),
        ],
        &[(Base::WinAppData, "Alpha")],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    put(&saves.join("save.dat"), "old");
    select(&mut harness, "Alpha");
    preview(&mut harness);

    assert!(shows(&harness, "Files to write (1)"));
    assert!(shows(&harness, "Skipped (1)"));
    assert!(shows(
        &harness,
        ": another file for the same target is newer"
    ));
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

const WINDOW_HEIGHT: f32 = 500.0;

/// The Restore tab listing 40 games in a window too short for them.
fn long_list(tmp: &TempDir) -> (Harness<'static, App>, Vec<String>) {
    let titles: Vec<String> = (0..40).map(|n| format!("Game {n:02}")).collect();
    let games: Vec<GameScan> = (titles.iter())
        .map(|title| {
            let tail = format!("{title}/save.dat");
            scan(
                tmp,
                title,
                &[(Base::WinAppData, &tail)],
                &[(Base::WinAppData, title)],
            )
        })
        .collect();
    let (mut harness, _env) = open_tab(tmp, Vec::new(), &games);
    harness.set_size(Vec2::new(1000.0, WINDOW_HEIGHT));
    run_settled(&mut harness);
    (harness, titles)
}

/// The bounds of the node labelled `label` in the last frame.
fn bounds(harness: &Harness<'static, App>, label: &str) -> accesskit::Rect {
    (harness.get_by_label(label).accesskit_node().raw_bounds()).unwrap()
}

/// Turns the mouse wheel down over the sidebar, as a user does.
fn wheel_over_sidebar(harness: &mut Harness<'static, App>) {
    let at = Pos2::new(
        harness.get_by_label("Load backups").rect().center().x,
        WINDOW_HEIGHT / 2.0,
    );
    harness.hover_at(at);
    harness.step();
    harness.event(Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -2000.0),
        phase: TouchPhase::Move,
        source: MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    run_settled(harness);
}

#[test]
fn restore_sidebar_scrolls_a_list_longer_than_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, titles) = long_list(&tmp);
    let window = f64::from(WINDOW_HEIGHT);
    let last = titles.last().unwrap();
    assert!(
        bounds(&harness, last).y0 > window,
        "the last game starts below the window"
    );
    let load_top = bounds(&harness, "Load backups").y0;

    harness.get_by_label(last).scroll_to_me();
    run_settled(&mut harness);
    let button = bounds(&harness, "Restore 0 games");
    assert!(button.y1 <= window, "the batch button stays in the window");
    assert!(
        bounds(&harness, last).y1 <= button.y0,
        "the last game scrolls into view above the batch button"
    );
    assert_eq!(
        bounds(&harness, "Load backups").y0,
        load_top,
        "the sidebar's controls stay put"
    );
}

#[test]
fn restore_sidebar_scrolls_with_the_wheel_while_a_batch_is_open() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, titles) = long_list(&tmp);
    click(&mut harness, "Include Game 00");
    plan_batch(&mut harness, "Restore 1 game");
    let last = titles.last().unwrap();
    assert!(
        bounds(&harness, last).y0 > f64::from(WINDOW_HEIGHT),
        "the last game starts below the window"
    );

    wheel_over_sidebar(&mut harness);
    assert!(
        bounds(&harness, last).y1 <= f64::from(WINDOW_HEIGHT),
        "the wheel brings the last game into the window"
    );
}

#[test]
fn restore_sidebar_keeps_the_batch_button_under_a_short_list() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _saves) = alpha(&tmp);
    run_settled(&mut harness);
    let row = bounds(&harness, "Alpha");
    let button = bounds(&harness, "Restore 0 games");
    let gap = button.y0 - row.y1;
    assert!(gap < 100.0, "the button sits right under the list: {gap}");
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
fn restore_browse_asks_the_env_for_the_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let other = tmp.path().join("other-pfx");
    let (mut harness, _env, _) = alpha_with(&tmp, |env| pick(env, other.clone()));
    select(&mut harness, "Alpha");
    preview(&mut harness);

    harness.get_by_label("Browse").click();

    run_until(&mut harness, "the picked prefix", |h| {
        value(h, "Prefix") == other.to_str().unwrap()
    });
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
            .filter(|node| node.accesskit_node().role() == Role::CheckBox)
            .count();
        let buttons = harness
            .query_all_by_label_contains("Override XdgData/")
            .count();
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
fn manifest_paths_lists_the_templates_and_copies_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _saves) = alpha(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(!shows(&harness, "Save paths of Alpha"));

    click(&mut harness, "Manifest paths");
    assert!(shows(&harness, "Save paths of Alpha"));
    assert!(shows(&harness, "Custom game."));
    assert!(shows(&harness, "Files (1)"));
    assert!(shows(&harness, "Holds save"));

    let template = "<winAppData>/Alpha";
    harness.get_by_label(template).click_secondary();
    run_settled(&mut harness);
    harness.get_by_label("Copy").click();
    // The command lasts one frame, so read it right after the click is handled.
    harness.step();
    let copied: Vec<_> = (harness.output().platform_output.commands.iter())
        .filter_map(|command| match command {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(copied, [template]);
    run_settled(&mut harness);
    assert!(shows(&harness, "Save paths of Alpha"));

    harness.key_press(Key::Escape);
    run_settled(&mut harness);
    assert!(!shows(&harness, "Save paths of Alpha"));
}

#[test]
fn manifest_paths_is_offered_beside_choose_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env) = mix(&tmp);
    click(&mut harness, "Cancel");

    click(&mut harness, "Manifest paths");
    assert!(shows(&harness, "Save paths of Mix"));
    harness.key_press(Key::Escape);
    run_settled(&mut harness);
    assert!(!shows(&harness, "Save paths of Mix"));
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
    // The safety backup was staged apart from the backup mirror, which holds this PC's saves
    // after the restore, staged for the base (ADR-0035).
    assert!(env.cache_dir.join("restore-stage").exists());
    let mirror = env
        .cache_dir
        .join("stage")
        .join(salvajocs_core::slug::slug("Alpha"));
    let staged = under(&mirror, "files/WinAppData/Alpha/save.dat");
    assert_eq!(fs::read_to_string(staged).unwrap(), "Alpha/save.dat");

    // The reloaded list offers the safety backup, but not as the default, and the game stays
    // chosen without a click.
    run_until(&mut harness, "the list to reload", |h| {
        !disabled(h, "Load backups")
    });
    run_until(&mut harness, "the game to stay chosen", |h| {
        shows(h, "Restore into:")
    });
    assert!(!value(&harness, "Snapshot").contains("pre-restore"));
    click(&mut harness, "Snapshot");
    assert!(shows(&harness, ", pre-restore"));
}

#[test]
fn a_restore_removes_the_dirs_earlier_sessions_left() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, _saves) = alpha(&tmp);
    // What an earlier session left a day and a half ago, and a dir salvajocs did not make.
    for parent in ["restore-work", "restore-stage", "restore-batch"] {
        let parent = env.cache_dir.join(parent);
        put(&parent.join("99999999/x"), "stale");
        age(&parent.join("99999999"));
        put(&parent.join("keep/x"), "not ours");
        age(&parent.join("keep"));
    }
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(env.cache_dir.join("restore-work/99999999").exists());

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    for parent in ["restore-work", "restore-stage", "restore-batch"] {
        let parent = env.cache_dir.join(parent);
        assert!(!parent.join("99999999").exists());
        assert!(parent.join("keep/x").exists());
    }
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
    // The kept prefix is not where this snapshot came from.
    assert!(!shows(&harness, "Filled in from the backup"));
    assert!(!shows(&harness, "(from the backup)"));
}

#[test]
fn restore_says_the_prefix_came_from_the_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic])
    });
    select(&mut harness, "Tower Wizard");
    preview(&mut harness);

    let bottle = "the Bottles bottle Generic-Game";
    assert!(shows(
        &harness,
        &format!("Filled in from the backup ({bottle})")
    ));
    assert!(shows(&harness, &format!("Into {bottle} (from the backup)")));
}

#[test]
fn restore_says_it_found_the_saves_in_the_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    put(&tower_save(&generic), "older");
    let (game, snapshot) = tower(&tmp, None);
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic])
    });
    select(&mut harness, "Tower Wizard");
    preview(&mut harness);

    let text = "Filled in: the backup's save folders are in this prefix (the Bottles bottle \
                Generic-Game)";
    assert!(shows(&harness, text));
    assert!(shows(&harness, "(its saves are there)"));
}

#[test]
fn restore_says_the_backups_prefix_is_not_here() {
    let tmp = tempfile::tempdir().unwrap();
    let (game, snapshot) = tower(&tmp, Some("Gone"));
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    select(&mut harness, "Tower Wizard");

    let text = "The backup came from the Bottles bottle Gone, which this machine does not have \
                or does not use.";
    assert!(shows(&harness, text));
}

#[test]
fn restore_names_the_prefixes_when_several_hold_the_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let first = bottle(&tmp, "First");
    let second = bottle(&tmp, "Second");
    put(&tower_save(&first), "first");
    put(&tower_save(&second), "second");
    let (game, snapshot) = tower(&tmp, None);
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&first, &second])
    });
    select(&mut harness, "Tower Wizard");

    let names = format!("{}, {}", first.display(), second.display());
    let text = format!("The backup's save folders are in 2 prefixes: {names}.");
    assert!(shows_path(&harness, &text));
    assert_eq!(value(&harness, "Prefix"), "");
}

#[test]
fn restore_names_the_prefixes_the_backups_prefix_name_fits() {
    let tmp = tempfile::tempdir().unwrap();
    let native = bottle(&tmp, "Generic-Game");
    let flatpak_dir = ".var/app/com.usebottles.bottles/data/bottles/bottles/Generic-Game";
    let flatpak = under(&home(&tmp), flatpak_dir);
    fs::create_dir_all(flatpak.join("drive_c/users/steamuser")).unwrap();
    fs::write(flatpak.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&native, &flatpak])
    });
    select(&mut harness, "Tower Wizard");

    let names = format!("{}, {}", native.display(), flatpak.display());
    let text = format!("The backup's prefix name fits 2 prefixes: {names}.");
    assert!(shows_path(&harness, &text));
    assert!(!shows(&harness, "save folders are in"));
    assert_eq!(value(&harness, "Prefix"), "");
}

#[test]
fn restore_reason_goes_when_the_prefix_is_edited() {
    let tmp = tempfile::tempdir().unwrap();
    let generic = bottle(&tmp, "Generic-Game");
    let other = bottle(&tmp, "Other");
    let (game, snapshot) = tower(&tmp, Some("Generic-Game"));
    let (mut harness, _env) = open_tab_with(&tmp, vec![game], &[snapshot], &[], |env| {
        found_bottles(env, &[&generic, &other])
    });
    select(&mut harness, "Tower Wizard");
    preview(&mut harness);
    assert!(shows(&harness, "Filled in from the backup"));

    replace_text(&mut harness, "Prefix", other.to_str().unwrap());
    assert!(!shows(&harness, "Filled in from the backup"));
    harness.key_press(Key::Enter);
    run_until(&mut harness, "the plan", |h| {
        shows(h, "Into the Bottles bottle Other")
    });
    assert!(!shows(&harness, "(from the backup)"));
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
fn restore_mapping_choice_and_restore_write_no_settings_until_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env) = mix(&tmp);
    let settings = || open_repo(&tmp).snapshots("salvajocs-settings").unwrap();
    assert!(settings().is_empty(), "the open writes nothing");
    replace_text(&mut harness, "Target 2", "<xdgData>/Mix");
    harness.get_by_label("Use these folders").click();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write (2)"));

    let games = run_until_saved(&mut harness, &env, |games| {
        games.iter().any(|game| !game.pairs.is_empty())
    });
    assert!(settings().is_empty(), "the choice stays on this PC");
    // The safety backup of a file the restore replaces sends nothing either.
    let target = under(&home(&tmp), ".local/share/Mix/a.sav");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "old").unwrap();
    preview(&mut harness);
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Mix"));
    assert!(settings().is_empty(), "the restore writes no settings");
    assert!(!snapshots(&tmp, "pre-restore").is_empty());

    back_up_unsent(&mut harness, "Saved 1 custom game to the repository");
    // Another PC reads the same mapping from the repository (ADR-0028).
    let stored = custom_games::load_all(&mut open_repo(&tmp)).unwrap();
    let Ok(Stored::Game(game)) = &stored["Mix"] else {
        panic!("Mix is not in the repository: {stored:?}");
    };
    assert_eq!(game.pairs, games[0].pairs);
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
    // The game stays chosen, and the count of the delete stays shown.
    run_until(&mut harness, "the game to stay chosen", |h| {
        shows(h, "Restore into:")
    });
    assert!(shows(&harness, "Removed 1 backup"));
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

#[test]
fn escape_cancels_a_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    let before = snapshots(&tmp, "salvajocs");

    click(&mut harness, "Delete all backups of Alpha");
    assert!(shows(&harness, "It cannot be undone"));
    harness.key_press(Key::Escape);
    run_settled(&mut harness);

    assert!(!shows(&harness, "It cannot be undone"));
    assert_eq!(snapshots(&tmp, "salvajocs"), before);
}

#[test]
fn clicking_outside_cancels_a_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = two_games(&tmp);
    let before = snapshots(&tmp, "salvajocs");

    click(&mut harness, "Delete all backups of Alpha");
    assert!(shows(&harness, "It cannot be undone"));
    click_at(&mut harness, Pos2::new(5.0, 800.0));
    run_settled(&mut harness);

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

/// Escape in the mapping dialog of a batch does not drop the batch.
#[test]
fn escape_keeps_the_batch_mapping_dialog_open() {
    let tmp = tempfile::tempdir().unwrap();
    let (kilo, kilo_scan) = unmapped(&tmp, "Kilo");
    let (mut harness, _env) = open_tab(&tmp, vec![kilo], &[kilo_scan]);

    click(&mut harness, "Select all");
    harness.get_by_label("Restore 1 game").click();
    run_until_dialog(&mut harness, "Kilo");
    harness.key_press(Key::Escape);
    run_settled(&mut harness);

    assert!(shows(&harness, "Where should the saves of Kilo go?"));
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
    let works = env.cache_dir.join("restore-work");
    put(&works.join("99999999/x"), "stale");
    age(&works.join("99999999"));
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
    assert!(!works.join("99999999").exists());
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

/// The Restore tab with Alpha backed up and no custom game for it here, after a read found the
/// custom game another PC saved for it: an offer, not used here.
fn alpha_offered(tmp: &TempDir) -> Harness<'static, App> {
    let (alpha, snapshot) = native(tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, _env) = open_tab(tmp, Vec::new(), &[snapshot]);
    let stage = tmp.path().join("elsewhere");
    custom_games::save(&mut open_repo(tmp), "Alpha", None, Some(&alpha), &stage).unwrap();
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    harness.get_by_label("Read repository").click();
    run_until(&mut harness, "the offer", |h| shows(h, "Alpha (new)"));
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    harness
}

/// What the plan says of a game whose custom game from another PC waits for review.
const OFFER_NOTE: &str = "This restore uses this PC's definition only.";

#[test]
fn restore_of_a_game_with_an_offer_points_to_custom_games() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = alpha_offered(&tmp);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(shows(&harness, OFFER_NOTE));
    // The offer declares nothing here, so the save is skipped, not written.
    assert!(shows(&harness, "Files to write (0)"));
    assert!(shows(&harness, "Skipped (1)"));
}

#[test]
fn restore_of_a_game_whose_offer_is_set_aside_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = alpha_offered(&tmp);
    harness.state_mut().set_tab(Tab::CustomGames);
    run_settled(&mut harness);
    click(&mut harness, "Not now for Alpha");
    harness.state_mut().set_tab(Tab::Restore);
    run_settled(&mut harness);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(shows(&harness, "is set aside in Custom games"));
    assert!(shows(&harness, OFFER_NOTE));
    assert!(!shows(&harness, "waits for review"));
}

#[test]
fn batch_restore_of_a_game_with_an_offer_points_to_custom_games() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = alpha_offered(&tmp);
    click(&mut harness, "Include Alpha");
    plan_batch(&mut harness, "Restore 1 game");
    run_until(&mut harness, "the batch plan", |h| {
        shows(h, "Alpha, snapshot")
    });
    assert!(shows(&harness, OFFER_NOTE));
    assert!(shows(&harness, "Skipped (1)"));
    assert!(shows(
        &harness,
        "Nothing to restore, so this game will not be restored."
    ));
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
    let into = "Into the Bottles bottle Generic-Game (from the backup)";
    assert!(shows(&harness, into));
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");

    let restored = fs::read_to_string(tower_save(&generic)).unwrap();
    assert_eq!(restored, format!("{TOWER}/save.tres"));
}

#[test]
fn into_line_names_a_proton_prefix_by_app_id() {
    let tmp = tempfile::tempdir().unwrap();
    let pfx = under(&home(&tmp), ".steam/steam/steamapps/compatdata/1868140/pfx");
    put(&pfx.join("user.reg"), "WINE REGISTRY Version 2\n");
    fs::create_dir_all(pfx.join("drive_c/users/steamuser")).unwrap();
    let source = tmp.path().join("source-pfx");
    put(
        &source.join("user.reg"),
        "WINE REGISTRY Version 2\n\n[Software\\\\Co\\\\Dave] 1\n#time=0\n\"v\"=\"saved\"\n",
    );
    let mut dave = custom("Dave", &["<winAppData>/Dave"], Some(&pfx), Vec::new());
    let key = "HKEY_CURRENT_USER/Software/Co/Dave".to_owned();
    dave.entry.registry.insert(key, RegistryRule::default());
    let mut snapshot = scan(
        &tmp,
        "Dave",
        &[(Base::WinAppData, "Dave/save.dat")],
        &[(Base::WinAppData, "Dave")],
    );
    snapshot.registry_keys = vec![r"HKEY_CURRENT_USER\Software\Co\Dave".into()];
    snapshot.registry_source = Some(RegistrySource::WinePrefix(source));
    let (mut harness, _env) = open_tab(&tmp, vec![dave], &[snapshot]);

    click(&mut harness, "Include Dave");
    plan_batch(&mut harness, "Restore 1 game");

    let proton = "the Steam Proton prefix of app 1868140";
    assert!(shows(&harness, &format!("Into {proton} (custom game)")));
    let registry = format!("Registry: 1 keys into {proton}, replaced");
    assert!(shows(&harness, &registry));
    let path = pfx.display().to_string();
    assert!(!shows_path(&harness, &path), "the path shows on hover only");
    harness
        .get_by_label_contains("Into the Steam Proton")
        .hover();
    run_until(&mut harness, "the prefix's path", move |h| {
        shows_path(h, &path)
    });
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
        .backup_dir(&stage, &as_path, &tags, "Alpha", true)
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
fn batch_confirm_tree_labels_each_file_with_a_checkbox() {
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
        let node = harness.get_by_label(row);
        let node = node.accesskit_node();
        assert_eq!(node.role(), Role::CheckBox, "{row}");
        assert_eq!(node.toggled(), Some(Toggled::True), "{row} starts ticked");
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
    assert!(shows(&harness, "the safety backup cannot read these files"));
    assert!(shows_path(&harness, &locked.display().to_string()));
    assert!(shows(
        &harness,
        "Untick them to restore the rest of this game."
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

/// A Wine prefix in a folder as long as Windows' temp paths, which once pushed the details
/// dialog's Close out of sight.
fn long_prefix(tmp: &TempDir) -> PathBuf {
    let long = "a folder whose name makes the path of the save wider than the details dialog";
    let prefix = tmp.path().join(long).join("pfx");
    fs::create_dir_all(prefix.join("drive_c/users/steamuser")).unwrap();
    fs::write(prefix.join("user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    prefix
}

/// A scan of Alpha with `save.dat` holding `saved`, a `same.dat` and a `slot2.dat`.
fn alpha_scan(tmp: &TempDir, saved: &str) -> GameScan {
    let mut snapshot = scan(
        tmp,
        "Alpha",
        &[
            (Base::WinAppData, "Alpha/save.dat"),
            (Base::WinAppData, "Alpha/same.dat"),
            (Base::WinAppData, "Alpha/slot2.dat"),
        ],
        &[(Base::WinAppData, "Alpha")],
    );
    fs::write(&snapshot.files[0].real, saved).unwrap();
    snapshot.files[0].size = saved.len() as u64;
    snapshot
}

/// Alpha backed up with `save.dat` holding `saved`, a `slot2.dat` the target lacks and a
/// `same.dat` the target already has; `save.dat` is then written with `local`.
fn alpha_differing(
    tmp: &TempDir,
    saved: &str,
    local: &str,
) -> (Harness<'static, App>, Env, PathBuf) {
    let prefix = long_prefix(tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let (mut harness, env) = open_tab(tmp, vec![game], &[alpha_scan(tmp, saved)]);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    fs::create_dir_all(&saves).unwrap();
    fs::write(saves.join("save.dat"), local).unwrap();
    fs::write(saves.join("same.dat"), "Alpha/same.dat").unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    (harness, env, saves)
}

#[test]
fn a_changed_file_of_the_plan_has_details() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha_differing(&tmp, "one\ntwo\nthree\n", "one\n2\nthree\n");

    harness
        .get_by_label(&format!("Details of {ALPHA_SAVE}"))
        .click();
    run_until(&mut harness, "the details", |h| shows(h, "− two"));
    run_settled(&mut harness);

    assert!(shows(&harness, "+ 2"));
    assert!(shows(&harness, "Location"));
    assert!(shows_path(
        &harness,
        &saves.join("save.dat").display().to_string()
    ));
    // The backup's path below `files/`, which is also this file's key.
    assert!(shows(&harness, "WinAppData/Alpha/save.dat"));
    let backup = "b6285c57e8797db5d4c51c80d6f11938afda9b11c6a003549709189e9b4b92a2";
    let local = "bc85caa9b61bcf3a54ccfc800e2b0eda6c11fa5df4e0481c896d31cda2462eb2";
    assert!(shows(&harness, backup) && shows(&harness, local));
    assert!(shows(&harness, "from its hash list"));
    assert!(!shows(&harness, "Reading the backup"));

    click(&mut harness, "Close details");
    assert!(!shows(&harness, "Location"));
    assert!(harness.query_by_label("Close details").is_none());
}

#[test]
fn only_a_changed_file_of_the_plan_has_a_details_button() {
    let tmp = tempfile::tempdir().unwrap();
    let (harness, _env, _saves) = alpha_differing(&tmp, "saved\n", "local\n");

    for row in ["save.dat, changed", "same.dat", "slot2.dat, new"] {
        assert_eq!(harness.query_all_by_label(row).count(), 1, "{row}");
    }
    // Not on the new file nor on the unchanged one.
    assert_eq!(harness.query_all_by_label_contains("Details of").count(), 1);
    let name = format!("Details of {ALPHA_SAVE}");
    let button = harness.get_by_label(&name);
    assert_eq!(button.accesskit_node().role(), Role::Button);
}

/// Sets the modification time of `path` to `secs` seconds after the Unix epoch.
fn set_modified(path: &Path, secs: u64) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

#[test]
fn details_of_a_big_backup_with_no_hash_list_record_no_hash_and_mark_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, _) = native(&tmp, "Alpha", &[]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[beta_scan]);
    // A stage as 0.5.0 made it: no hashes.json. Its 10.5 MiB file is over the cap, so the
    // backup's copy is not read and its hash is unknown.
    let stage = tmp.path().join("old-stage");
    let meta = r#"{"format":1,"title":"Alpha","roots":[["XdgData","Alpha"]]}"#;
    put(&stage.join("salvajocs.json"), meta);
    let backup_copy = stage.join("files/XdgData/Alpha/save.dat");
    put(&backup_copy, &"0123456789abcdef\n".repeat(650_000));
    set_modified(&backup_copy, 1_700_000_000);
    let slug = salvajocs_core::slug::slug("Alpha");
    let (as_path, game_tag) = (format!("/salvajocs/{slug}"), format!("game:{slug}"));
    let tags = ["salvajocs", game_tag.as_str()];
    open_repo(&tmp)
        .backup_dir(&stage, &as_path, &tags, "Alpha", true)
        .unwrap()
        .unwrap();
    let local = under(&xdg_data(&tmp), "Alpha/save.dat");
    put(&local, "small\n");
    set_modified(&local, 1_700_000_100);
    click(&mut harness, "Load backups");
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open XdgData/Alpha");

    harness
        .get_by_label("Details of XdgData/Alpha/save.dat")
        .click();
    run_until(&mut harness, "the details", |h| {
        shows(h, "not recorded in this backup")
    });

    // The size and the modified time differ; the unknown hash has no mark.
    assert_eq!(harness.query_all_by_label("Differs").count(), 2);
    assert!(shows(
        &harness,
        "Diff not shown: the backup's copy is 10.5 MiB"
    ));
}

#[test]
fn the_details_of_the_plan_show_that_only_line_endings_differ() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, _saves) = alpha_differing(&tmp, "a\nb\n", "a\r\nb\r\n");

    harness
        .get_by_label(&format!("Details of {ALPHA_SAVE}"))
        .click();
    run_until(&mut harness, "the details", |h| {
        shows(h, "Only the line endings differ.")
    });

    assert!(shows(&harness, "2 Windows (CRLF), 0 Linux (LF)"));
    assert!(shows(&harness, "0 Windows (CRLF), 2 Linux (LF)"));
}

#[test]
fn the_details_of_an_overridden_file_compare_the_new_target() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _env, saves) = alpha_differing(&tmp, "one\ntwo\nthree\n", "one\n2\nthree\n");
    let other = tmp.path().join("other.dat");
    fs::write(&other, "one\nTWO\nthree\n").unwrap();

    apply_override(&mut harness, ALPHA_SAVE, &other);
    harness
        .get_by_label(&format!("Details of {ALPHA_SAVE}"))
        .click();
    run_until(&mut harness, "the details", |h| shows(h, "+ TWO"));

    assert!(shows(&harness, "− two"));
    assert!(shows_path(&harness, &other.display().to_string()));
    let replaced = saves.join("save.dat").display().to_string();
    assert!(!shows_path(&harness, &replaced));
    assert!(!shows(&harness, "+ 2"));
}

#[test]
fn the_details_of_the_plan_read_the_chosen_older_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = long_prefix(&tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let older = alpha_scan(&tmp, "one\ntwo\nthree\n");
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[older]);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    fs::create_dir_all(&saves).unwrap();
    fs::write(saves.join("save.dat"), "one\n2\nthree\n").unwrap();
    // A newer backup of the same game, which the plan starts with.
    let newer = alpha_scan(&tmp, "one\nTWO\nthree\n");
    let stage = tmp.path().join("fixture-stage");
    backup_games(&mut open_repo(&tmp), &[newer], &stage, &[], None, |_| {}).unwrap();
    click(&mut harness, "Load backups");
    select(&mut harness, "Alpha");
    preview(&mut harness);

    harness.get_by_label("Snapshot").click();
    harness.step();
    harness.step();
    let mut options = harness.query_all_by_label_contains(" UTC, ");
    options.nth(1).expect("both snapshots are listed").click();
    drop(options);
    harness.step();
    run_until(&mut harness, "the plan", |h| shows(h, "Files to write"));
    click(&mut harness, "Open WinAppData/Alpha");
    harness
        .get_by_label(&format!("Details of {ALPHA_SAVE}"))
        .click();
    run_until(&mut harness, "the details", |h| shows(h, "+ 2"));

    assert!(shows(&harness, "− two"));
    assert!(!shows(&harness, "− TWO"));
    // The older save's hash, not the newer one's.
    let hash = "b6285c57e8797db5d4c51c80d6f11938afda9b11c6a003549709189e9b4b92a2";
    assert!(shows(&harness, hash));
}

/// Whether the checkbox `label` is ticked.
fn ticked(harness: &Harness<'static, App>, label: &str) -> bool {
    let node = harness.get_by_label(label);
    node.accesskit_node().toggled() == Some(Toggled::True)
}

/// The paths below the game's folder in the safety backup of `title`; `None` without one.
fn safety_entries(tmp: &TempDir, title: &str) -> Option<Vec<String>> {
    let safety = snapshots(tmp, "pre-restore");
    let snapshot = safety.iter().find(|snapshot| snapshot.label == title)?;
    let path = format!("/salvajocs/{}", salvajocs_core::slug::slug(title));
    let entries = open_repo(tmp)
        .snapshot_entries(&snapshot.id, &path)
        .unwrap();
    Some(entries.into_iter().map(|entry| entry.path).collect())
}

/// Alpha, a Windows build in a prefix with a long path, with `save.dat` and `slot2.dat` backed
/// up; each of `existing` already holds `old` at its target. Planned, with its folder open.
fn alpha_planned(tmp: &TempDir, existing: &[&str]) -> (Harness<'static, App>, PathBuf) {
    let prefix = long_prefix(tmp);
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let snapshot = scan(
        tmp,
        "Alpha",
        &[
            (Base::WinAppData, "Alpha/save.dat"),
            (Base::WinAppData, "Alpha/slot2.dat"),
        ],
        &[(Base::WinAppData, "Alpha")],
    );
    let (mut harness, _env) = open_tab(tmp, vec![game], &[snapshot]);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    for name in existing {
        put(&saves.join(name), "old");
    }
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    (harness, saves)
}

#[test]
fn an_unticked_file_is_neither_restored_nor_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, saves) = alpha_planned(&tmp, &["slot2.dat"]);
    assert!(ticked(&harness, "slot2.dat, changed"));

    click(&mut harness, "slot2.dat, changed");
    assert!(!ticked(&harness, "slot2.dat, changed"));
    assert!(shows(&harness, "Files to write (1)"));
    assert!(shows(&harness, "1 left out"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(shows(&harness, "Safety backup: none needed"));
    assert!(snapshots(&tmp, "pre-restore").is_empty());
    assert_eq!(fs::read_to_string(saves.join("slot2.dat")).unwrap(), "old");
    let save = fs::read_to_string(saves.join("save.dat")).unwrap();
    assert_eq!(save, "Alpha/save.dat");
    let slot2 = saves.join("slot2.dat").display().to_string();
    assert!(!shows_path(&harness, &slot2), "no result line for it");
}

#[test]
fn the_safety_backup_holds_only_the_ticked_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, saves) = alpha_planned(&tmp, &["save.dat", "slot2.dat"]);

    click(&mut harness, "slot2.dat, changed");
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    let entries = safety_entries(&tmp, "Alpha").expect("a safety backup");
    assert!(
        entries.iter().any(|path| path.ends_with("Alpha/save.dat")),
        "{entries:?}"
    );
    assert!(
        !entries.iter().any(|path| path.ends_with("slot2.dat")),
        "{entries:?}"
    );
    assert_eq!(fs::read_to_string(saves.join("slot2.dat")).unwrap(), "old");
    let save = fs::read_to_string(saves.join("save.dat")).unwrap();
    assert_eq!(save, "Alpha/save.dat");
}

#[test]
fn a_folder_counts_its_ticked_files_and_restore_needs_one() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _saves) = alpha_planned(&tmp, &[]);
    assert!(shows(&harness, "2 of 2 files"));

    click(&mut harness, "slot2.dat, new");
    assert!(shows(&harness, "1 of 2 files"));
    assert!(!disabled(&harness, "Restore files"));

    // A mixed folder ticks everything, the next click nothing.
    click(&mut harness, "WinAppData/Alpha");
    assert!(shows(&harness, "2 of 2 files"));
    click(&mut harness, "WinAppData/Alpha");
    assert!(shows(&harness, "0 of 2 files"));
    assert!(disabled(&harness, "Restore files"));
    assert!(shows(&harness, "Tick at least one file to restore."));

    click(&mut harness, "WinAppData/Alpha");
    assert!(shows(&harness, "2 of 2 files"));
    assert!(!disabled(&harness, "Restore files"));
    assert!(!shows(&harness, "Tick at least one"));
}

#[cfg(unix)]
#[test]
fn unticking_an_unreadable_target_lets_the_restore_run() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let prefix = long_prefix(&tmp);
    let saves = under(&prefix, "drive_c/users/steamuser/AppData/Roaming/Alpha");
    let locked = saves.join("save.dat");
    put(&locked, "old");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let unlock = Unlock(locked.clone(), 0o644);
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }
    let game = custom("Alpha", &["<winAppData>/Alpha"], Some(&prefix), Vec::new());
    let snapshot = scan(
        &tmp,
        "Alpha",
        &[
            (Base::WinAppData, "Alpha/save.dat"),
            (Base::WinAppData, "Alpha/slot2.dat"),
        ],
        &[(Base::WinAppData, "Alpha")],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    assert!(shows(&harness, "Nothing will be restored"));
    assert!(shows(&harness, "Untick them, or make them readable"));
    assert!(disabled(&harness, "Restore files"));

    click(&mut harness, "Open WinAppData/Alpha");
    click(&mut harness, "save.dat, will fail");
    assert!(!shows(&harness, "Nothing will be restored"));
    assert!(!disabled(&harness, "Restore files"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));
    drop(unlock);

    assert_eq!(fs::read_to_string(&locked).unwrap(), "old");
    let slot2 = fs::read_to_string(saves.join("slot2.dat")).unwrap();
    assert_eq!(slot2, "Alpha/slot2.dat");
}

#[test]
fn a_new_plan_ticks_every_file_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, _saves) = alpha_planned(&tmp, &[]);
    click(&mut harness, "slot2.dat, new");
    assert!(shows(&harness, "1 left out"));

    // Other targets plan again.
    click(&mut harness, "Native Linux build");
    preview(&mut harness);
    if harness.query_by_label("Open WinAppData/Alpha").is_some() {
        click(&mut harness, "Open WinAppData/Alpha");
    }

    assert!(ticked(&harness, "slot2.dat, new"));
    assert!(ticked(&harness, "save.dat, new"));
    assert!(!shows(&harness, "left out"));
}

#[test]
fn an_unticked_overridden_file_stays_unticked_and_is_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, saves) = alpha_planned(&tmp, &[]);
    let elsewhere = under(tmp.path(), "elsewhere/slot2.dat");
    click(&mut harness, "slot2.dat, new");

    apply_override(&mut harness, ALPHA_SLOT2, &elsewhere);
    assert!(!ticked(&harness, "slot2.dat, new"));
    click(
        &mut harness,
        &format!("Clear the override of {ALPHA_SLOT2}"),
    );
    assert!(!ticked(&harness, "slot2.dat, new"));
    apply_override(&mut harness, ALPHA_SLOT2, &elsewhere);
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(!elsewhere.exists());
    assert!(!saves.join("slot2.dat").exists());
    assert!(saves.join("save.dat").exists());
}

const REG_KEY: &str = r"HKEY_CURRENT_USER\Software\Co\Reg";
const OTHER_KEY: &str = r"HKEY_CURRENT_USER\Software\Co\Other";

/// A `user.reg` holding `Software\Co\Other` and `Software\Co\Reg`, whose values `v` are
/// `<age>-other` and `<age>-reg`.
fn co_user_reg(age: &str) -> String {
    format!(
        "WINE REGISTRY Version 2\n\n[Software\\\\Co\\\\Other] 1\n#time=0\n\"v\"=\"{age}-other\"\n\n\
         [Software\\\\Co\\\\Reg] 1\n#time=0\n\"v\"=\"{age}-reg\"\n"
    )
}

/// Reg, a Windows build in a prefix with a long path that declares the keys `Software\Co\Reg`
/// and `Software\Co\Other`, backed up with a save and `new` values while the prefix holds both
/// keys with `old` values; and Beta, a native game. `wineserver_running` answers the wineserver
/// check. Returns the prefix.
fn registry_games(
    tmp: &TempDir,
    wineserver_running: fn() -> bool,
) -> (Harness<'static, App>, PathBuf) {
    let prefix = long_prefix(tmp);
    put(&prefix.join("user.reg"), &co_user_reg("old"));
    let source = tmp.path().join("source-pfx");
    put(&source.join("user.reg"), &co_user_reg("new"));
    let mut reg = custom("Reg", &["<winAppData>/Reg"], Some(&prefix), Vec::new());
    for key in ["Reg", "Other"] {
        let key = format!("HKEY_CURRENT_USER/Software/Co/{key}");
        reg.entry.registry.insert(key, RegistryRule::default());
    }
    let mut reg_scan = scan(
        tmp,
        "Reg",
        &[(Base::WinAppData, "Reg/save.dat")],
        &[(Base::WinAppData, "Reg")],
    );
    reg_scan.registry_keys = vec![REG_KEY.into(), OTHER_KEY.into()];
    reg_scan.registry_source = Some(RegistrySource::WinePrefix(source));
    let (beta, beta_scan) = native(tmp, "Beta", &["Beta/save.dat"]);
    let games = vec![reg, beta];
    let (harness, _env) = open_tab_with(tmp, games, &[reg_scan, beta_scan], &[], |env| {
        env.wineserver_running = wineserver_running
    });
    (harness, prefix)
}

/// Where Reg's save goes in `prefix`.
fn reg_save(prefix: &Path) -> PathBuf {
    under(
        prefix,
        "drive_c/users/steamuser/AppData/Roaming/Reg/save.dat",
    )
}

#[test]
fn an_unticked_registry_key_is_neither_restored_nor_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = registry_games(&tmp, || false);
    select(&mut harness, "Reg");
    preview(&mut harness);
    assert!(shows(&harness, "Registry: 2 keys into"));
    assert!(ticked(&harness, REG_KEY) && ticked(&harness, OTHER_KEY));

    click(&mut harness, OTHER_KEY);
    assert!(shows(&harness, "Registry: 1 keys into"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Reg"));

    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    assert!(user_reg.contains("new-reg") && !user_reg.contains("old-reg"));
    assert!(user_reg.contains("old-other") && !user_reg.contains("new-other"));
    let safety = snapshots(&tmp, "pre-restore");
    let path = format!(
        "/salvajocs/{}/registry.json",
        salvajocs_core::slug::slug("Reg")
    );
    let saved = open_repo(&tmp)
        .read_file(&safety[0].id, &path, u64::MAX)
        .unwrap();
    let saved = String::from_utf8(saved).unwrap();
    assert!(saved.contains("old-reg"), "{saved}");
    assert!(!saved.contains("Other"), "{saved}");
}

#[test]
fn unticking_every_registry_key_restores_files_while_wine_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = registry_games(&tmp, || true);
    select(&mut harness, "Reg");
    preview(&mut harness);

    click(&mut harness, REG_KEY);
    click(&mut harness, OTHER_KEY);
    assert!(shows(&harness, "Registry: 0 keys into"));
    assert!(!disabled(&harness, "Restore files"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Reg"));

    assert!(!shows(&harness, "Close the game and Wine first"));
    let save = fs::read_to_string(reg_save(&prefix)).unwrap();
    assert_eq!(save, "Reg/save.dat");
    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    assert_eq!(user_reg, co_user_reg("old"));
}

#[test]
fn batch_restores_only_the_ticked_files() {
    let tmp = tempfile::tempdir().unwrap();
    let tails = ["Alpha/save.dat", "Alpha/slot2.dat"];
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &tails);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    for tail in tails {
        put(&under(&xdg_data(&tmp), tail), "old");
    }

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    click(&mut harness, "Open XdgData/Alpha");
    click(&mut harness, "slot2.dat, changed");
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    let read = |tail| fs::read_to_string(under(&xdg_data(&tmp), tail)).unwrap();
    assert_eq!(read("Alpha/slot2.dat"), "old");
    assert_eq!(read("Alpha/save.dat"), "Alpha/save.dat");
    assert_eq!(read("Beta/save.dat"), "Beta/save.dat");
    let entries = safety_entries(&tmp, "Alpha").expect("Alpha's safety backup");
    assert!(
        entries.iter().any(|path| path.ends_with("Alpha/save.dat")),
        "{entries:?}"
    );
    assert!(
        !entries.iter().any(|path| path.ends_with("slot2.dat")),
        "{entries:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_batch_game_restores_without_an_unticked_unreadable_file() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat", "Beta/slot2.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    let locked = under(&xdg_data(&tmp), "Beta/save.dat");
    put(&locked, "old");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let unlock = Unlock(locked.clone(), 0o644);
    // Root reads it anyway, so there is nothing to test.
    if fs::read(&locked).is_ok() {
        return;
    }

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    assert!(harness.query_by_label("Restore 1 game").is_some());
    click(&mut harness, "Open XdgData/Beta");
    click(&mut harness, "save.dat, will fail");
    assert!(!shows(&harness, "Untick them to restore the rest"));
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");
    drop(unlock);

    assert_eq!(fs::read_to_string(&locked).unwrap(), "old");
    let slot2 = fs::read_to_string(under(&xdg_data(&tmp), "Beta/slot2.dat")).unwrap();
    assert_eq!(slot2, "Beta/slot2.dat");
    let none = safety_entries(&tmp, "Beta");
    assert_eq!(none, None, "nothing it replaced existed");
}

#[test]
fn a_batch_game_with_nothing_ticked_is_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, _env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    let beta_save = under(&xdg_data(&tmp), "Beta/save.dat");
    put(&beta_save, "old");

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    click(&mut harness, "XdgData/Beta");
    assert!(shows(&harness, "Nothing ticked"));
    assert!(!disabled(&harness, "Restore 1 game"));

    click(&mut harness, "XdgData/Alpha");
    assert!(disabled(&harness, "Restore 0 games"));
    click(&mut harness, "XdgData/Alpha");
    run_batch(&mut harness, "Restore 1 game", "Restored 1 of 1 game");

    assert_eq!(fs::read_to_string(&beta_save).unwrap(), "old");
    assert_eq!(safety_entries(&tmp, "Beta"), None);
    let alpha_save = under(&xdg_data(&tmp), "Alpha/save.dat");
    assert_eq!(fs::read_to_string(alpha_save).unwrap(), "Alpha/save.dat");
}

#[test]
fn a_batch_game_without_its_registry_keys_restores_while_wine_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = registry_games(&tmp, || true);

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    click(&mut harness, REG_KEY);
    click(&mut harness, OTHER_KEY);
    assert!(shows(&harness, "Registry: 0 keys into"));
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    assert!(!shows(&harness, "Close the game and Wine first"));
    let save = fs::read_to_string(reg_save(&prefix)).unwrap();
    assert_eq!(save, "Reg/save.dat");
    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    assert_eq!(user_reg, co_user_reg("old"));
}

/// The block of `Software\Co\Reg` or of its subkey `child` with `v` = `<age>-<name>`, `name`
/// being the child in lowercase or `reg`.
fn slot_block(age: &str, child: &str) -> String {
    let (path, name) = match child {
        "" => (r"Software\\Co\\Reg".to_owned(), "reg".to_owned()),
        _ => (format!(r"Software\\Co\\Reg\\{child}"), child.to_lowercase()),
    };
    format!("[{path}] 1\n#time=0\n\"v\"=\"{age}-{name}\"\n")
}

/// A `user.reg` holding `Software\Co\Reg` with `Slot1` and `Slot2`, each value `<age>-…`; with
/// `local`, also `Reg\Local`, which only the restore's target has.
fn slots_user_reg(age: &str, local: bool) -> String {
    let children: &[&str] = if local {
        &["", "Local", "Slot1", "Slot2"]
    } else {
        &["", "Slot1", "Slot2"]
    };
    let blocks: Vec<String> = children
        .iter()
        .map(|child| slot_block(age, child))
        .collect();
    format!("WINE REGISTRY Version 2\n\n{}\n", blocks.join("\n"))
}

/// Reg, a Windows build in a prefix with a long path that declares `Software\Co\Reg`, backed up
/// with a save and `new` values of `Reg`, `Slot1` and `Slot2`, while the prefix holds them with
/// `old` values and `Reg\Local`; and Beta, a native game. Returns the prefix.
fn slot_games(tmp: &TempDir) -> (Harness<'static, App>, PathBuf) {
    let prefix = long_prefix(tmp);
    put(&prefix.join("user.reg"), &slots_user_reg("old", true));
    let source = tmp.path().join("source-pfx");
    put(&source.join("user.reg"), &slots_user_reg("new", false));
    let mut reg = custom("Reg", &["<winAppData>/Reg"], Some(&prefix), Vec::new());
    let key = "HKEY_CURRENT_USER/Software/Co/Reg".to_owned();
    reg.entry.registry.insert(key, RegistryRule::default());
    let mut reg_scan = scan(
        tmp,
        "Reg",
        &[(Base::WinAppData, "Reg/save.dat")],
        &[(Base::WinAppData, "Reg")],
    );
    reg_scan.registry_keys = vec![REG_KEY.into()];
    reg_scan.registry_source = Some(RegistrySource::WinePrefix(source));
    let (beta, beta_scan) = native(tmp, "Beta", &["Beta/save.dat"]);
    let (harness, _env) = open_tab(tmp, vec![reg, beta], &[reg_scan, beta_scan]);
    (harness, prefix)
}

/// Asserts that `prefix` holds the old `Slot1` byte for byte, the new `Reg` and `Slot2`, and no
/// `Local`, and that Reg's safety backup holds the old `Slot1`.
fn assert_slot1_left_alone(tmp: &TempDir, prefix: &Path) {
    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    assert!(user_reg.contains(&slot_block("old", "Slot1")), "{user_reg}");
    assert!(
        user_reg.contains("new-reg") && user_reg.contains("new-slot2"),
        "{user_reg}"
    );
    assert!(
        !user_reg.contains("old-reg") && !user_reg.contains("old-slot2"),
        "{user_reg}"
    );
    assert!(
        !user_reg.contains("Local"),
        "owned by the ticked Reg: {user_reg}"
    );
    let safety = snapshots(tmp, "pre-restore");
    let safety = safety
        .iter()
        .find(|snapshot| snapshot.label == "Reg")
        .unwrap();
    let path = format!(
        "/salvajocs/{}/registry.json",
        salvajocs_core::slug::slug("Reg")
    );
    let saved = open_repo(tmp)
        .read_file(&safety.id, &path, u64::MAX)
        .unwrap();
    let saved = String::from_utf8(saved).unwrap();
    assert!(
        saved.contains("old-slot1") && saved.contains("old-local"),
        "{saved}"
    );
}

#[test]
fn an_unticked_registry_subkey_is_left_as_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = slot_games(&tmp);
    select(&mut harness, "Reg");
    preview(&mut harness);
    assert!(shows(&harness, "Registry: 3 keys into"));

    click(&mut harness, &format!("Open {REG_KEY}"));
    click(&mut harness, "Slot1");
    assert!(shows(&harness, "Registry: 2 keys into"));
    assert!(shows(&harness, "2 of 3 keys"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Reg"));

    assert_slot1_left_alone(&tmp, &prefix);
}

#[test]
fn a_ticked_subkey_under_an_unticked_key_is_restored_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = slot_games(&tmp);
    select(&mut harness, "Reg");
    preview(&mut harness);

    click(&mut harness, &format!("Open {REG_KEY}"));
    click(&mut harness, &format!("Values of {REG_KEY}"));
    assert!(shows(&harness, "Registry: 2 keys into"));
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Reg"));

    let user_reg = fs::read_to_string(prefix.join("user.reg")).unwrap();
    for child in ["", "Local"] {
        assert!(user_reg.contains(&slot_block("old", child)), "{user_reg}");
    }
    assert!(
        user_reg.contains("new-slot1") && user_reg.contains("new-slot2"),
        "{user_reg}"
    );
    assert!(
        !user_reg.contains("old-slot1") && !user_reg.contains("old-slot2"),
        "{user_reg}"
    );
}

#[test]
fn batch_leaves_an_unticked_registry_subkey_as_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, prefix) = slot_games(&tmp);

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    click(&mut harness, &format!("Open {REG_KEY}"));
    click(&mut harness, "Slot1");
    assert!(shows(&harness, "Registry: 2 keys into"));
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    assert_slot1_left_alone(&tmp, &prefix);
}

/// Beta, a native game, and Stray, whose only backed-up file lies outside its declared folder,
/// so its plan skips it and has nothing to restore.
fn with_nothing_to_restore(tmp: &TempDir) -> Harness<'static, App> {
    let (beta, beta_scan) = native(tmp, "Beta", &["Beta/save.dat"]);
    let stray = custom("Stray", &["<xdgData>/Stray"], None, Vec::new());
    let stray_scan = scan(
        tmp,
        "Stray",
        &[(Base::XdgData, "Elsewhere/x.dat")],
        &[(Base::XdgData, "Stray")],
    );
    open_tab(tmp, vec![beta, stray], &[beta_scan, stray_scan]).0
}

#[test]
fn a_plan_with_nothing_to_restore_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = with_nothing_to_restore(&tmp);
    select(&mut harness, "Stray");
    preview(&mut harness);

    assert!(shows(&harness, "Files to write (0)"));
    assert!(shows(&harness, "Skipped (1)"));
    assert!(shows(&harness, "Nothing to restore."));
    assert!(!shows(&harness, "Tick at least one"));
    assert!(disabled(&harness, "Restore files"));
}

#[test]
fn a_batch_game_with_nothing_to_restore_is_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = with_nothing_to_restore(&tmp);

    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");

    assert!(shows(&harness, "Nothing to restore, so this game"));
    assert!(!shows(&harness, "Nothing ticked"));
    assert!(!disabled(&harness, "Restore 1 game"));
}

/// A scan of `title` with `files` save files in its own folder.
fn scan_of(tmp: &TempDir, title: &str, files: usize) -> GameScan {
    let tails: Vec<String> = (0..files).map(|n| format!("{title}/{n}.sav")).collect();
    let files: Vec<_> = (tails.iter())
        .map(|tail| (Base::WinAppData, tail.as_str()))
        .collect();
    scan(tmp, title, &files, &[(Base::WinAppData, title)])
}

/// The titles among `titles`, top to bottom as the sidebar shows them.
fn listed_order<'a>(harness: &Harness<'static, App>, titles: &[&'a str]) -> Vec<&'a str> {
    let mut titles = titles.to_vec();
    let top = |title: &str| harness.get_by_label(title).rect().min.y;
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

/// Runs frames until the sidebar shows the space of `count` games, worked out.
fn run_until_space_listed(harness: &mut Harness<'static, App>, count: usize) {
    run_until(harness, "the space used", |h| {
        h.query_all_by_label_contains(" used").count() == count && !shows(h, "… used")
    });
    run_settled(harness);
}

const TITLES: [&str; 3] = ["Alpha", "Beta", "Gamma"];

#[test]
fn restore_list_sorts_by_last_backup_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    // Backed up in this order, so Alpha is the newest.
    let games = ["Beta", "Gamma", "Alpha"].map(|title| scan_of(&tmp, title, 1));
    let (mut harness, _env) = open_tab(&tmp, Vec::new(), &games);
    run_settled(&mut harness);
    assert_eq!(listed_order(&harness, &TITLES), TITLES, "by name");

    sort_by(&mut harness, "Last backup");
    assert_eq!(listed_order(&harness, &TITLES), ["Alpha", "Gamma", "Beta"]);
    click(&mut harness, "Newest first");
    assert!(shows(&harness, "Oldest first"));
    assert_eq!(listed_order(&harness, &TITLES), ["Beta", "Gamma", "Alpha"]);
}

#[test]
fn a_game_with_only_safety_backups_sorts_last_by_last_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let games = ["Alpha", "Beta"].map(|title| scan_of(&tmp, title, 1));
    let (mut harness, _env) = open_tab(&tmp, Vec::new(), &games);
    let gamma = [scan_of(&tmp, "Gamma", 1)];
    let stage = tmp.path().join("fixture-stage");
    backup_games(
        &mut open_repo(&tmp),
        &gamma,
        &stage,
        &["pre-restore"],
        None,
        |_| {},
    )
    .unwrap();
    click(&mut harness, "Load backups");
    run_until(&mut harness, "the list", |h| shows(h, "Gamma"));
    assert!(shows(&harness, "Only safety backups"));

    sort_by(&mut harness, "Last backup");
    assert_eq!(listed_order(&harness, &TITLES), ["Beta", "Alpha", "Gamma"]);
    click(&mut harness, "Newest first");
    assert_eq!(listed_order(&harness, &TITLES), ["Alpha", "Beta", "Gamma"]);
}

#[test]
fn clicking_the_figures_under_a_title_selects_the_game() {
    let tmp = tempfile::tempdir().unwrap();
    let games = ["Alpha", "Beta"].map(|title| scan_of(&tmp, title, 1));
    let (mut harness, _env) = open_tab(&tmp, Vec::new(), &games);
    run_settled(&mut harness);

    let title = harness.get_by_label("Beta").rect();
    let line = (harness.query_all_by_label_contains(" used"))
        .filter(|line| line.rect().min.y >= title.max.y)
        .min_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y))
        .expect("Beta has a line of figures");
    line.click();
    run_until(&mut harness, "Beta to be chosen", |h| {
        shows(h, "Delete all backups of Beta")
    });
}

#[test]
fn restore_sort_is_independent_of_backup_sort() {
    let tmp = tempfile::tempdir().unwrap();
    let games = ["Alpha", "Beta"].map(|title| scan_of(&tmp, title, 1));
    let (mut harness, env) = open_tab(&tmp, Vec::new(), &games);

    sort_by(&mut harness, "Last backup size");
    // The config is written in the background, which asks for no frames while it runs.
    let config_dir = env.config_dir.clone();
    let picked = ListSort::new(SortKey::LastBackupSize, true);
    run_until(&mut harness, "the sort to be saved", move |_| {
        let saved = AppConfig::load(&config_dir).ok().flatten();
        saved.is_some_and(|saved| saved.restore_sort == picked)
    });
    let config = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(config.backup_sort, ListSort::default());
}

#[test]
fn restore_list_sorts_by_space_used_once_computed() {
    let tmp = tempfile::tempdir().unwrap();
    // More files take more space: the blobs and the tree entries add up.
    let games = [("Alpha", 5), ("Beta", 1), ("Gamma", 20)];
    let games = games.map(|(title, files)| scan_of(&tmp, title, files));
    let (mut harness, _env) = open_tab(&tmp, Vec::new(), &games);
    run_until_space_listed(&mut harness, 3);

    sort_by(&mut harness, "Space used");
    assert_eq!(listed_order(&harness, &TITLES), ["Gamma", "Alpha", "Beta"]);
    click(&mut harness, "Largest first");
    assert_eq!(listed_order(&harness, &TITLES), ["Beta", "Alpha", "Gamma"]);
}

/// The folder of the plan of Deep, whose files are `a.sav`, `sub/b.sav` and `sub/deeper/c.sav`.
const DEEP: &str = "XdgData/Deep";

/// Deep, a native game with files in nested folders, planned with its folder open; each of
/// `existing`, a path below the game's folder, already holds `old` at its planned target.
fn deep_planned(tmp: &TempDir, existing: &[&str]) -> Harness<'static, App> {
    let tails = ["Deep/a.sav", "Deep/sub/b.sav", "Deep/sub/deeper/c.sav"];
    let (game, snapshot) = native(tmp, "Deep", &tails);
    let (mut harness, _env) = open_tab(tmp, vec![game], &[snapshot]);
    for tail in existing {
        put(&under(&xdg_data(tmp).join("Deep"), tail), "old");
    }
    select(&mut harness, "Deep");
    preview(&mut harness);
    click(&mut harness, &format!("Open {DEEP}"));
    harness
}

/// Opens the override field of the plan's folder `key`, types `target` and applies it, then
/// waits for the new targets to be compared.
fn apply_folder_override(harness: &mut Harness<'static, App>, key: &str, target: &Path) {
    click(harness, &format!("Override folder {key}"));
    let field = format!("New target of folder {key}");
    replace_text(harness, &field, target.to_str().unwrap());
    click(harness, &format!("Apply the new target of folder {key}"));
    run_until(harness, "the new targets compared", |h| {
        !shows(h, "Checking the new targets")
    });
}

/// Restores the plan and waits for the result.
fn restore_deep(harness: &mut Harness<'static, App>) {
    harness.get_by_label("Restore files").click();
    run_until(harness, "the restore", |h| shows(h, "Restored Deep"));
}

#[test]
fn a_folder_override_sends_every_file_below_it_into_the_new_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &["sub/b.sav"]);
    let elsewhere = under(tmp.path(), "elsewhere");
    put(&elsewhere.join("sub/b.sav"), "old");

    apply_folder_override(&mut harness, DEEP, &elsewhere);

    let overridden = "(overridden for this restore)";
    assert!(shows_path(
        &harness,
        &format!("{} {overridden}", elsewhere.display())
    ));
    assert!(shows(&harness, "3 files overridden"));
    let a = under(&elsewhere, "a.sav");
    assert!(shows_path(
        &harness,
        &format!("{} {overridden}", a.display())
    ));
    click(&mut harness, "Open sub");
    let b = under(&elsewhere, "sub/b.sav");
    assert!(shows_path(
        &harness,
        &format!("{} {overridden}", b.display())
    ));
    // The target there holds other bytes, which the label compares afresh.
    assert!(harness.query_by_label("b.sav, changed").is_some());

    restore_deep(&mut harness);

    assert_eq!(fs::read_to_string(&a).unwrap(), "Deep/a.sav");
    assert_eq!(fs::read_to_string(&b).unwrap(), "Deep/sub/b.sav");
    let c = under(&elsewhere, "sub/deeper/c.sav");
    assert_eq!(fs::read_to_string(c).unwrap(), "Deep/sub/deeper/c.sav");
    // Nothing went to the planned targets, where `b.sav` is still the old one.
    let planned = xdg_data(&tmp).join("Deep");
    assert!(!planned.join("a.sav").exists());
    assert_eq!(
        fs::read_to_string(planned.join("sub/b.sav")).unwrap(),
        "old"
    );
    // The safety backup holds what the restore overwrote at the new targets, and only that.
    let entries = safety_entries(&tmp, "Deep").expect("a safety backup");
    assert!(
        entries
            .iter()
            .any(|path| path.ends_with("elsewhere/sub/b.sav")),
        "{entries:?}"
    );
    assert!(
        !entries.iter().any(|path| path.ends_with("Deep/sub/b.sav")),
        "{entries:?}"
    );
    assert!(
        !entries.iter().any(|path| path.ends_with("a.sav")),
        "{entries:?}"
    );
}

#[test]
fn clearing_a_folder_override_restores_to_the_planned_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);
    let elsewhere = under(tmp.path(), "elsewhere");

    apply_folder_override(&mut harness, DEEP, &elsewhere);
    assert!(shows(&harness, "3 files overridden"));
    click(
        &mut harness,
        &format!("Clear the override of folder {DEEP}"),
    );
    run_until(&mut harness, "the targets compared", |h| {
        !shows(h, "Checking the new targets")
    });
    assert!(!shows(&harness, "overridden"));
    assert!(!disabled(&harness, "Restore files"));

    restore_deep(&mut harness);

    let planned = xdg_data(&tmp).join("Deep");
    assert_eq!(
        fs::read_to_string(planned.join("a.sav")).unwrap(),
        "Deep/a.sav"
    );
    assert!(planned.join("sub/deeper/c.sav").exists());
    assert!(!elsewhere.exists());
}

#[test]
fn a_folder_override_with_a_relative_path_is_refused_and_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);

    apply_folder_override(&mut harness, DEEP, Path::new("relative/dir"));

    assert!(shows(&harness, "not an absolute path"));
    assert!(!shows(&harness, "overridden"));
    assert!(harness.query_by_label("a.sav, new").is_some());
    assert!(disabled(&harness, "Restore files"));
    assert!(shows(
        &harness,
        "The new target of folder XdgData/Deep is not applied"
    ));

    // Closing the field drops the text and the error, and the plan restores as planned.
    click(&mut harness, &format!("Override folder {DEEP}"));
    assert!(!shows(&harness, "not an absolute path"));
    assert!(!disabled(&harness, "Restore files"));
    restore_deep(&mut harness);
    let planned = xdg_data(&tmp).join("Deep");
    assert!(planned.join("a.sav").exists());
    assert!(!tmp.path().join("relative").exists());
}

#[test]
fn an_unticked_file_below_an_overridden_folder_is_not_written_or_backed_up() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);
    let elsewhere = under(tmp.path(), "elsewhere");
    put(&elsewhere.join("a.sav"), "old");
    click(&mut harness, "a.sav, new");

    apply_folder_override(&mut harness, DEEP, &elsewhere);
    assert!(!ticked(&harness, "a.sav, changed"));
    restore_deep(&mut harness);

    assert_eq!(fs::read_to_string(elsewhere.join("a.sav")).unwrap(), "old");
    assert!(elsewhere.join("sub/b.sav").exists());
    assert!(!xdg_data(&tmp).join("Deep").exists());
    assert!(safety_entries(&tmp, "Deep").is_none_or(|entries| entries.is_empty()));
}

#[test]
fn a_folder_override_replaces_the_overrides_it_covers_and_is_replaced_by_a_file_override() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);
    let (one, two, three) = (
        under(tmp.path(), "one"),
        under(tmp.path(), "two"),
        under(tmp.path(), "three"),
    );

    // The file's own override gives way to the folder's.
    apply_override(&mut harness, "XdgData/Deep/a.sav", &one.join("x.sav"));
    apply_folder_override(&mut harness, DEEP, &two);
    let overridden = "(overridden for this restore)";
    assert!(shows_path(
        &harness,
        &format!("{} {overridden}", two.join("a.sav").display())
    ));
    assert!(!shows_path(
        &harness,
        &one.join("x.sav").display().to_string()
    ));

    // The inner folder's override leaves the outer folder's header with nothing to clear.
    click(&mut harness, "Open sub");
    apply_folder_override(&mut harness, "XdgData/Deep/sub", &three);
    assert!(
        harness
            .query_by_label(&format!("Clear the override of folder {DEEP}"))
            .is_none()
    );
    let clear = "Clear the override of folder XdgData/Deep/sub";
    assert!(harness.query_by_label(clear).is_some());
    assert!(shows(&harness, "3 files overridden"));

    // Overriding a file below ends its folder's override too.
    apply_override(&mut harness, "XdgData/Deep/sub/b.sav", &one.join("y.sav"));
    assert!(harness.query_by_label(clear).is_none());
    assert!(shows(&harness, "3 files overridden"));
    restore_deep(&mut harness);
    assert!(two.join("a.sav").exists());
    assert!(one.join("y.sav").exists());
    assert!(three.join("deeper/c.sav").exists());
    assert!(!three.join("b.sav").exists());
}

#[test]
fn restore_files_waits_while_the_new_targets_of_a_folder_are_compared() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);
    let elsewhere = under(tmp.path(), "elsewhere");

    click(&mut harness, &format!("Override folder {DEEP}"));
    let field = format!("New target of folder {DEEP}");
    replace_text(&mut harness, &field, elsewhere.to_str().unwrap());
    // Frame by frame: the job's result is only taken by a frame after the click's.
    let apply = format!("Apply the new target of folder {DEEP}");
    harness.get_by_label(&apply).click();
    run_until(&mut harness, "the comparison to start", |h| {
        shows(h, "Checking the new targets")
    });
    assert!(disabled(&harness, "Restore files"));

    run_until(&mut harness, "the comparison to end", |h| {
        !shows(h, "Checking the new targets")
    });
    assert!(!disabled(&harness, "Restore files"));
    assert!(!disabled(&harness, &format!("Override folder {DEEP}")));
}

// NTFS ignores case, so the plan's target there keeps the backup's `my games`: the folder has
// no other case on disk to keep.
#[cfg(unix)]
#[test]
fn a_folder_override_keeps_the_case_the_target_has_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let prefix = make_prefix(&tmp);
    let documents = under(&prefix, "drive_c/users/steamuser/Documents");
    fs::create_dir_all(documents.join("My Games")).unwrap();
    let files = ["<winDocuments>/my games/Cased", "<winDocuments>/Notes"];
    let game = custom("Cased", &files, Some(&prefix), Vec::new());
    // The backup says `my games`, the prefix has `My Games`; `Notes` keeps WinDocuments from
    // being drawn as a chain with `my games`, so the Base is the folder to override.
    let snapshot = scan(
        &tmp,
        "Cased",
        &[
            (Base::WinDocuments, "my games/Cased/save.dat"),
            (Base::WinDocuments, "Notes/n.dat"),
        ],
        &[
            (Base::WinDocuments, "my games/Cased"),
            (Base::WinDocuments, "Notes"),
        ],
    );
    let (mut harness, _env) = open_tab(&tmp, vec![game], &[snapshot]);
    select(&mut harness, "Cased");
    preview(&mut harness);
    click(&mut harness, "Open WinDocuments");
    let elsewhere = under(tmp.path(), "elsewhere");

    apply_folder_override(&mut harness, "WinDocuments", &elsewhere);

    click(&mut harness, "Open my games/Cased");
    let moved = under(&elsewhere, "My Games/Cased/save.dat");
    let overridden = "(overridden for this restore)";
    assert!(shows_path(
        &harness,
        &format!("{} {overridden}", moved.display())
    ));
}

/// The id this PC's recorded base of the game `title` holds in the tests' repository, if any.
fn recorded_base(tmp: &TempDir, env: &Env, title: &str) -> Option<String> {
    let scope = local_config(&repo_dir(tmp)).secret_scope();
    let bases = sync::load(&env.config_dir);
    let base = bases.get(&scope, &salvajocs_core::slug::slug(title))?;
    Some(base.snapshot.clone())
}

/// Records `snapshot` as this PC's base of the game `title`, as an earlier Back up did.
fn record_base(tmp: &TempDir, env: &Env, title: &str, snapshot: &str) {
    let scope = local_config(&repo_dir(tmp)).secret_scope();
    let update = (
        salvajocs_core::slug::slug(title),
        SyncBase::new(snapshot.to_owned(), None),
    );
    sync::record(&env.config_dir, &scope, [update]).unwrap();
}

#[test]
fn a_restore_records_the_restored_snapshot_as_the_base() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, _saves) = alpha(&tmp);
    assert_eq!(recorded_base(&tmp, &env, "Alpha"), None);
    select(&mut harness, "Alpha");
    preview(&mut harness);

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    let alpha = backups_of(&tmp, "Alpha");
    assert_eq!(recorded_base(&tmp, &env, "Alpha"), Some(alpha[0].clone()));
    assert!(!shows(&harness, "sync state was not updated"));
}

#[test]
fn a_restore_with_a_failed_write_records_no_base() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, _saves) = alpha(&tmp);
    let dir = under(tmp.path(), "elsewhere/slot2.dat");
    fs::create_dir_all(&dir).unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    apply_override(&mut harness, ALPHA_SLOT2, &dir);

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(shows_path(&harness, "failed"));
    assert_eq!(recorded_base(&tmp, &env, "Alpha"), None);
}

#[test]
fn a_base_that_cannot_be_updated_does_not_fail_the_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, saves) = alpha(&tmp);
    // A directory where the file belongs cannot be read as one.
    fs::create_dir_all(env.config_dir.join("sync-base.json")).unwrap();
    select(&mut harness, "Alpha");
    preview(&mut harness);

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(shows(&harness, "sync state was not updated"));
    assert!(saves.join("save.dat").exists());
}

#[test]
fn a_restore_with_a_file_unticked_still_records_the_base() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, saves) = alpha(&tmp);
    let alpha = backups_of(&tmp, "Alpha");
    select(&mut harness, "Alpha");
    preview(&mut harness);
    click(&mut harness, "Open WinAppData/Alpha");
    click(&mut harness, "slot2.dat, new");

    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(saves.join("save.dat").exists());
    assert!(
        !saves.join("slot2.dat").exists(),
        "the unticked file is left out"
    );
    assert_eq!(recorded_base(&tmp, &env, "Alpha"), Some(alpha[0].clone()));
}

#[test]
fn a_batch_restore_records_the_base_of_each_game_it_restored() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    for title in ["Alpha", "Beta"] {
        let newest = backups_of(&tmp, title)[0].clone();
        assert_eq!(recorded_base(&tmp, &env, title), Some(newest));
    }
}

#[test]
fn a_batch_game_whose_backup_cannot_be_fetched_records_no_base() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    let alpha = backups_of(&tmp, "Alpha");
    open_repo(&tmp).forget(&[&alpha[0]]).unwrap();
    run_batch(&mut harness, "Restore 2 games", "Restored 1 of 2 games");

    assert_eq!(recorded_base(&tmp, &env, "Alpha"), None);
    let beta = backups_of(&tmp, "Beta");
    assert_eq!(recorded_base(&tmp, &env, "Beta"), Some(beta[0].clone()));
}
/// This PC's recorded base of the game `title` in the tests' repository, if any.
fn recorded(tmp: &TempDir, env: &Env, title: &str) -> Option<SyncBase> {
    let scope = local_config(&repo_dir(tmp)).secret_scope();
    let bases = sync::load(&env.config_dir);
    bases
        .get(&scope, &salvajocs_core::slug::slug(title))
        .cloned()
}

/// Restores Alpha's default snapshot and waits for the list to reload.
fn restore_alpha(harness: &mut Harness<'static, App>) {
    select(harness, "Alpha");
    preview(harness);
    harness.get_by_label("Restore files").click();
    run_until(harness, "the restore", |h| shows(h, "Restored Alpha"));
    run_until(harness, "the list to reload", |h| {
        !disabled(h, "Load backups")
    });
}

/// Scans and backs up on the Backup tab, where Alpha is up to date.
fn back_up_alpha_up_to_date(harness: &mut Harness<'static, App>) {
    harness.state_mut().set_tab(Tab::Backup);
    run_settled(harness);
    harness.get_by_label("Scan for games").click();
    run_until(harness, "the scan", |h| {
        shows(h, "1 game found") && !shows(h, "Scanning") && !shows(h, "Checked")
    });
    run_settled(harness);
    harness.get_by_label("Back up 1 game").click();
    run_until_backed_up(harness, "Alpha: up to date");
}

#[test]
fn a_restore_records_this_pcs_saves_as_back_up_stages_them() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[alpha_scan]);
    restore_alpha(&mut harness);
    let restored = recorded(&tmp, &env, "Alpha").unwrap().saves;
    let restored = restored.expect("the restore records this PC's saves");

    back_up_alpha_up_to_date(&mut harness);

    assert_eq!(backups_of(&tmp, "Alpha").len(), 1);
    let backed_up = recorded(&tmp, &env, "Alpha").unwrap().saves.unwrap();
    assert!(restored.same_content(&backed_up));
}

#[test]
fn a_restore_lists_this_pcs_saves_without_a_file_a_pending_choice_excludes() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    put(
        &xdg_data(&tmp).join("Alpha").join("slot.dat"),
        "not backed up",
    );
    let excluded = Change::Path {
        node: "XdgData/Alpha/slot.dat".into(),
        included: false,
    };
    let mut pending = Pending::default();
    pending
        .games_mut(&local_config(&repo_dir(&tmp)))
        .insert("Alpha".into(), vec![excluded]);
    pending.save(&env(&tmp).config_dir).unwrap();
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[alpha_scan]);
    restore_alpha(&mut harness);
    let restored = recorded(&tmp, &env, "Alpha").unwrap().saves.unwrap();

    back_up_alpha_up_to_date(&mut harness);

    let backed_up = recorded(&tmp, &env, "Alpha").unwrap().saves.unwrap();
    assert!(restored.same_content(&backed_up));
}

#[test]
fn a_restored_game_the_scan_does_not_find_records_no_saves() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = deep_planned(&tmp, &[]);
    // Outside Deep's save folder, where no scan looks.
    apply_folder_override(&mut harness, DEEP, &under(tmp.path(), "elsewhere"));
    restore_deep(&mut harness);

    let env = env(&tmp);
    let deep = recorded(&tmp, &env, "Deep").unwrap();
    assert_eq!(deep.snapshot, backups_of(&tmp, "Deep")[0]);
    assert_eq!(deep.saves, None);
    assert!(!shows(&harness, "were not listed"));
}

#[test]
fn saves_that_cannot_be_staged_record_the_base_without_them_and_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[alpha_scan]);
    // A file where Back up's staging mirror belongs.
    put(&env.cache_dir.join("stage"), "not a folder");
    restore_alpha(&mut harness);

    assert!(shows(&harness, "were not listed"));
    let alpha = recorded(&tmp, &env, "Alpha").unwrap();
    assert_eq!(alpha.snapshot, backups_of(&tmp, "Alpha")[0]);
    assert_eq!(alpha.saves, None);
}

#[test]
fn pending_choices_that_cannot_be_read_record_the_base_without_saves_and_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[alpha_scan]);
    // A newer version's file: Back up would filter by the choices it shows instead.
    let newer = r#"{"format": 999, "repositories": {}}"#;
    put(&env.config_dir.join("pending-selection.json"), newer);
    restore_alpha(&mut harness);

    assert!(shows(&harness, "were not listed"));
    let alpha = recorded(&tmp, &env, "Alpha").unwrap();
    assert_eq!(alpha.snapshot, backups_of(&tmp, "Alpha")[0]);
    assert_eq!(alpha.saves, None);
}

#[test]
fn settings_that_cannot_be_read_record_the_base_without_saves_and_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha], &[alpha_scan]);
    select(&mut harness, "Alpha");
    preview(&mut harness);
    // A snapshot file that is not one: the repository's snapshots cannot be listed again.
    let snapshots = Path::new(&repo_dir(&tmp)).join("snapshots");
    let garbage = snapshots.join("0".repeat(64));
    put(&garbage, "garbage");
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));

    assert!(shows(&harness, "were not listed"));
    fs::remove_file(garbage).unwrap();
    let alpha = recorded(&tmp, &env, "Alpha").unwrap();
    assert_eq!(alpha.snapshot, backups_of(&tmp, "Alpha")[0]);
    assert_eq!(alpha.saves, None);
}

#[test]
fn a_batch_restore_records_this_pcs_saves_of_each_game_back_up_stores() {
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, alpha_scan) = native(&tmp, "Alpha", &["Alpha/save.dat"]);
    let (beta, beta_scan) = native(&tmp, "Beta", &["Beta/save.dat"]);
    let (mut harness, env) = open_tab(&tmp, vec![alpha, beta], &[alpha_scan, beta_scan]);
    // Back up stores nothing of a skipped game, so its base gets no list.
    let skip = [Change::Skip(true)];
    let settings = tmp.path().join("settings");
    selection::save(&mut open_repo(&tmp), "Beta", &skip, &settings).unwrap();
    click(&mut harness, "Select all");
    plan_batch(&mut harness, "Restore 2 games");
    run_batch(&mut harness, "Restore 2 games", "Restored 2 of 2 games");

    let alpha = recorded(&tmp, &env, "Alpha").unwrap();
    assert_eq!(alpha.snapshot, backups_of(&tmp, "Alpha")[0]);
    assert!(alpha.saves.is_some());
    let beta = recorded(&tmp, &env, "Beta").unwrap();
    assert_eq!(beta.snapshot, backups_of(&tmp, "Beta")[0]);
    assert_eq!(beta.saves, None);
    assert!(!shows(&harness, "were not listed"));
}

/// What the sidebar's marker says, up to the host and the date.
const NEWER_SAVES: &str = "The newest backup is not the one this PC last synced";

#[test]
fn the_sidebar_marks_a_game_whose_base_is_not_its_newest_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let names = ["Alpha", "Beta", "Gamma"];
    let (games, scans): (Vec<_>, Vec<_>) = (names.iter())
        .map(|name| native(&tmp, name, &[&format!("{name}/save.dat")]))
        .unzip();
    let (mut harness, env) = open_tab(&tmp, games, &scans);
    assert!(!shows(&harness, NEWER_SAVES), "no record, no marker");

    // Alpha's base is an older backup, Beta's the newest, and Gamma has none.
    record_base(&tmp, &env, "Alpha", &"0".repeat(64));
    let beta = backups_of(&tmp, "Beta");
    record_base(&tmp, &env, "Beta", &beta[0]);
    harness.get_by_label("Load backups").click();
    run_until(&mut harness, "the marker", |h| shows(h, NEWER_SAVES));
    run_settled(&mut harness);
    assert_eq!(
        harness.query_all_by_label_contains(NEWER_SAVES).count(),
        1,
        "only Alpha is marked"
    );
    // The newest backup's host, and its date as the snapshot picker shows it.
    select(&mut harness, "Alpha");
    let host = snapshots(&tmp, "salvajocs")[0].host.clone();
    let picked = value(&harness, "Snapshot");
    let tooltip = format!("{NEWER_SAVES}: from {host}, {}", &picked[..10]);
    assert!(shows(&harness, &tooltip), "{tooltip}");

    let alpha = backups_of(&tmp, "Alpha");
    record_base(&tmp, &env, "Alpha", &alpha[0]);
    harness.get_by_label("Load backups").click();
    run_until(&mut harness, "the list", |h| {
        !disabled(h, "Load backups") && !shows(h, NEWER_SAVES)
    });
}

#[test]
fn restoring_a_safety_backup_records_it_as_the_base_like_any_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut harness, env, saves) = alpha(&tmp);
    fs::create_dir_all(&saves).unwrap();
    fs::write(saves.join("save.dat"), "old").unwrap();
    let regular = backups_of(&tmp, "Alpha")[0].clone();
    select(&mut harness, "Alpha");
    preview(&mut harness);
    harness.get_by_label("Restore files").click();
    run_until(&mut harness, "the restore", |h| shows(h, "Restored Alpha"));
    assert_eq!(recorded_base(&tmp, &env, "Alpha"), Some(regular));
    // Another game's base stays.
    let other = "1".repeat(64);
    record_base(&tmp, &env, "Other", &other);
    let safety = snapshots(&tmp, "pre-restore");
    assert_eq!(safety.len(), 1);

    // The safety backup holds the files the restore replaced: going back to them is deliberate.
    run_until(&mut harness, "the list to reload", |h| {
        !disabled(h, "Load backups") && shows(h, "Restore into:")
    });
    click(&mut harness, "Snapshot");
    harness.get_by_label_contains(", pre-restore").click();
    preview(&mut harness);
    harness.get_by_label("Restore files").click();
    let save = saves.join("save.dat");
    run_until(&mut harness, "the second restore", |h| {
        shows(h, "Restored Alpha") && fs::read_to_string(&save).is_ok_and(|text| text == "old")
    });
    run_until(&mut harness, "the list to reload", |h| {
        !disabled(h, "Load backups")
    });

    assert_eq!(fs::read_to_string(saves.join("save.dat")).unwrap(), "old");
    assert_eq!(
        recorded_base(&tmp, &env, "Alpha"),
        Some(safety[0].id.clone())
    );
    assert_eq!(recorded_base(&tmp, &env, "Other"), Some(other));
    // The newest regular backup is not that base, so the sidebar marks the game.
    assert!(shows(&harness, NEWER_SAVES));
}
