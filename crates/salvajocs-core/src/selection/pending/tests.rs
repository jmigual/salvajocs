use super::*;
use crate::config::{ListSort, PasswordMode, RepoOption, ThemeChoice};

const GAME: &str = "Game, with comma: ünï";

fn config(repository: &str, options: &[(&str, &str, bool)]) -> AppConfig {
    AppConfig {
        repository: repository.into(),
        repository_options: options
            .iter()
            .map(|(key, value, secret)| RepoOption {
                key: (*key).into(),
                value: Some((*value).into()),
                secret: *secret,
            })
            .collect(),
        password: PasswordMode::None,
        roots: vec![],
        custom_games: vec![],
        check_updates: false,
        ignored_prefixes: Vec::new(),
        theme: ThemeChoice::System,
        backup_sort: ListSort::default(),
        restore_sort: ListSort::default(),
        custom_games_repository: None,
        custom_games_unsent: Vec::new(),
        retention_unsent: None,
    }
}

fn changes() -> Vec<Change> {
    vec![
        Change::Skip(true),
        Change::Path {
            node: "WinAppData/Foo".into(),
            included: false,
        },
        Change::Registry {
            key: r"HKEY_CURRENT_USER\Software\Foo".into(),
            included: false,
        },
    ]
}

#[test]
fn save_then_load_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("config");
    let repo = config("/backups/repo", &[]);
    let mut pending = Pending::default();
    pending.games_mut(&repo).insert(GAME.into(), changes());
    pending
        .games_mut(&repo)
        .insert("Other".into(), vec![Change::Skip(false)]);
    pending.save(&dir).unwrap();
    assert_eq!(Pending::load(&dir).unwrap(), pending);

    pending.remove(&repo, "Other");
    pending.save(&dir).unwrap();
    let mut loaded = Pending::load(&dir).unwrap();
    assert_eq!(
        loaded.games_mut(&repo).clone(),
        BTreeMap::from([(GAME.to_string(), changes())])
    );
}

#[test]
fn missing_file_has_no_changes() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(Pending::load(tmp.path()).unwrap(), Pending::default());
}

#[test]
fn repositories_are_kept_apart() {
    let tmp = tempfile::tempdir().unwrap();
    let a = config("/backups/a", &[]);
    let b = config("/backups/b", &[]);
    let mut pending = Pending::default();
    pending.games_mut(&a).insert(GAME.into(), changes());
    pending.save(tmp.path()).unwrap();

    // Switched to B: its changes are saved without touching A's.
    let mut pending = Pending::load(tmp.path()).unwrap();
    assert!(pending.games_mut(&b).is_empty());
    pending
        .games_mut(&b)
        .insert(GAME.into(), vec![Change::Skip(false)]);
    pending.save(tmp.path()).unwrap();

    let mut pending = Pending::load(tmp.path()).unwrap();
    assert_eq!(pending.games_mut(&a)[GAME], changes());
    assert_eq!(pending.games_mut(&b)[GAME], vec![Change::Skip(false)]);
}

#[test]
fn same_repository_string_with_other_options_is_another_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let saves = config(
        "opendal:s3",
        &[("bucket", "saves", false), ("key", "hush", true)],
    );
    let other = config("opendal:s3", &[("bucket", "other", false)]);
    let mut pending = Pending::default();
    pending.games_mut(&saves).insert(GAME.into(), changes());
    pending.save(tmp.path()).unwrap();

    let mut pending = Pending::load(tmp.path()).unwrap();
    assert!(pending.games_mut(&other).is_empty());
    assert_eq!(pending.games_mut(&saves)[GAME], changes());
    // A secret option's value is never written.
    let file = fs::read_to_string(tmp.path().join(PENDING_FILE)).unwrap();
    assert!(!file.contains("hush"));
}

#[test]
fn newer_format_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let file = r#"{ "format": 2, "repositories": [] }"#;
    fs::write(tmp.path().join(PENDING_FILE), file).unwrap();
    assert!(matches!(
        Pending::load(tmp.path()),
        Err(SelectionError::NewerFormat(2))
    ));
}
