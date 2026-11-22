//! Headless checks of the first-run flow, the startup prompt and the Settings tab.

mod common;

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use common::{
    MemorySecrets, back_up_unsent, env, env_with, harness, init_repo, init_repo_with, local_config,
    other_repo_dir, pick, refusing_rest_repo, refusing_server, replace_text, repo_dir, run_settled,
    run_until, run_until_ready, save_config, shows,
};
use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{OutputCommand, ThemePreference};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::config::{AppConfig, ListSort, PasswordMode, RepoOption, SortKey, ThemeChoice};
use salvajocs_core::custom_games::{self, Stored};
use salvajocs_core::layout::RootKind;
use salvajocs_core::manifest::{CustomGame, GameEntry, Integration};
use salvajocs_core::retention::{self, Retention, Unit};
use salvajocs_core::secret::SecretKey;
use salvajocs_core::storage::{Compression, RepoLocation, Storage};
use salvajocs_gui::env::SecretStore;
use salvajocs_gui::tabs::Tab;
use salvajocs_gui::{App, Env};

fn fill(harness: &mut Harness<'static, App>, label: &str, text: &str) {
    let field = harness.get_by_label(label);
    field.focus();
    field.type_text(text);
    run_settled(harness);
}

/// Opens the dropdown named `label` and picks `entry`.
fn choose(harness: &mut Harness<'static, App>, label: &str, entry: &str) {
    // A dropdown that just appeared (after a `run_until`) can still move; click where it settles.
    run_settled(harness);
    harness.get_by_label(label).click();
    run_settled(harness);
    // The long lists scroll; a click lands on the center of the entry, which must be on screen.
    harness.get_by_label(entry).scroll_to_me();
    run_settled(harness);
    harness.get_by_label(entry).click();
    run_settled(harness);
}

/// Whether the Settings compression dropdown has `entry` selected; the selection is the value
/// of the node, not a label.
fn selects(harness: &Harness<'static, App>, entry: &str) -> bool {
    harness
        .query_all_by_label("Compression")
        .any(|node| node.value().as_deref() == Some(entry))
}

/// Waits until the saved config satisfies `ready`; saving is a background job.
fn run_until_saved(
    harness: &mut Harness<'static, App>,
    env: &Env,
    ready: impl Fn(&AppConfig) -> bool,
) {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the config to be saved", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .is_some_and(|config| ready(&config))
    });
}

/// An app with the repository at `repo_dir` open and the Settings tab in front.
fn open_settings(env: Env) -> Harness<'static, App> {
    let mut harness = harness(env);
    run_until_ready(&mut harness);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);
    harness
}

/// Applies `repository` as the new location from the Settings form, confirms its creation and
/// waits until the app saved it.
fn apply_new_location(harness: &mut Harness<'static, App>, env: &Env, repository: &str) {
    replace_text(harness, "Location", repository);
    harness.get_by_label("Apply").click();
    run_until(harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });
    harness.get_by_label("Create").click();
    run_until_saved(harness, env, |config| config.repository == repository);
    // The stage changes in the frame that adopts the repository and is drawn in the next one.
    run_settled(harness);
}

#[test]
fn missing_repo_asks_before_init() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });

    assert!(
        shows(&harness, &repository),
        "the question names the location"
    );
    assert!(
        !shows(&harness, "Encryption password"),
        "the form is gone while the question is asked"
    );
    assert!(
        !shows(&harness, "refuses your login"),
        "a local folder needs no warning about the login"
    );
    assert!(
        !Path::new(&repository).join("config").exists(),
        "nothing is created before the user confirms"
    );
    assert!(
        !env.config_dir.join("config.yaml").exists(),
        "no config is saved before the repository exists"
    );

    harness.get_by_label("Create").click();
    let creating_seen = std::cell::Cell::new(false);
    run_until(&mut harness, "the dialog to close", |h| {
        if shows(h, "Creating the repository") {
            creating_seen.set(true);
            assert!(
                !shows(h, "Encryption password"),
                "the form stays hidden while the repository is created"
            );
        }
        !shows(h, "Welcome to salvajocs")
    });
    assert!(creating_seen.get(), "the creation was observed");
    assert!(Path::new(&repository).join("config").exists());
    run_until_saved(&mut harness, &env, |config| config.repository == repository);
}

#[test]
fn back_from_the_question_shows_the_form_with_its_values() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });
    assert!(shows(&harness, "Compression (new repository)"));
    harness.get_by_label("Back").click();
    run_settled(&mut harness);

    assert!(shows(&harness, "Encryption password"), "the form is back");
    assert!(!shows(&harness, "Do you want to create one?"));
    assert!(
        harness
            .get_by_label("Location")
            .value()
            .is_some_and(|value| value == repository),
        "the typed location is kept"
    );
    assert!(!Path::new(&repository).join("config").exists());
}

#[test]
fn first_run_creates_the_repository_with_the_chosen_compression() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    let mut harness = harness(env);
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });
    choose(
        &mut harness,
        "Compression (new repository)",
        "Smallest (level 19)",
    );
    harness.get_by_label("Create").click();
    run_until(&mut harness, "the dialog to close", |h| {
        !shows(h, "Welcome to salvajocs")
    });

    let location = RepoLocation::new(repository, Default::default());
    let reopened = Storage::open(&location, "").unwrap();
    assert_eq!(reopened.compression(), Compression::Level(19));
}

