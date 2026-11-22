use super::*;

#[test]
fn a_second_lock_on_the_same_dir_fails_until_the_first_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    // The cache dir does not exist on the first run.
    let cache_dir = tmp.path().join("missing").join("cache");

    let first = lock(&cache_dir).unwrap();
    assert!(first.is_some(), "the first instance gets the lock");
    assert!(lock(&cache_dir).unwrap().is_none(), "the second does not");
    drop(first);
    assert!(lock(&cache_dir).unwrap().is_some(), "dropping frees it");
}

#[test]
fn locks_on_different_dirs_do_not_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let first = lock(&tmp.path().join("a")).unwrap();
    let second = lock(&tmp.path().join("b")).unwrap();
    assert!(first.is_some() && second.is_some());
}
