//! The repository backend picker shared by first run and Settings (ADR-0009): a kind, a
//! location or the named fields of the service, and an Advanced key/value option list where any
//! option can be marked secret.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

use eframe::egui::{self, TextEdit, Ui};
use salvajocs_core::config::{self, AppConfig, BackendField, BackendKind, RepoOption};

use super::{field_hint, labelled_edit};
use crate::env::Env;
use crate::job::Job;
use crate::theme::{self, icons};

/// The options rustic reads for the local, REST and rclone backends.
const RUSTIC_OPTIONS_DOCS: &str = "https://github.com/rustic-rs/rustic/blob/main/config/full.toml";

/// One row of the Advanced option list. No `Debug`: the value may be a secret.
#[derive(Default)]
struct OptionRow {
    key: String,
    value: String,
    secret: bool,
}

/// An option the form would save, as typed.
struct Entry<'a> {
    key: &'a str,
    value: &'a str,
    secret: bool,
}

/// The rclone remotes: not asked for yet, being listed, or the answer.
enum Remotes {
    Unasked,
    Listing(Job<io::Result<Vec<String>>>),
    Listed(io::Result<Vec<String>>),
}

/// The state of the backend picker.
pub struct BackendForm {
    kind: BackendKind,
    location: String,
    /// Values of the named fields by key, shared across kinds; only the current kind's are saved.
    named: BTreeMap<String, String>,
    /// The Advanced options.
    options: Vec<OptionRow>,
    advanced_open: bool,
    remotes: Remotes,
    picker: Option<Job<Option<PathBuf>>>,
}

impl Default for BackendForm {
    fn default() -> Self {
        Self {
            kind: BackendKind::Local,
            location: String::new(),
            named: BTreeMap::new(),
            options: Vec::new(),
            advanced_open: false,
            remotes: Remotes::Unasked,
            picker: None,
        }
    }
}

/// The named fields of a kind; only opendal services have them.
fn fields_of(kind: BackendKind) -> &'static [BackendField] {
    match kind {
        BackendKind::OpenDal(service) => config::opendal_fields(service),
        _ => &[],
    }
}

/// The picker entry for a kind.
fn kind_label(kind: BackendKind) -> String {
    match kind {
        BackendKind::Local => "Local folder".to_string(),
        BackendKind::Rest => "REST server".to_string(),
        BackendKind::Rclone => "rclone remote".to_string(),
        BackendKind::OpenDal(service) => config::opendal_label(service)
            .unwrap_or(service)
            .to_string(),
    }
}

impl BackendForm {
    /// A form showing the repository of `config`, with secret values taken from `secrets`. A
    /// repository string with a prefix this picker does not offer stays a raw local location, so
    /// it is saved back unchanged.
    pub fn from_config(config: &AppConfig, secrets: &BTreeMap<String, String>) -> Self {
        let repository = config.repository.as_str();
        let (kind, location) = if let Some(rest) = repository.strip_prefix("rest:") {
            (BackendKind::Rest, rest)
        } else if let Some(remote) = repository.strip_prefix("rclone:") {
            (BackendKind::Rclone, remote)
        } else if let Some(kind) = repository.strip_prefix("opendal:").and_then(|service| {
            config::backend_kinds()
                .into_iter()
                .find(|kind| matches!(kind, BackendKind::OpenDal(name) if *name == service))
        }) {
            (kind, "")
        } else {
            (BackendKind::Local, repository)
        };
        let fields = fields_of(kind);
        let mut named = BTreeMap::new();
        let mut options = Vec::new();
        for option in &config.repository_options {
            let value = if option.secret {
                secrets.get(&option.key).cloned()
            } else {
                option.value.clone()
            }
            .unwrap_or_default();
            // An option moves into a field only when key and secret flag both match, so the flag,
            // and with it the keyring scope, is never flipped. Anything else stays Advanced.
            let in_field = fields
                .iter()
                .any(|field| field.key == option.key && field.secret == option.secret)
                && !named.contains_key(&option.key);
            if in_field {
                named.insert(option.key.clone(), value);
            } else {
                options.push(OptionRow {
                    key: option.key.clone(),
                    value,
                    secret: option.secret,
                });
            }
        }
        Self {
            kind,
            location: location.to_string(),
            named,
            advanced_open: !options.is_empty(),
            options,
            remotes: Remotes::Unasked,
            picker: None,
        }
    }