#[test]
fn a_custom_compression_level_out_of_range_shows_an_error_and_blocks_create() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repo_dir(&tmp));
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });
    assert!(!shows(&harness, "out of range"));

    choose(&mut harness, "Compression (new repository)", "Custom level");
    fill(&mut harness, "Level number", "99");
    assert!(
        shows(&harness, "Level 99 is out of range."),
        "99 is not a zstd level"
    );
    assert!(shows(&harness, "1 to 22"), "the message says what is valid");
    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Create")).is_disabled(),
        "an invalid level cannot be submitted"
    );

    replace_text(&mut harness, "Level number", "-7");
    assert!(!shows(&harness, "out of range"));
    assert!(
        !egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Create"))
            .is_disabled(),
        "-7 is a level zstd accepts"
    );
}

#[test]
fn backend_option_marked_secret_not_written_to_config() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &repository);
    fill(&mut harness, "Encryption password", "pw-1");
    harness.get_by_label("Advanced options").click();
    run_settled(&mut harness);
    harness.get_by_label("Add option").click();
    run_settled(&mut harness);
    fill(&mut harness, "Option 1 key", "access_key");
    fill(&mut harness, "Option 1 value", "hunter2");
    // The "secret" box is already ticked: the key says `key`, and nothing was clicked.
    run_settled(&mut harness);
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });
    harness.get_by_label("Create").click();
    run_until(&mut harness, "the dialog to close", |h| {
        !shows(h, "Welcome to salvajocs")
    });
    run_until_saved(&mut harness, &env, |_| true);

    let yaml = std::fs::read_to_string(env.config_dir.join("config.yaml")).unwrap();
    assert!(yaml.contains("access_key"), "the key stays in the config");
    assert!(!yaml.contains("hunter2"), "the value does not");
    assert!(!yaml.contains("pw-1"), "neither does the password");
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.password, PasswordMode::Keyring);
    let scope = saved.secret_scope();
    assert_eq!(
        secrets
            .get(&scope, &SecretKey::Option("access_key".into()))
            .unwrap()
            .as_deref(),
        Some("hunter2")
    );
}

#[test]
fn an_advanced_key_the_backend_does_not_read_gets_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });
    choose(&mut harness, "Backend", "WebDAV");
    harness.get_by_label("Advanced options").click();
    run_settled(&mut harness);
    harness.get_by_label("Add option").click();
    run_settled(&mut harness);

    fill(&mut harness, "Option 1 key", "disable_create_dirs");
    assert!(shows(&harness, "Not an option of this backend"), "a typo");

    replace_text(&mut harness, "Option 1 key", "Disable_Create_Dir");
    assert!(
        !shows(&harness, "Not an option of this backend"),
        "any letter case"
    );

    replace_text(&mut harness, "Option 1 key", "retry");
    assert!(
        !shows(&harness, "Not an option of this backend"),
        "rustic's own key"
    );
}

#[test]
fn webdav_needs_its_server_url() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    choose(&mut harness, "Backend", "WebDAV");
    assert!(shows(&harness, "Server URL"));
    assert!(
        shows(&harness, "A WebDAV server, such as Nextcloud."),
        "the hint says what the backend is"
    );
    assert!(
        !shows(&harness, "Windows builds"),
        "the sftp note belongs to sftp"
    );
    assert!(
        shows(&harness, "Server password"),
        "the backend login is not called Password"
    );
    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Continue"))
            .is_disabled(),
        "the server URL is required"
    );

    fill(&mut harness, "Server URL", "https://dav.example.com");
    assert!(
        !egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Continue"))
            .is_disabled()
    );
}

#[test]
fn choosing_an_rclone_remote_sets_the_start_of_the_location() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    env.rclone_remotes = || Ok(vec!["gdrive".to_string(), "nas".to_string()]);
    let mut harness = harness(env);
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    choose(&mut harness, "Backend", "rclone remote");
    run_until(&mut harness, "the remote list", |h| {
        h.query_all_by_label("Remote").next().is_some()
    });
    choose(&mut harness, "Remote", "nas");
    assert_eq!(
        harness.get_by_label("Location").value().as_deref(),
        Some("nas:")
    );

    fill(&mut harness, "Location", "games");
    choose(&mut harness, "Remote", "gdrive");
    assert_eq!(
        harness.get_by_label("Location").value().as_deref(),
        Some("gdrive:games"),
        "the path after the colon is kept"
    );
}

#[test]
fn a_missing_rclone_is_said_and_the_location_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    choose(&mut harness, "Backend", "rclone remote");
    run_until(&mut harness, "the rclone note", |h| {
        shows(h, "rclone is not installed")
    });

    assert!(harness.query_all_by_label("Remote").next().is_none());
    fill(&mut harness, "Location", "nas:games");
    assert!(
        !egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Continue"))
            .is_disabled()
    );
}

#[test]
fn a_server_password_does_not_follow_the_user_to_another_service() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    choose(&mut harness, "Backend", "WebDAV");
    fill(&mut harness, "Server password", "hunter2");
    choose(&mut harness, "Backend", "pCloud");
    fill(&mut harness, "API endpoint", "https://api.pcloud.com");
    fill(&mut harness, "Account email", "me@example.com");

    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Continue"))
            .is_disabled(),
        "the pCloud account password is required and was not typed"
    );
}

