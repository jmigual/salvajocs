use super::*;

/// The names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn write_atomic_replaces_an_existing_file_and_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    write_atomic(&path, b"old").unwrap();
    write_atomic(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(names(dir.path()), ["state.json"]);
}

#[test]
fn a_failed_write_removes_the_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    // A directory cannot be replaced by a file, so the rename fails after the temp is written.
    let path = dir.path().join("state.json");
    fs::create_dir(&path).unwrap();
    assert!(write_atomic(&path, b"data").is_err());
    assert_eq!(names(dir.path()), ["state.json"]);
}

#[test]
fn read_capped_refuses_bodies_over_the_limit() {
    assert_eq!(
        read_capped(Some(5), &b"hello"[..], 5).unwrap().unwrap(),
        b"hello"
    );
    assert_eq!(read_capped(Some(6), &b"hello!"[..], 5).unwrap(), None);
    assert_eq!(read_capped(None, &b"hello!"[..], 5).unwrap(), None);
}

#[test]
fn remove_if_exists_ignores_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone");
    remove_if_exists(&path).unwrap();
    fs::write(&path, b"x").unwrap();
    remove_if_exists(&path).unwrap();
    assert!(!path.exists());
}
