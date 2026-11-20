use salvajocs_core::storage::{Compression, RepoLocation, Storage};

use super::*;

fn storage(dir: &std::path::Path) -> SharedStorage {
    let location = RepoLocation::new(dir.to_str().unwrap().to_owned(), Default::default());
    Arc::new(Mutex::new(
        Storage::init(&location, "", Compression::Default).unwrap(),
    ))
}

/// The ids of a game with one snapshot that is not in the repository.
fn unknown(digit: char) -> Vec<String> {
    vec![digit.to_string().repeat(64)]
}

fn finish(space: &mut Space) {
    while space.poll() != Polled::Finished {
        std::thread::yield_now();
    }
}

#[test]
fn a_start_with_every_game_cached_computes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = SpaceCache::default();
    lock(&cache).insert(unknown('1'), 42);

    let mut space = Space::start(
        &egui::Context::default(),
        &storage(dir.path()),
        &cache,
        vec![unknown('1')],
    );
    assert!(space.job.is_none());
    assert_eq!(space.poll(), Polled::Nothing);
    assert_eq!(space.get(&unknown('1')), Some(Ok(42)));
}

#[test]
fn a_run_records_a_failure_without_caching_it() {
    let dir = tempfile::tempdir().unwrap();
    let cache = SpaceCache::default();
    let mut space = Space::start(
        &egui::Context::default(),
        &storage(dir.path()),
        &cache,
        vec![unknown('1')],
    );
    finish(&mut space);
    assert!(matches!(space.get(&unknown('1')), Some(Err(_))));
    assert!(lock(&cache).is_empty());
}

#[test]
fn a_game_another_run_cached_meanwhile_arrives_without_being_worked_out() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, cache) = (storage(dir.path()), SpaceCache::default());
    // Held so the run waits while another tab's run caches the game.
    let held = lock_storage(&storage).unwrap();
    let mut space = Space::start(
        &egui::Context::default(),
        &storage,
        &cache,
        vec![unknown('1')],
    );
    lock(&cache).insert(unknown('1'), 42);
    drop(held);

    finish(&mut space);
    // Unknown ids would fail if worked out, so this figure came from the cache.
    assert_eq!(lock(&space.done).get(&unknown('1')), Some(&Ok(42)));
}

#[test]
fn a_stopped_run_works_out_no_further_game() {
    let dir = tempfile::tempdir().unwrap();
    let (storage, cache) = (storage(dir.path()), SpaceCache::default());
    // Held so the run waits before its first game.
    let held = lock_storage(&storage).unwrap();
    let games = vec![unknown('1'), unknown('2')];
    let mut space = Space::start(&egui::Context::default(), &storage, &cache, games);
    space.stop.store(true, Ordering::Relaxed);
    drop(held);

    finish(&mut space);
    assert_eq!(space.get(&unknown('1')), None);
    assert_eq!(space.get(&unknown('2')), None);
}