#[test]
fn applying_an_untouched_form_keeps_the_free_options() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.repository_options = vec![RepoOption::new("retry".into(), Some("3".into()), false)];
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    assert!(
        shows(&harness, "Option 1 key"),
        "a loaded option is not hidden behind the toggle"
    );
    // Only an Apply writes the file again.
    std::fs::remove_file(env.config_dir.join("config.yaml")).unwrap();
    harness.get_by_label("Apply").click();
    run_until_saved(&mut harness, &env, |saved| {
        saved.repository_options == config.repository_options
    });
}

#[test]
fn startup_prompts_for_missing_secret_option() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let mut config = local_config(&repo_dir(&tmp));
    config
        .repository_options
        .push(RepoOption::new("access_key".into(), None, true));
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the prompt", |h| {
        shows(h, "Repository secrets")
    });

    assert!(shows(&harness, "Option access_key"));
    assert!(
        !shows(&harness, "Encryption password"),
        "only the missing secrets are asked for"
    );
}

#[test]
fn startup_names_a_missing_backend_secret_by_its_field() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let mut config = local_config("opendal:webdav");
    config.repository_options = vec![
        RepoOption::new(
            "endpoint".into(),
            Some("https://dav.example.com".into()),
            false,
        ),
        RepoOption::new("password".into(), None, true),
    ];
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the prompt", |h| {
        shows(h, "Repository secrets")
    });

    assert!(shows(&harness, "Server password"));
    assert!(!shows(&harness, "Option password"));
}

#[test]
fn a_wrong_keyring_password_prompts_instead_of_looping() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "right", Compression::Default);
    let mut config = local_config(&repository);
    config.password = PasswordMode::Keyring;
    let scope = config.secret_scope();
    secrets.set(&scope, &SecretKey::Password, "bad").unwrap();
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the password prompt", |h| {
        shows(h, "Wrong encryption password")
    });
    fill(&mut harness, "Encryption password", "right");
    harness.get_by_label("Open repository").click();

    run_until(&mut harness, "the repository to open", |h| {
        !shows(h, "Repository secrets") && !shows(h, "Opening the repository")
    });
    assert!(!shows(&harness, "could not be opened"));
    let store = secrets.clone();
    run_until(
        &mut harness,
        "the keyring to hold the right password",
        move |_| store.get(&scope, &SecretKey::Password).unwrap().as_deref() == Some("right"),
    );
}

const REMEMBER: &str = "Remember passwords in the system keyring";

/// A repository at `repository` without encryption password whose secret option `access_key`
/// is in `secrets` when `remembered`.
fn server_password_config(
    env: &Env,
    secrets: &MemorySecrets,
    repository: &str,
    remembered: bool,
) -> AppConfig {
    init_repo(repository);
    let mut config = local_config(repository);
    config
        .repository_options
        .push(RepoOption::new("access_key".into(), None, true));
    if remembered {
        let key = SecretKey::Option("access_key".into());
        secrets.set(&config.secret_scope(), &key, "ak").unwrap();
    }
    save_config(env, &config);
    config
}

fn remember_box(harness: &Harness<'static, App>) -> Option<Toggled> {
    harness
        .get_by_role_and_label(Role::CheckBox, REMEMBER)
        .accesskit_node()
        .toggled()
}

#[test]
fn a_server_password_from_the_keyring_stays_remembered_after_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let config = server_password_config(&env, &secrets, &repo_dir(&tmp), true);
    let mut harness = open_settings(env.clone());

    assert_eq!(remember_box(&harness), Some(Toggled::True));
    // Only an Apply writes the file again, after it stored or deleted the secrets.
    std::fs::remove_file(env.config_dir.join("config.yaml")).unwrap();
    harness.get_by_label("Apply").click();
    run_until_saved(&mut harness, &env, |saved| {
        saved.repository == config.repository
    });

    let key = SecretKey::Option("access_key".into());
    assert_eq!(
        secrets
            .get(&config.secret_scope(), &key)
            .unwrap()
            .as_deref(),
        Some("ak")
    );
}

#[test]
fn a_server_password_typed_at_startup_leaves_remember_unticked() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    server_password_config(&env, &secrets, &repo_dir(&tmp), false);
    let mut harness = harness(env);
    run_until(&mut harness, "the prompt", |h| {
        shows(h, "Repository secrets")
    });
    fill(&mut harness, "Option access_key", "ak");
    harness.get_by_label("Open repository").click();
    run_until(&mut harness, "the repository to open", |h| {
        !shows(h, "Repository secrets") && !shows(h, "Opening the repository")
    });

    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert_eq!(remember_box(&harness), Some(Toggled::False));
}

#[test]
fn unreadable_config_blocks_settings_and_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    std::fs::create_dir_all(&env.config_dir).unwrap();
    let path = env.config_dir.join("config.yaml");
    std::fs::write(&path, "repository: [unclosed").unwrap();
    let mut harness = harness(env);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert!(shows(&harness, "cannot be read"));
    assert!(!shows(&harness, "Location"), "no form can overwrite it");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "repository: [unclosed"
    );
}