    /// The rustic repository string for the chosen kind and location.
    pub fn repository(&self) -> String {
        match self.kind {
            BackendKind::Local => self.location.trim().to_string(),
            BackendKind::Rest => format!("rest:{}", self.location.trim()),
            BackendKind::Rclone => format!("rclone:{}", self.location.trim()),
            BackendKind::OpenDal(service) => format!("opendal:{service}"),
        }
    }

    /// The named fields first, then the Advanced rows with a key. A named field left empty is
    /// not saved: `""` would change the keyring scope, and opendal may read it as set.
    fn entries(&self) -> Vec<Entry<'_>> {
        let named = fields_of(self.kind).iter().filter_map(|field| {
            let value = self.named.get(field.key)?;
            (!value.trim().is_empty()).then_some(Entry {
                key: field.key,
                value,
                secret: field.secret,
            })
        });
        let advanced = self
            .options
            .iter()
            .filter(|row| !row.key.trim().is_empty())
            .map(|row| Entry {
                key: row.key.trim(),
                value: &row.value,
                secret: row.secret,
            });
        named.chain(advanced).collect()
    }

    /// A key that would be saved twice, which `location()` would resolve by dropping one.
    fn duplicate_key(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        self.entries()
            .into_iter()
            .find(|entry| !seen.insert(entry.key))
            .map(|entry| entry.key.to_string())
    }

    /// Whether the form describes a repository: a location for local, REST and rclone; a value
    /// for each required field of an opendal service, in its field or an Advanced row; and no
    /// key set twice.
    pub fn is_complete(&self) -> bool {
        if self.duplicate_key().is_some() {
            return false;
        }
        match self.kind {
            BackendKind::OpenDal(_) => {
                let entries = self.entries();
                fields_of(self.kind)
                    .iter()
                    .filter(|field| field.required)
                    .all(|field| {
                        entries
                            .iter()
                            .any(|entry| entry.key == field.key && !entry.value.trim().is_empty())
                    })
            }
            _ => !self.location.trim().is_empty(),
        }
    }

    /// The options as the config keeps them: a secret option has no value.
    pub fn options(&self) -> Vec<RepoOption> {
        self.entries()
            .into_iter()
            .map(|entry| RepoOption {
                key: entry.key.to_string(),
                value: (!entry.secret).then(|| entry.value.to_string()),
                secret: entry.secret,
            })
            .collect()
    }

    /// The values of the secret options, by key.
    pub fn secrets(&self) -> BTreeMap<String, String> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.secret)
            .map(|entry| (entry.key.to_string(), entry.value.to_string()))
            .collect()
    }

    /// Draws the picker as rows of a two-column grid. `env` lists the rclone remotes, once, the
    /// first time that kind is shown.
    pub fn ui(&mut self, ui: &mut Ui, env: &Env) {
        if let Some(job) = &mut self.picker
            && let Some(result) = job.take_result()
        {
            self.picker = None;
            if let Ok(Some(folder)) = result {
                self.location = folder.to_string_lossy().into_owned();
            }
        }
        let label = ui.label("Backend");
        egui::ComboBox::from_id_salt("backend_kind")
            .selected_text(kind_label(self.kind))
            .show_ui(ui, |ui| {
                theme::pill_list(ui);
                for kind in config::backend_kinds() {
                    ui.selectable_value(&mut self.kind, kind, kind_label(kind));
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();

        if !matches!(self.kind, BackendKind::OpenDal(_)) {
            if self.kind == BackendKind::Rclone {
                self.poll_remotes(ui.ctx(), env);
                if let Some(note) = self.remotes_note() {
                    ui.label("");
                    field_hint(ui, &note);
                    ui.end_row();
                }
            }
            ui.label("");
            field_hint(
                ui,
                match self.kind {
                    BackendKind::Local => "A folder on this machine or a mounted drive.",
                    BackendKind::Rest => "The URL of a rustic or restic REST server.",
                    _ => "remote:path. Needs the rclone binary on the PATH.",
                },
            );
            ui.end_row();
            if self.kind == BackendKind::Rclone {
                self.remote_ui(ui);
            }
            labelled_edit(ui, "Location", &mut self.location, false);
            if self.kind == BackendKind::Local
                && ui.button("Browse").clicked()
                && self.picker.is_none()
            {
                self.picker = Some(Job::spawn(ui.ctx(), || {
                    Ok(rfd::FileDialog::new().pick_folder())
                }));
            }
            ui.end_row();
        } else {
            ui.label("");
            field_hint(ui, "opendal:sftp is not available in Windows builds.");
            ui.end_row();
            for field in fields_of(self.kind) {
                self.field_ui(ui, field);
            }
        }
        self.duplicate_ui(ui);
        self.advanced_ui(ui);
    }

    /// Starts listing the remotes on first use and collects the answer.
    fn poll_remotes(&mut self, ctx: &egui::Context, env: &Env) {
        match &mut self.remotes {
            Remotes::Unasked => {
                let list = env.rclone_remotes;
                self.remotes = Remotes::Listing(Job::spawn(ctx, move || Ok(list())));
            }
            Remotes::Listing(job) => {
                if let Some(result) = job.take_result() {
                    self.remotes = Remotes::Listed(
                        result.unwrap_or_else(|err| Err(io::Error::other(format!("{err:#}")))),
                    );
                }
            }
            Remotes::Listed(_) => {}
        }
    }

    /// Why there is no remote to choose from, once the listing is back.
    fn remotes_note(&self) -> Option<String> {
        match &self.remotes {
            Remotes::Listed(Err(err)) if err.kind() == io::ErrorKind::NotFound => {
                Some("rclone is not installed or not on the PATH.".to_string())
            }
            Remotes::Listed(Err(err)) => Some(format!("Could not list rclone remotes: {err}")),
            Remotes::Listed(Ok(list)) if list.is_empty() => {
                Some("rclone has no remotes yet. Run rclone config first.".to_string())
            }
            _ => None,
        }
    }

    /// The Remote dropdown, when rclone listed any. Choosing one replaces the part of Location
    /// before the first `:`, which stays the one place the repository is kept.
    fn remote_ui(&mut self, ui: &mut Ui) {
        let Remotes::Listed(Ok(list)) = &self.remotes else {
            return;
        };
        if list.is_empty() {
            return;
        }
        let (current, path) = match self.location.split_once(':') {
            Some((remote, path)) => (remote, path),
            None => (self.location.as_str(), ""),
        };
        let current = list.iter().find(|remote| remote.as_str() == current);
        let label = ui.label("Remote");
        let mut chosen = None;
        egui::ComboBox::from_id_salt("rclone_remote")
            .selected_text(current.map_or("Choose a remote", String::as_str))
            .show_ui(ui, |ui| {
                theme::pill_list(ui);
                for remote in list {
                    let selected = current == Some(remote);
                    if ui.selectable_label(selected, remote).clicked() && !selected {
                        chosen = Some(format!("{remote}:{path}"));
                    }
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
        if let Some(location) = chosen {
            self.location = location;
        }
    }

    /// A named field: its edit, then a hint row when it has a hint or is required.
    fn field_ui(&mut self, ui: &mut Ui, field: &BackendField) {
        let label = ui.label(field.label);
        let value = self.named.entry(field.key.to_string()).or_default();
        ui.add(
            TextEdit::singleline(value)
                .password(field.secret)
                .hint_text(field.example),
        )
        .labelled_by(label.id);
        ui.end_row();
        let hint = match (field.required, field.hint) {
            (true, "") => "Required.".to_string(),
            (true, hint) => format!("Required. {hint}"),
            (false, hint) => hint.to_string(),
        };
        if !hint.is_empty() {
            ui.label("");
            field_hint(ui, &hint);
            ui.end_row();
        }
    }

    /// The error row for a key that would be saved twice.
    fn duplicate_ui(&self, ui: &mut Ui) {
        let Some(key) = self.duplicate_key() else {
            return;
        };
        let message = if fields_of(self.kind).iter().any(|field| field.key == key) {
            format!("\"{key}\" is set in both a field and Advanced options. Remove one.")
        } else {
            format!("\"{key}\" is set twice in Advanced options. Remove one.")
        };
        ui.label("");
        let color = ui.visuals().error_fg_color;
        theme::status(ui, icons::X_CIRCLE, color, message);
        ui.end_row();
    }

    /// The toggle row and, when open, the option reference and the option list. A toggle row
    /// because a `CollapsingHeader` cannot span the rows of a grid.
    fn advanced_ui(&mut self, ui: &mut Ui) {
        ui.label("");
        // A framed button, so it reads as a button while closed; selected while open.
        let toggle = egui::Button::new("Advanced options").selected(self.advanced_open);
        if ui.add(toggle).clicked() {
            self.advanced_open = !self.advanced_open;
        }
        ui.end_row();
        if !self.advanced_open {
            return;
        }
        let docs = match self.kind {
            BackendKind::OpenDal(service) => config::opendal_docs_url(service),
            _ => RUSTIC_OPTIONS_DOCS.to_string(),
        };
        ui.label("");
        ui.hyperlink_to("Option reference", docs);
        ui.end_row();
        self.options_ui(ui);
    }

    fn options_ui(&mut self, ui: &mut Ui) {
        let mut remove = None;
        for (index, row) in self.options.iter_mut().enumerate() {
            let number = index + 1;
            labelled_edit(ui, &format!("Option {number} key"), &mut row.key, false);
            if ui.button("Remove").clicked() {
                remove = Some(index);
            }
            ui.end_row();
            labelled_edit(
                ui,
                &format!("Option {number} value"),
                &mut row.value,
                row.secret,
            );
            ui.checkbox(&mut row.secret, "secret");
            ui.end_row();
        }
        if let Some(index) = remove {
            self.options.remove(index);
        }
        ui.label("");
        if ui.button("Add option").clicked() {
            self.options.push(OptionRow::default());
        }
        ui.end_row();
    }

    /// Adds an option row, for tests that cannot click.
    #[cfg(test)]
    fn push_option(&mut self, key: &str, value: &str, secret: bool) {
        self.options.push(OptionRow {
            key: key.to_string(),
            value: value.to_string(),
            secret,
        });
    }

    /// Sets a named field, for tests that cannot type.
    #[cfg(test)]
    fn set_named(&mut self, key: &str, value: &str) {
        self.named.insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use salvajocs_core::config::PasswordMode;

    use super::*;

    fn config(repository: &str, options: Vec<RepoOption>) -> AppConfig {
        AppConfig {
            repository: repository.to_string(),
            repository_options: options,
            password: PasswordMode::None,
            roots: Vec::new(),
            custom_games: Vec::new(),
            check_updates: true,
        }
    }

    fn option(key: &str, value: Option<&str>, secret: bool) -> RepoOption {
        RepoOption {
            key: key.to_string(),
            value: value.map(str::to_string),
            secret,
        }
    }

    fn keys(options: &[OptionRow]) -> Vec<&str> {
        options.iter().map(|row| row.key.as_str()).collect()
    }

    /// The options as a set, since their order is not part of the config's meaning.
    fn option_set(options: Vec<RepoOption>) -> BTreeSet<(String, Option<String>, bool)> {
        options
            .into_iter()
            .map(|o| (o.key, o.value, o.secret))
            .collect()
    }

    #[test]
    fn repository_strings_round_trip_through_the_form() {
        for repository in [
            "/backups/games",
            "rest:https://host/repo",
            "rclone:remote:games",
            "opendal:s3",
            "opendal:memory",
        ] {
            let form = BackendForm::from_config(&config(repository, Vec::new()), &BTreeMap::new());
            assert_eq!(form.repository(), repository);
        }
    }

    #[test]
    fn a_secret_option_keeps_its_value_out_of_the_config_options() {
        let mut form = BackendForm::default();
        form.push_option("access_key", "hunter2", true);
        form.push_option("bucket", "games", false);
        form.push_option("  ", "ignored", false);

        let options = form.options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].value, None);
        assert!(options[0].secret);
        assert_eq!(options[1].value.as_deref(), Some("games"));
        assert_eq!(
            form.secrets(),
            BTreeMap::from([("access_key".to_string(), "hunter2".to_string())])
        );
    }

    #[test]
    fn from_config_fills_secret_rows_from_the_session_secrets() {
        let option = RepoOption {
            key: "access_key".to_string(),
            value: None,
            secret: true,
        };
        let secrets = BTreeMap::from([("access_key".to_string(), "hunter2".to_string())]);
        let form = BackendForm::from_config(&config("opendal:s3", vec![option]), &secrets);
        assert_eq!(form.secrets(), secrets);
    }

    #[test]
    fn named_and_advanced_options_round_trip() {
        let secrets = BTreeMap::from([("secret_access_key".to_string(), "hunter2".to_string())]);
        let original = config(
            "opendal:s3",
            vec![
                option("bucket", Some("games"), false),
                option("region", Some("eu-west-1"), false),
                option("secret_access_key", None, true),
                option("retry", Some("5"), false),
                option("aws_profile", Some("x"), false),
            ],
        );
        let form = BackendForm::from_config(&original, &secrets);

        assert_eq!(form.named.get("bucket").map(String::as_str), Some("games"));
        assert_eq!(keys(&form.options), ["retry", "aws_profile"]);
        assert!(form.advanced_open, "a loaded Advanced option is not hidden");
        assert_eq!(
            option_set(form.options()),
            option_set(original.repository_options.clone())
        );
        assert_eq!(form.secrets(), secrets);
        let saved = AppConfig {
            repository_options: form.options(),
            ..original.clone()
        };
        assert_eq!(saved.secret_scope(), original.secret_scope());
    }

    #[test]
    fn a_field_stored_with_another_secret_flag_stays_advanced() {
        let original = config(
            "opendal:webdav",
            vec![
                option("endpoint", Some("https://dav.example.com"), false),
                option("password", Some("plain"), false),
            ],
        );
        let form = BackendForm::from_config(&original, &BTreeMap::new());

        assert_eq!(keys(&form.options), ["password"]);
        assert_eq!(
            option_set(form.options()),
            option_set(original.repository_options.clone())
        );
        let saved = AppConfig {
            repository_options: form.options(),
            ..original.clone()
        };
        assert_eq!(saved.secret_scope(), original.secret_scope());
    }

    #[test]
    fn a_required_secret_stored_plain_stays_complete() {
        let original = config(
            "opendal:pcloud",
            vec![
                option("endpoint", Some("https://api.pcloud.com"), false),
                option("username", Some("me@example.com"), false),
                option("password", Some("plain"), false),
            ],
        );
        let form = BackendForm::from_config(&original, &BTreeMap::new());

        assert!(form.is_complete(), "the untouched config still applies");
        assert_eq!(
            option_set(form.options()),
            option_set(original.repository_options.clone())
        );
        let saved = AppConfig {
            repository_options: form.options(),
            ..original.clone()
        };
        assert_eq!(saved.secret_scope(), original.secret_scope());
    }

    #[test]
    fn empty_named_fields_are_not_emitted() {
        let mut form =
            BackendForm::from_config(&config("opendal:s3", Vec::new()), &BTreeMap::new());
        form.set_named("bucket", "games");
        form.set_named("region", "");
        form.set_named("secret_access_key", "  ");

        assert_eq!(
            option_set(form.options()),
            option_set(vec![option("bucket", Some("games"), false)])
        );
        assert!(form.secrets().is_empty());
    }

    #[test]
    fn only_the_current_kinds_named_fields_are_emitted() {
        let mut form =
            BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
        form.set_named("endpoint", "https://dav.example.com");
        form.set_named("bucket", "left over from another service");

        assert_eq!(
            option_set(form.options()),
            option_set(vec![option(
                "endpoint",
                Some("https://dav.example.com"),
                false
            )])
        );
    }

    #[test]
    fn required_fields_gate_is_complete() {
        let mut form =
            BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
        assert!(!form.is_complete());
        form.set_named("endpoint", "   ");
        assert!(!form.is_complete(), "blank is not a value");
        form.set_named("endpoint", "https://dav.example.com");
        assert!(form.is_complete());

        let mut local = BackendForm::default();
        assert!(!local.is_complete(), "a local repository needs a location");
        local.location = "/backups".to_string();
        assert!(local.is_complete());
    }

    #[test]
    fn a_required_field_set_in_an_advanced_row_counts() {
        let mut form =
            BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
        form.push_option("endpoint", "https://dav.example.com", false);
        assert!(form.is_complete());
    }

    #[test]
    fn duplicate_key_blocks_is_complete() {
        let mut form =
            BackendForm::from_config(&config("opendal:webdav", Vec::new()), &BTreeMap::new());
        form.set_named("endpoint", "https://dav.example.com");
        form.push_option("endpoint", "https://other.example.com", false);
        assert!(!form.is_complete());
        assert_eq!(form.duplicate_key().as_deref(), Some("endpoint"));

        form.options.clear();
        form.push_option("retry", "1", false);
        form.push_option("retry", "2", false);
        assert!(!form.is_complete(), "two Advanced rows clash too");
    }
}
