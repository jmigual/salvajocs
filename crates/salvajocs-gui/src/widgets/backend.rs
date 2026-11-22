//! The repository backend picker shared by first run and Settings (ADR-0009): a kind, a
//! location or the named fields of the service, and an Advanced key/value option list where any
//! option can be marked secret.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

use eframe::egui::{self, TextEdit, Ui};
use salvajocs_core::config::{self, AppConfig, BackendField, BackendKind, RepoOption};
use salvajocs_core::storage::{REST_PASSWORD_KEY, split_rest_password};

use super::{field_hint, field_status, guide_url, labelled_edit};
use crate::env::Env;
use crate::job::Job;
use crate::theme::{self, icons};

/// The advanced options reference, at the tag of the running version so the page matches the keys
/// this build reads.
const OPTIONS_DOCS: &str = guide_url!("advanced-options.md");

/// One row of the Advanced option list. No `Debug`: the value may be a secret.
#[derive(Default)]
struct OptionRow {
    key: String,
    value: String,
    secret: bool,
    /// Whether the backend does not read the key. Kept here because working it out is too slow
    /// for every frame; it is redone when the key or the backend changes.
    unknown: bool,
    /// Whether the secret flag still follows the key: set on a row the user added, cleared when
    /// they click the checkbox. A loaded row never follows, as flipping its flag would change the
    /// keyring scope.
    follows_key: bool,
}

impl OptionRow {
    /// A row the user just added, which ticks "secret" for a key that looks like one.
    fn added() -> Self {
        Self {
            follows_key: true,
            ..Self::default()
        }
    }

    /// Redoes what depends on the key, after it or the backend changed.
    fn key_changed(&mut self, kind: BackendKind) {
        self.unknown = is_unknown_key(kind, &self.key);
        if self.follows_key {
            self.secret = config::looks_secret(&self.key);
        }
    }
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
    /// The secret named values of the services the user switched away from, by service, so they
    /// come back with that service and are never shown in another.
    stashed: BTreeMap<&'static str, BTreeMap<String, String>>,
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
            stashed: BTreeMap::new(),
            remotes: Remotes::Unasked,
            picker: None,
        }
    }
}

/// The named fields of a kind; only REST and the opendal services have them.
fn fields_of(kind: BackendKind) -> &'static [BackendField] {
    match kind {
        BackendKind::OpenDal(service) => config::opendal_fields(service),
        BackendKind::Rest => config::rest_fields(),
        _ => &[],
    }
}

/// The warning under an Advanced key the backend does not read.
const UNKNOWN_KEY: &str = "Not an option of this backend, so it probably does nothing. Check the spelling in the option reference.";

/// The warning for an rclone remote served without a login.
const RCLONE_NO_LOGIN: &str = "use-password is false, so the local rclone server has no login. Any program on this PC can read and delete the repository while salvajocs runs.";

/// Whether an Advanced key is typed but is not one the backend reads. Allowed, but a typo would
/// otherwise be ignored without a word.
fn is_unknown_key(kind: BackendKind, key: &str) -> bool {
    let key = key.trim();
    !key.is_empty() && !config::option_key_known(kind, key)
}