#[test]
fn settings_about_shows_the_build_and_copies_it() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env);

    assert!(shows(&harness, "About"));
    assert!(shows(&harness, env!("CARGO_PKG_VERSION")));
    assert!(shows(&harness, "Commit"));
    assert!(shows(&harness, "Storage library"));
    harness.get_by_label("Copy").scroll_to_me();
    run_settled(&mut harness);
    harness.get_by_label("Copy").click();
    // The command lasts one frame, so read it right after the click is handled.
    harness.step();
    let copied: Vec<String> = harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(copied.len(), 1);
    assert!(copied[0].starts_with(concat!("Version: ", env!("CARGO_PKG_VERSION"))));
    assert!(copied[0].contains("\nTarget: "));
}

#[test]
fn settings_shows_the_open_repository_compression() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "", Compression::Level(19));
    save_config(&env, &local_config(&repository));
    let harness = open_settings(env);

    assert!(shows(&harness, &format!("Current: {repository}")));
    assert!(
        selects(&harness, "Smallest (level 19)"),
        "the dropdown shows the repository's level"
    );
}

#[test]
fn settings_shows_a_stored_level_without_a_preset_as_a_custom_level() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "", Compression::Level(5));
    save_config(&env, &local_config(&repository));
    let harness = open_settings(env);

    assert!(selects(&harness, "Custom level"));
    assert!(
        harness
            .get_by_label("Level number")
            .value()
            .is_some_and(|value| value == "5"),
        "the number field holds the stored level"
    );
    assert!(
        egui_kittest::kittest::NodeT::accesskit_node(&harness.get_by_label("Apply compression"))
            .is_disabled(),
        "nothing changed, so there is nothing to apply"
    );
}

#[test]
fn without_roots_settings_says_which_folders_are_found_anyway() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());
    assert!(shows(&harness, "No roots yet."));

    fill(&mut harness, "Root folder", "/games");
    harness.get_by_label("Add root").click();
    run_settled(&mut harness);
    assert!(!shows(&harness, "No roots yet."));
}

#[test]
fn browse_asks_the_env_for_the_root_folder_and_the_repository_location() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let picked = tmp.path().join("picked");
    pick(&mut env, picked.clone());
    let mut harness = open_settings(env);

    // The repository form's Browse comes first, the roots' second.
    let browse = |h: &Harness<'static, App>, nth: usize| {
        h.query_all_by_label("Browse").nth(nth).unwrap().click();
    };
    let value = |h: &Harness<'static, App>, label| h.get_by_label(label).accesskit_node().value();
    browse(&harness, 0);
    run_until(&mut harness, "the location", |h| {
        value(h, "Location").as_deref() == picked.to_str()
    });
    browse(&harness, 1);
    run_until(&mut harness, "the root folder", |h| {
        value(h, "Root folder").as_deref() == picked.to_str()
    });
}

#[test]
fn applying_a_new_location_keeps_roots_added_since_the_form_was_built() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());

    fill(&mut harness, "Root folder", "/games");
    harness.get_by_label("Add root").click();
    run_settled(&mut harness);
    run_until_saved(&mut harness, &env, |config| config.roots.len() == 1);

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);

    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.roots.len(), 1, "the root added before Apply stays");
    assert_eq!(saved.roots[0].kind, RootKind::Other);
}

/// Launcher prefixes are found on Linux only (ADR-0017).
#[cfg(target_os = "linux")]
#[test]
fn unticking_a_found_prefix_ignores_it_and_ticking_uses_it_again() {
    use std::fs;

    use salvajocs_core::launcher::{Launcher, LauncherGame, LauncherPrefix};
    use salvajocs_core::manifest::fetch::JSON_FILE;
    use salvajocs_core::manifest::{self, parse_yaml};

    let tmp = tempfile::tempdir().unwrap();
    let mut env = env(&tmp);
    // Two Heroic games without their own prefix share the default one.
    let prefix = tmp.path().join("heroic");
    for title in ["Foo™", "Bar"] {
        let mut game = LauncherGame::default();
        game.title = Some(title.to_owned());
        let found = LauncherPrefix::new(Launcher::Heroic, prefix.clone(), Some(game));
        env.environment
            .as_mut()
            .unwrap()
            .launcher_prefixes
            .push(found);
    }
    fs::create_dir_all(&env.cache_dir).unwrap();
    let manifest = parse_yaml("Foo: {}\nBar: {}\n").unwrap();
    fs::write(
        env.cache_dir.join(JSON_FILE),
        manifest::to_json(&manifest).unwrap(),
    )
    .unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());
    let label = format!("{} (Heroic: Foo, Bar)", prefix.display());
    run_until(&mut harness, "the found prefix", |h| shows(h, &label));
    let path = prefix.display().to_string();
    assert_eq!(harness.query_all_by_label_contains(&path).count(), 1);

    harness.get_by_label(&label).click();
    let ignored = [prefix.clone()];
    run_until_saved(&mut harness, &env, |config| {
        config.ignored_prefixes == ignored
    });
    harness.get_by_label(&label).click();
    run_until_saved(&mut harness, &env, |config| {
        config.ignored_prefixes.is_empty()
    });
}

#[test]
fn applying_a_new_location_keeps_the_ignored_prefixes() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.ignored_prefixes = vec![tmp.path().join("prefix")];
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);

    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.ignored_prefixes, config.ignored_prefixes);
}

/// The settings snapshots of the repository at `repository`.
fn settings_snapshots(repository: &str) -> usize {
    let location = RepoLocation::new(repository.to_owned(), Default::default());
    let storage = Storage::open(&location, "").unwrap();
    storage.snapshots("salvajocs-settings").unwrap().len()
}

