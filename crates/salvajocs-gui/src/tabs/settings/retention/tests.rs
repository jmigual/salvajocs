use std::time::{Duration, Instant};

use eframe::egui::accesskit::{Role, Toggled};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::manifest::fetch::{FetchError, Freshness};
use salvajocs_core::storage::{Compression, RepoLocation, Storage};

use super::*;
use crate::env::{Env, MemorySecrets};
use crate::state::Session;

const KEEP_LAST: &str = "Keep the last";

/// The section, the state it draws from, and whether it is drawn (its tab is in front).
type SectionHarness = Harness<'static, (RetentionSection, State, bool)>;

fn location(dir: &Path) -> RepoLocation {
    RepoLocation {
        repository: dir.join("repo").to_str().unwrap().to_string(),
        options: Default::default(),
    }
}

/// The section over a new repository in `dir`.
fn harness(dir: &Path) -> SectionHarness {
    harness_on(state(dir))
}

/// The section drawn from `state`. Building the harness draws the first frame, which starts the
/// load.
fn harness_on(state: State) -> SectionHarness {
    theme::harness(
        |ui, (section, state, shown): &mut (RetentionSection, State, bool)| {
            if *shown {
                section.ui(ui, state);
            }
        },
        (RetentionSection::default(), state, true),
    )
}

/// A state with a new repository in `dir` open.
fn state(dir: &Path) -> State {
    fn offline(_: &Path) -> Result<Freshness, FetchError> {
        Ok(Freshness::NotModified)
    }
    let mut state = State::new(Env {
        config_dir: dir.join("config"),
        cache_dir: dir.join("cache"),
        secrets: Arc::new(MemorySecrets::new()),
        refresh_manifest: offline,
        environment: None,
        wineserver_running: salvajocs_core::prefix::wineserver_running,
        check_update: None,
        installed_by_installer: || false,
        run_installer: |_| panic!("tests never start an installer"),
        rclone_remotes: || Err(std::io::ErrorKind::NotFound.into()),
        pick_folder: Arc::new(|| None),
        open_folder: Arc::new(|_| Ok(())),
    });
    let storage = Storage::init(&location(dir), "", Compression::Default).unwrap();
    state.session = Some(Session {
        compression: storage.compression(),
        storage: Arc::new(Mutex::new(storage)),
    });
    state
}

fn storage(state: &State) -> SharedStorage {
    state.session.as_ref().unwrap().storage.clone()
}

/// The policy in the repository, read by a session of its own.
fn stored(dir: &Path) -> Retention {
    retention::load(&mut Storage::open(&location(dir), "").unwrap()).unwrap()
}

fn step_until(harness: &mut SectionHarness, what: &str, ready: impl Fn(&SectionHarness) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if ready(harness) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn editable(harness: &SectionHarness) -> bool {
    !harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .accesskit_node()
        .is_disabled()
}

fn keeps_last(harness: &SectionHarness) -> bool {
    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

#[test]
fn controls_stay_disabled_while_the_policy_loads() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = storage(&state);
    let held = storage.lock().unwrap();
    let mut harness = harness_on(state);
    for _ in 0..3 {
        harness.step();
    }
    assert!(harness.state().0.loading.is_some());
    assert!(!editable(&harness));

    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    harness.step();
    drop(held);
    step_until(&mut harness, "the policy to load", editable);
    assert!(!keeps_last(&harness));
    assert!(!harness.state().0.busy());
    assert!(stored(dir.path()).is_off());
}

#[test]
fn busy_until_the_last_queued_save_ends_even_with_the_tab_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(dir.path());
    step_until(&mut harness, "the policy to load", editable);
    let storage = storage(&harness.state().1);
    let held = storage.lock().unwrap();

    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    harness.step();
    assert!(harness.state().0.busy());
    // The job takes the first policy off the queue on its own thread; edit again only once it
    // has, or the edit would replace the first policy instead of queueing behind it.
    step_until(&mut harness, "the first save to start", |h| {
        lock(&h.state().0.saves).next.is_none()
    });
    harness
        .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
        .set_value_accesskit(7.0);
    harness.step();
    // The first save waits for the repository, so the second is queued behind it.
    assert_eq!(
        lock(&harness.state().0.saves)
            .next
            .and_then(|policy| policy.keep_last)
            .map(NonZeroU32::get),
        Some(7)
    );

    harness.state_mut().2 = false;
    drop(held);
    step_until(&mut harness, "the saves to end", |h| !h.state().0.busy());
    assert_eq!(stored(dir.path()).keep_last.map(NonZeroU32::get), Some(7));
}

#[test]
fn a_reset_keeps_a_running_save_busy() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(dir.path());
    step_until(&mut harness, "the policy to load", editable);
    let storage = storage(&harness.state().1);
    let held = storage.lock().unwrap();
    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    harness.step();

    harness.state_mut().0.reset();
    assert!(harness.state().0.busy());
    drop(held);
    // `busy` turns false when the job ends; `previous` is pruned by the next frame's poll.
    step_until(&mut harness, "the save to end and be dropped", |h| {
        !h.state().0.busy() && h.state().0.previous.is_empty()
    });
    assert!(stored(dir.path()).keep_last.is_some());
}

#[test]
fn a_panicking_save_is_no_longer_running() {
    let saves = Arc::new(Mutex::new(Saves {
        running: true,
        ..Saves::default()
    }));
    let shared = saves.clone();
    let panicked = std::thread::spawn(move || {
        let _guard = PanicGuard(&shared);
        panic!("a failing save");
    })
    .join();
    assert!(panicked.is_err());
    let saves = lock(&saves);
    assert!(!saves.running);
    assert!(saves.error.is_some());
}

#[test]
fn a_failed_load_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(dir.path());
    let storage = storage(&state);
    // A poisoned lock fails the load the way an unreachable repository would.
    let poisoner = storage.clone();
    let _ = std::thread::spawn(move || {
        let _held = poisoner.lock().unwrap();
        panic!("poison the repository lock");
    })
    .join();
    let mut harness = harness_on(state);
    step_until(&mut harness, "the load to fail", |h| {
        h.query_by_label("Retry").is_some()
    });
    assert!(!editable(&harness));

    storage.clear_poison();
    harness.get_by_label("Retry").click();
    step_until(&mut harness, "the policy to load", editable);
}

#[test]
fn showing_the_tab_again_reloads_the_policy() {
    let dir = tempfile::tempdir().unwrap();
    let mut harness = harness(dir.path());
    step_until(&mut harness, "the policy to load", editable);
    assert!(!keeps_last(&harness));

    // Another machine changes the policy while the tab is hidden.
    let policy = Retention {
        keep_last: NonZeroU32::new(3),
        keep_within: None,
    };
    let mut other = Storage::open(&location(dir.path()), "").unwrap();
    retention::save(&mut other, &policy, &dir.path().join("stage")).unwrap();
    harness.state_mut().2 = false;
    harness.step();
    harness.state_mut().2 = true;
    step_until(&mut harness, "the new policy", |h| {
        editable(h) && keeps_last(h)
    });
}