/// The section of the options reference for a kind. Its heading is the picker entry, and GitHub
/// makes the anchor from the heading: lowercase, spaces to hyphens, punctuation other than `-`
/// and `_` dropped.
fn options_docs_url(kind: BackendKind) -> String {
    let anchor: String = kind_label(kind)
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect();
    format!("{OPTIONS_DOCS}#{anchor}")
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
            // and with it the keyring scope, is never flipped. Anything else stays Advanced,
            // including an empty value, which a field would not save.
            let in_field = fields
                .iter()
                .any(|field| field.key == option.key && field.secret == option.secret)
                && !value.trim().is_empty()
                && !named.contains_key(&option.key);
            if in_field {
                named.insert(option.key.clone(), value);
            } else {
                options.push(OptionRow {
                    key: option.key.clone(),
                    value,
                    secret: option.secret,
                    unknown: is_unknown_key(kind, &option.key),
                    follows_key: false,
                });
            }
        }
        let mut form = Self {
            kind,
            location: location.to_string(),
            named,
            advanced_open: !options.is_empty(),
            stashed: BTreeMap::new(),
            options,
            remotes: Remotes::Unasked,
            picker: None,
        };
        form.take_pasted_password();
        form
    }

    /// Moves a password in the REST URL into the Password field, so it is never part of the
    /// repository string, the config or the keyring scope. A password in the URL wins over one
    /// already in the field.
    fn take_pasted_password(&mut self) {
        if self.kind != BackendKind::Rest {
            return;
        }
        if let Some((url, password)) =
            split_rest_password(&format!("rest:{}", self.location.trim()))
            && let Some(location) = url.strip_prefix("rest:")
        {
            self.location = location.to_string();
            self.named.insert(REST_PASSWORD_KEY.to_string(), password);
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

    /// The key of the field that `key` stands for: itself, or the key opendal reads an alias as.
    /// opendal reads keys in any letter case; rustic's own keys are exact.
    fn canonical_key(&self, key: &str) -> String {
        if !matches!(self.kind, BackendKind::OpenDal(_)) {
            return key.to_string();
        }
        let lower = key.to_ascii_lowercase();
        // `Retry` is not rustic's `retry`, and opendal ignores both, so neither clashes.
        if config::is_rustic_key(self.kind, &lower) {
            return key.to_string();
        }
        fields_of(self.kind)
            .iter()
            .find(|field| field.key == lower || field.aliases.contains(&lower.as_str()))
            .map_or(lower, |field| field.key.to_string())
    }

    /// A key that would be saved twice, which `location()` would resolve by dropping one and
    /// opendal rejects when an alias and its key are both set.
    fn duplicate_key(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        self.entries()
            .into_iter()
            .map(|entry| self.canonical_key(entry.key))
            .find(|key| !seen.insert(key.clone()))
    }

    /// The error shown for a duplicate key, saying where the two are.
    fn duplicate_message(&self) -> Option<String> {
        let key = self.duplicate_key()?;
        let in_field = fields_of(self.kind).iter().any(|field| field.key == key)
            && self
                .named
                .get(&key)
                .is_some_and(|value| !value.trim().is_empty());
        Some(if in_field {
            format!("\"{key}\" is set in both a field and Advanced options. Remove one.")
        } else {
            format!("\"{key}\" is set twice in Advanced options. Remove one.")
        })
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
                        entries.iter().any(|entry| {
                            self.canonical_key(entry.key) == field.key
                                && !entry.value.trim().is_empty()
                        })
                    })
            }
            _ => !self.location.trim().is_empty(),
        }
    }

    /// Changes the kind. The secrets of the old kind are set aside, so a server password never
    /// reaches another service, and come back when the user returns to it.
    fn switch_kind(&mut self, kind: BackendKind) {
        if let BackendKind::OpenDal(service) = self.kind {
            let secrets = fields_of(self.kind)
                .iter()
                .filter(|field| field.secret)
                .filter_map(|field| Some((field.key.to_string(), self.named.remove(field.key)?)))
                .collect();
            self.stashed.insert(service, secrets);
        }
        self.kind = kind;
        for row in &mut self.options {
            row.unknown = is_unknown_key(kind, &row.key);
        }
        if let BackendKind::OpenDal(service) = kind
            && let Some(secrets) = self.stashed.remove(service)
        {
            self.named.extend(secrets);
        }
    }

    /// Whether the rclone server salvajocs starts would run without a login: `use-password` is
    /// `false`, which is the only value rustic reads as off.
    fn rclone_without_login(&self) -> bool {
        self.kind == BackendKind::Rclone
            && self
                .entries()
                .iter()
                .any(|entry| entry.key == "use-password" && entry.value == "false")
    }

    /// The options as the config keeps them: a secret option has no value.
    pub fn options(&self) -> Vec<RepoOption> {
        self.entries()
            .into_iter()
            .map(|entry| {
                RepoOption::new(
                    entry.key.to_string(),
                    (!entry.secret).then(|| entry.value.to_string()),
                    entry.secret,
                )
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
        self.take_pasted_password();
        if let Some(job) = &mut self.picker
            && let Some(result) = job.take_result()
        {
            self.picker = None;
            if let Ok(Some(folder)) = result {
                self.location = folder.to_string_lossy().into_owned();
            }
        }
        let label = ui.label("Backend");
        let mut chosen = None;
        egui::ComboBox::from_id_salt("backend_kind")
            .selected_text(kind_label(self.kind))
            .show_ui(ui, |ui| {
                theme::pill_list(ui);
                for kind in config::backend_kinds() {
                    let selected = self.kind == kind;
                    if ui.selectable_label(selected, kind_label(kind)).clicked() && !selected {
                        chosen = Some(kind);
                    }
                }
            })
            .response
            .labelled_by(label.id);
        ui.end_row();
        if let Some(kind) = chosen {
            self.switch_kind(kind);
        }

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
                    BackendKind::Rest => {
                        "The URL of a rustic or restic REST server, with the user name if it has a login: https://user@host/. The password goes in the field below."
                    }
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
                let pick = env.pick_folder.clone();
                self.picker = Some(Job::spawn(ui.ctx(), move || Ok(pick())));
            }
            ui.end_row();
        } else {
            if let BackendKind::OpenDal(service) = self.kind {
                ui.label("");
                field_hint(ui, config::opendal_hint(service).unwrap_or_default());
                ui.end_row();
            }
        }
        for field in fields_of(self.kind) {
            self.field_ui(ui, field);
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
        let Some(message) = self.duplicate_message() else {
            return;
        };
        ui.label("");
        let color = ui.visuals().error_fg_color;
        field_status(ui, icons::X_CIRCLE, color, message);
        ui.end_row();
    }

    /// The toggle row and, when open, the option reference and the option list. A toggle row
    /// because a `CollapsingHeader` cannot span the rows of a grid.
    fn advanced_ui(&mut self, ui: &mut Ui) {
        if self.rclone_without_login() {
            ui.label("");
            let color = ui.visuals().warn_fg_color;
            field_status(ui, icons::WARNING, color, RCLONE_NO_LOGIN);
            ui.end_row();
        }
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
        ui.label("");
        ui.hyperlink_to("Option reference", options_docs_url(self.kind));
        ui.end_row();
        self.options_ui(ui);
    }

    fn options_ui(&mut self, ui: &mut Ui) {
        let kind = self.kind;
        let mut remove = None;
        for (index, row) in self.options.iter_mut().enumerate() {
            let number = index + 1;
            let key_edit = labelled_edit(ui, &format!("Option {number} key"), &mut row.key, false);
            if key_edit.changed() {
                row.key_changed(kind);
            }
            if ui.button("Remove").clicked() {
                remove = Some(index);
            }
            ui.end_row();
            if row.unknown {
                ui.label("");
                let color = ui.visuals().warn_fg_color;
                field_status(ui, icons::WARNING, color, UNKNOWN_KEY);
                ui.end_row();
            }
            labelled_edit(
                ui,
                &format!("Option {number} value"),
                &mut row.value,
                row.secret,
            );
            if ui.checkbox(&mut row.secret, "secret").clicked() {
                row.follows_key = false;
            }
            ui.end_row();
        }
        if let Some(index) = remove {
            self.options.remove(index);
        }
        ui.label("");
        if ui.button("Add option").clicked() {
            self.options.push(OptionRow::added());
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
            unknown: is_unknown_key(self.kind, key),
            follows_key: false,
        });
    }

    /// Sets a named field, for tests that cannot type.
    #[cfg(test)]
    fn set_named(&mut self, key: &str, value: &str) {
        self.named.insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests;