#[test]
fn settings_new_repository_gets_this_pcs_custom_games_at_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    let mut game = CustomGame::new("Mine".into(), GameEntry::default(), Integration::Extend);
    game.wine_prefix = Some(tmp.path().join("prefix"));
    config.custom_games = vec![game.clone()];
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());
    run_until_saved(&mut harness, &env, |config| {
        config.custom_games_repository.is_some()
    });

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);
    run_until_saved(&mut harness, &env, |config| {
        config.custom_games_repository == Some(config.secret_scope())
    });
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.custom_games, [game.clone()], "the prefix stays");
    assert_eq!(saved.custom_games_unsent.len(), 1, "the change goes along");
    // Neither repository holds a setting before the Back up.
    assert_eq!(settings_snapshots(&repository), 0);
    assert_eq!(settings_snapshots(&other), 0);

    back_up_unsent(&mut harness, "Saved 1 custom game to the repository");
    let mut stored = game;
    stored.wine_prefix = None;
    let location = RepoLocation::new(other, Default::default());
    let listed = custom_games::load_all(&mut Storage::open(&location, "").unwrap()).unwrap();
    assert!(
        matches!(&listed["Mine"], Ok(Stored::Game(game)) if **game == stored),
        "{listed:?}"
    );
    assert_eq!(settings_snapshots(&repository), 0);
}

/// Applies the existing repository `repository` from the Settings form and waits until the app
/// saved it.
fn apply_existing_location(harness: &mut Harness<'static, App>, env: &Env, repository: &str) {
    replace_text(harness, "Location", repository);
    harness.get_by_label("Apply").click();
    run_until_saved(harness, env, |config| config.repository == repository);
    run_settled(harness);
}

#[test]
fn unsent_custom_games_survive_a_switch_and_back() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let first = repo_dir(&tmp);
    init_repo(&first);
    let second = other_repo_dir(&tmp);
    init_repo(&second);
    let open = |repository: &str| {
        let location = RepoLocation::new(repository.to_owned(), Default::default());
        Storage::open(&location, "").unwrap()
    };
    let theirs = CustomGame::new("Theirs".into(), GameEntry::default(), Integration::Extend);
    let stage = tmp.path().join("elsewhere");
    custom_games::save(&mut open(&second), "Theirs", None, Some(&theirs), &stage).unwrap();
    let mine = CustomGame::new("Mine".into(), GameEntry::default(), Integration::Extend);
    let mut config = local_config(&first);
    config.custom_games = vec![mine.clone()];
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());
    run_until_saved(&mut harness, &env, |config| {
        config.custom_games_repository == Some(config.secret_scope())
    });

    apply_existing_location(&mut harness, &env, &second);
    run_until_saved(&mut harness, &env, |config| config.custom_games.len() == 2);
    let unsent = |config: &AppConfig| -> Vec<String> {
        (config.custom_games_unsent.iter())
            .map(|entry| entry.name.clone())
            .collect()
    };
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.custom_games, [mine.clone(), theirs.clone()]);
    assert_eq!(unsent(&saved), ["Mine"]);

    apply_existing_location(&mut harness, &env, &first);
    run_until_saved(&mut harness, &env, |config| {
        config.custom_games_repository == Some(config.secret_scope())
            && config.custom_games_unsent.len() == 2
    });
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.custom_games, [mine, theirs.clone()]);
    assert_eq!(unsent(&saved), ["Mine", "Theirs"]);
    assert_eq!(settings_snapshots(&first), 0);
    assert_eq!(settings_snapshots(&second), 1);

    back_up_unsent(&mut harness, "Saved 2 custom games to the repository");
    let listed = custom_games::load_all(&mut open(&first)).unwrap();
    assert!(
        matches!(&listed["Theirs"], Ok(Stored::Game(game)) if **game == theirs),
        "{listed:?}"
    );
}

#[test]
fn applying_a_new_location_keeps_the_sorts() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.backup_sort = ListSort::new(SortKey::SpaceUsed, true);
    config.restore_sort = ListSort::new(SortKey::LastBackup, false);
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);

    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.backup_sort, config.backup_sort);
    assert_eq!(saved.restore_sort, config.restore_sort);
}

#[test]
fn changing_location_offers_to_delete_stale_keyring_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let repository = repo_dir(&tmp);
    init_repo_with(&repository, "pw", Compression::Default);
    let mut config = local_config(&repository);
    config.password = PasswordMode::Keyring;
    let old_scope = config.secret_scope();
    secrets.set(&old_scope, &SecretKey::Password, "pw").unwrap();
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);
    run_until(&mut harness, "the cleanup offer", |h| {
        shows(h, "Delete the keyring entries")
    });
    assert!(
        secrets
            .get(&old_scope, &SecretKey::Password)
            .unwrap()
            .is_some(),
        "nothing is deleted before the user agrees"
    );

    harness.get_by_label("Delete entries").click();
    let check = secrets.clone();
    run_until(&mut harness, "the old entry to go", move |_| {
        check
            .get(&old_scope, &SecretKey::Password)
            .unwrap()
            .is_none()
    });
}

/// Starts on `repository` with the failure `message` showing, then chooses another repository
/// in Settings and expects the failure gone.
fn choosing_a_repository_clears_the_failure(repository: &str, message: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    save_config(&env, &local_config(repository));
    let mut harness = harness(env.clone());
    run_until(&mut harness, "the failure", |h| shows(h, message));
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    apply_new_location(&mut harness, &env, &other_repo_dir(&tmp));

    assert!(!shows(&harness, message));
    assert!(!shows(&harness, "Retry"));
}

#[test]
fn choosing_a_repository_in_settings_clears_the_missing_repository() {
    let tmp = tempfile::tempdir().unwrap();
    // Nothing was ever created there.
    choosing_a_repository_clears_the_failure(&repo_dir(&tmp), "No repository at");
}

#[test]
fn choosing_a_repository_in_settings_clears_the_open_failure() {
    choosing_a_repository_clears_the_failure("nosuchbackend:somewhere", "could not be opened");
}

#[test]
fn retry_opens_a_repository_that_was_created_meanwhile() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env);
    run_until(&mut harness, "the guidance", |h| {
        shows(h, "No repository at")
    });
    run_settled(&mut harness);

    init_repo(&repository);
    harness.get_by_label("Retry").click();

    run_until(&mut harness, "the repository to open", |h| {
        !shows(h, "No repository at") && !shows(h, "Opening the repository")
    });
    assert!(!shows(&harness, "Retry"));
}

#[test]
fn open_settings_after_a_missing_repository_keeps_the_password() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let mut config = local_config(&repo_dir(&tmp));
    config.password = PasswordMode::Keyring;
    secrets
        .set(&config.secret_scope(), &SecretKey::Password, "pw")
        .unwrap();
    save_config(&env, &config);
    let mut harness = harness(env);
    run_until(&mut harness, "the guidance", |h| {
        shows(h, "No repository at")
    });
    run_settled(&mut harness);

    harness.get_by_label("Open Settings").click();
    run_settled(&mut harness);

    assert!(
        !shows(&harness, "No encryption password"),
        "the form starts with the password, so re-creating keeps the repository encrypted"
    );
}

#[test]
fn a_server_that_may_refuse_the_login_is_warned_about_before_creating() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    fill(&mut harness, "Location", &refusing_rest_repo());
    harness.get_by_label("Continue").click();
    run_until(&mut harness, "the confirmation", |h| {
        shows(h, "Do you want to create one?")
    });

    assert!(shows(&harness, "refuses your login"));
}

const LOGIN_REFUSED: &str = "The server refused the login. Check the user name and password.";

#[test]
fn a_refused_login_is_said_in_the_form() {
    let tmp = tempfile::tempdir().unwrap();
    let mut harness = harness(env(&tmp));
    run_until(&mut harness, "the first-run dialog", |h| {
        shows(h, "Welcome to salvajocs")
    });

    choose(&mut harness, "Backend", "WebDAV");
    fill(&mut harness, "Server URL", &refusing_server());
    harness.get_by_label("Continue").click();

    run_until(&mut harness, "the refusal", |h| shows(h, LOGIN_REFUSED));
    assert!(!shows(&harness, "Do you want to create one?"));
}

#[test]
fn a_refused_login_at_startup_is_said() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let mut config = local_config("opendal:webdav");
    config.repository_options = vec![RepoOption::new(
        "endpoint".into(),
        Some(refusing_server()),
        false,
    )];
    save_config(&env, &config);
    let mut harness = harness(env);

    run_until(&mut harness, "the failure", |h| {
        shows(h, "could not be opened")
    });

    assert!(shows(
        &harness,
        &format!("The repository could not be opened: {LOGIN_REFUSED}")
    ));
    assert!(!shows(&harness, "No repository at"));
}

#[test]
fn after_a_refused_login_settings_keeps_the_remembered_server_password() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let mut config = local_config("opendal:webdav");
    config.repository_options = vec![
        RepoOption::new("endpoint".into(), Some(refusing_server()), false),
        RepoOption::new("password".into(), None, true),
    ];
    let key = SecretKey::Option("password".into());
    secrets.set(&config.secret_scope(), &key, "old").unwrap();
    save_config(&env, &config);
    let mut harness = harness(env);
    run_until(&mut harness, "the failure", |h| shows(h, LOGIN_REFUSED));

    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert_eq!(remember_box(&harness), Some(Toggled::True));
}

#[test]
fn after_a_rest_refusal_settings_keeps_the_remembered_server_password() {
    let tmp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let env = env_with(&tmp, secrets.clone());
    let mut config = local_config(&refusing_rest_repo());
    config.repository_options = vec![RepoOption::new("password".into(), None, true)];
    let key = SecretKey::Option("password".into());
    secrets.set(&config.secret_scope(), &key, "old").unwrap();
    save_config(&env, &config);
    let mut harness = harness(env);
    run_until(&mut harness, "the failure", |h| {
        shows(h, "could not be opened")
    });

    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert_eq!(remember_box(&harness), Some(Toggled::True));
}

#[test]
fn a_missing_repository_points_to_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    // Nothing was ever created there, as if the repository had been deleted.
    let repository = repo_dir(&tmp);
    save_config(&env, &local_config(&repository));
    let mut harness = harness(env);
    run_until(&mut harness, "the guidance", |h| {
        shows(h, "No repository at")
    });
    run_settled(&mut harness);

    assert!(
        shows(&harness, &repository),
        "the message names the location"
    );
    assert!(!shows(&harness, "Roots"), "Settings is not in front yet");
    harness.get_by_label("Open Settings").click();
    run_settled(&mut harness);
    assert!(
        shows(&harness, "Roots"),
        "the button shows the Settings tab"
    );
}

#[test]
fn other_open_failures_keep_their_message_and_have_no_settings_button() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    save_config(&env, &local_config("nosuchbackend:somewhere"));
    let mut harness = harness(env);
    run_until(&mut harness, "the failure", |h| {
        shows(h, "could not be opened")
    });

    assert!(!shows(&harness, "No repository at"));
    assert!(harness.query_by_label("Open Settings").is_none());
}

const KEEP_LAST: &str = "Keep the last";
const KEEP_WITHIN: &str = "Keep every backup newer than";

fn location(repository: &str) -> RepoLocation {
    RepoLocation::new(repository.to_string(), Default::default())
}

/// Waits until this PC's unsent policy satisfies `ready`; saving the config is a background job.
fn run_until_unsent(
    harness: &mut Harness<'static, App>,
    env: &Env,
    ready: impl Fn(&Retention) -> bool,
) {
    let config_dir = env.config_dir.clone();
    run_until(harness, "the policy to be kept", move |_| {
        AppConfig::load(&config_dir)
            .ok()
            .flatten()
            .and_then(|config| config.retention_unsent)
            .is_some_and(|unsent| ready(&unsent.policy))
    });
}

/// Waits until the repository holds a policy satisfying `ready`; saving is a background job.
fn run_until_policy(
    harness: &mut Harness<'static, App>,
    repository: &str,
    ready: impl Fn(&Retention) -> bool,
) {
    let location = location(repository);
    run_until(harness, "the policy to be saved", move |_| {
        Storage::open(&location, "")
            .ok()
            .and_then(|mut storage| retention::load(&mut storage).ok())
            .is_some_and(|policy| ready(&policy))
    });
}

fn is_disabled(harness: &Harness<'static, App>, role: Role, label: &str) -> bool {
    harness
        .get_by_role_and_label(role, label)
        .accesskit_node()
        .is_disabled()
}

/// The app on the repository at `repository` with the Settings tab in front, once the policy
/// has loaded.
fn open_retention(env: Env, repository: &str) -> Harness<'static, App> {
    save_config(&env, &local_config(repository));
    let mut harness = open_settings(env);
    run_until(&mut harness, "the policy to load", |h| {
        !is_disabled(h, Role::CheckBox, KEEP_LAST)
    });
    harness
}

#[test]
fn old_backups_are_disabled_before_the_repository_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    // Nothing was ever created there, so opening it fails.
    save_config(&env, &local_config(&repo_dir(&tmp)));
    let mut harness = harness(env);
    run_until(&mut harness, "the failure", |h| {
        shows(h, "No repository at")
    });
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);

    assert!(is_disabled(&harness, Role::CheckBox, KEEP_LAST));
    assert!(is_disabled(&harness, Role::SpinButton, KEEP_LAST));
    assert!(is_disabled(&harness, Role::CheckBox, KEEP_WITHIN));
    assert!(is_disabled(&harness, Role::ComboBox, KEEP_WITHIN));
}

#[test]
fn ticking_keep_last_saves_the_policy_in_the_repository_at_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut harness = open_retention(env.clone(), &repository);
    assert!(shows(&harness, "All backups are kept"));

    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    run_until_unsent(&mut harness, &env, |policy| {
        policy.keep_last.is_some() && policy.keep_within.is_none()
    });
    assert!(!shows(&harness, "All backups are kept"));
    assert!(shows(
        &harness,
        "Saved to the repository with the next Back up."
    ));
    assert_eq!(
        settings_snapshots(&repository),
        0,
        "the repository is unchanged"
    );

    back_up_unsent(&mut harness, "Saved the Old backups rules");
    run_until_policy(&mut harness, &repository, |policy| {
        policy.keep_last.is_some() && policy.keep_within.is_none()
    });
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.retention_unsent, None);
    harness.state_mut().set_tab(Tab::Settings);
    run_settled(&mut harness);
    assert!(!shows(
        &harness,
        "Saved to the repository with the next Back up."
    ));
}

#[test]
fn keeping_the_repositorys_rules_drops_this_pcs_and_shows_them() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut stored = Retention::default();
    stored.keep_last = NonZeroU32::new(5);
    let mut storage = Storage::open(&location(&repository), "").unwrap();
    retention::save(&mut storage, &stored, &tmp.path().join("stage")).unwrap();
    let mut config = local_config(&repository);
    let mut mine = Retention::default();
    mine.keep_last = NonZeroU32::new(3);
    config.retention_unsent = Some(retention::Unsent::new(mine, Retention::default()));
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());
    let shown = |h: &Harness<'static, App>, n: &str| {
        h.query_by_role_and_label(Role::SpinButton, KEEP_LAST)
            .and_then(|spin| spin.value())
            .as_deref()
            == Some(n)
    };
    run_until(&mut harness, "this PC's rules", |h| shown(h, "3"));
    let written = settings_snapshots(&repository);

    harness.state_mut().set_tab(Tab::Backup);
    run_until(&mut harness, "Back up to be enabled", |h| {
        h.query_by_label("Back up")
            .is_some_and(|button| !button.accesskit_node().is_disabled())
    });
    harness.get_by_label("Back up").click();
    run_until(&mut harness, "the question", |h| {
        shows(h, "Changed on another PC")
    });
    harness.get_by_label("Keep the repository's").click();
    run_settled(&mut harness);
    let button = harness
        .query_all_by_label("Back up")
        .find(|node| !node.accesskit_node().is_disabled())
        .unwrap();
    button.click();
    run_until(&mut harness, "the answer", |h| {
        !shows(h, "Changed on another PC")
    });
    run_settled(&mut harness);

    assert_eq!(
        settings_snapshots(&repository),
        written,
        "nothing was written"
    );
    run_until_saved(&mut harness, &env, |config| {
        config.retention_unsent.is_none()
    });
    harness.state_mut().set_tab(Tab::Settings);
    run_until(&mut harness, "the repository's rules", |h| shown(h, "5"));
    assert!(!shows(
        &harness,
        "Saved to the repository with the next Back up."
    ));
}

#[test]
fn changing_the_unit_saves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut harness = open_retention(env.clone(), &repository);
    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_WITHIN)
        .click();
    run_until_unsent(&mut harness, &env, |policy| {
        policy
            .keep_within
            .is_some_and(|within| within.unit == Unit::Months)
    });

    harness
        .get_by_role_and_label(Role::ComboBox, KEEP_WITHIN)
        .click();
    run_settled(&mut harness);
    harness.get_by_label("weeks").click();
    run_until_unsent(&mut harness, &env, |policy| {
        policy
            .keep_within
            .is_some_and(|within| within.unit == Unit::Weeks)
    });
}

#[test]
fn keep_last_is_never_below_one() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut harness = open_retention(env.clone(), &repository);
    harness
        .get_by_role_and_label(Role::CheckBox, KEEP_LAST)
        .click();
    run_settled(&mut harness);

    harness
        .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
        .set_value_accesskit(0.0);
    run_until_unsent(&mut harness, &env, |policy| {
        policy.keep_last.is_some_and(|n| n.get() == 1)
    });
    assert_eq!(
        harness
            .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
            .value()
            .as_deref(),
        Some("1")
    );
}

#[test]
fn the_stored_policy_is_shown() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut policy = Retention::default();
    policy.keep_last = NonZeroU32::new(4);
    let mut storage = Storage::open(&location(&repository), "").unwrap();
    retention::save(&mut storage, &policy, &tmp.path().join("stage")).unwrap();
    let harness = open_retention(env(&tmp), &repository);

    let checkbox = harness.get_by_role_and_label(Role::CheckBox, KEEP_LAST);
    assert_eq!(checkbox.accesskit_node().toggled(), Some(Toggled::True));
    assert_eq!(
        harness
            .get_by_role_and_label(Role::SpinButton, KEEP_LAST)
            .value()
            .as_deref(),
        Some("4")
    );
}

#[test]
fn an_unreadable_policy_is_reported_and_not_editable() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    // A newer version's policy.
    let dir = tmp.path().join("newer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("retention.json"), r#"{ "format": 2 }"#).unwrap();
    Storage::open(&location(&repository), "")
        .unwrap()
        .backup_dir(
            &dir,
            "/salvajocs-settings/retention",
            &["salvajocs-settings", "settings:retention"],
            "Retention policy",
        )
        .unwrap()
        .unwrap();
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env);
    run_until(&mut harness, "the error", |h| {
        shows(h, "cleanup rules could not be read; nothing is removed")
    });
    assert!(is_disabled(&harness, Role::CheckBox, KEEP_LAST));
}

fn theme_preference(harness: &Harness<'static, App>) -> ThemePreference {
    harness.ctx.options(|options| options.theme_preference)
}

#[test]
fn the_theme_switch_applies_at_once_and_is_saved() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    save_config(&env, &local_config(&repository));
    let mut harness = open_settings(env.clone());

    // The test window starts dark, so Light is the change that shows.
    harness.get_by_label("Light").click();
    run_until_saved(&mut harness, &env, |config| {
        config.theme == ThemeChoice::Light
    });
    assert_eq!(theme_preference(&harness), ThemePreference::Light);

    harness.get_by_label("Dark").click();
    run_until_saved(&mut harness, &env, |config| {
        config.theme == ThemeChoice::Dark
    });
    assert_eq!(theme_preference(&harness), ThemePreference::Dark);
    let yaml = std::fs::read_to_string(env.config_dir.join("config.yaml")).unwrap();
    assert!(yaml.contains("theme: dark"));

    harness.get_by_label("System").click();
    run_until_saved(&mut harness, &env, |config| {
        config.theme == ThemeChoice::System
    });
    assert_eq!(theme_preference(&harness), ThemePreference::System);
}

#[test]
fn a_saved_theme_is_applied_at_start() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.theme = ThemeChoice::Light;
    save_config(&env, &config);
    let harness = open_settings(env);
    assert_eq!(theme_preference(&harness), ThemePreference::Light);
}

#[test]
fn the_theme_survives_a_new_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let env = env(&tmp);
    let repository = repo_dir(&tmp);
    init_repo(&repository);
    let mut config = local_config(&repository);
    config.theme = ThemeChoice::Light;
    save_config(&env, &config);
    let mut harness = open_settings(env.clone());

    let other = other_repo_dir(&tmp);
    apply_new_location(&mut harness, &env, &other);
    let saved = AppConfig::load(&env.config_dir).unwrap().unwrap();
    assert_eq!(saved.theme, ThemeChoice::Light);
}
