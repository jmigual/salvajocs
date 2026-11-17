//! The repository backend picker shared by first run and Settings (ADR-0009): a kind, a
//! location and a key/value option list where any option can be marked secret.

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui::{self, Ui};
use salvajocs_core::config::{self, AppConfig, BackendKind, RepoOption};

use super::labelled_edit;
use crate::job::Job;
use crate::theme;

/// Where the options of each backend are documented.
const OPTIONS_DOCS: &str = "https://docs.rs/rustic_backend/0.7.0/rustic_backend/";

/// One row of the option list. No `Debug`: the value may be a secret.
#[derive(Default)]
struct OptionRow {
    key: String,
    value: String,
    secret: bool,
}

/// The state of the backend picker.
pub struct BackendForm {
    kind: BackendKind,
    location: String,
    options: Vec<OptionRow>,
    picker: Option<Job<Option<PathBuf>>>,
}

impl Default for BackendForm {
    fn default() -> Self {
        Self {
            kind: BackendKind::Local,
            location: String::new(),
            options: Vec::new(),
            picker: None,
        }
    }
}

/// The picker entry for a kind.
fn kind_label(kind: BackendKind) -> String {
    match kind {
        BackendKind::Local => "Local folder".to_string(),
        BackendKind::Rest => "REST server".to_string(),
        BackendKind::Rclone => "rclone remote".to_string(),
        BackendKind::OpenDal(service) => format!("OpenDAL: {service}"),
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
        let options = config
            .repository_options
            .iter()
            .map(|option| OptionRow {
                key: option.key.clone(),
                value: if option.secret {
                    secrets.get(&option.key).cloned().unwrap_or_default()
                } else {
                    option.value.clone().unwrap_or_default()
                },
                secret: option.secret,
            })
            .collect();
        Self {
            kind,
            location: location.to_string(),
            options,
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

    /// Whether the location is filled in; an opendal service is located by its options.
    pub fn has_location(&self) -> bool {
        matches!(self.kind, BackendKind::OpenDal(_)) || !self.location.trim().is_empty()
    }

    /// The options as the config keeps them: a secret option has no value.
    pub fn options(&self) -> Vec<RepoOption> {
        self.options
            .iter()
            .filter(|row| !row.key.trim().is_empty())
            .map(|row| RepoOption {
                key: row.key.trim().to_string(),
                value: (!row.secret).then(|| row.value.clone()),
                secret: row.secret,
            })
            .collect()
    }

    /// The values of the secret options, by key.
    pub fn secrets(&self) -> BTreeMap<String, String> {
        self.options
            .iter()
            .filter(|row| row.secret && !row.key.trim().is_empty())
            .map(|row| (row.key.trim().to_string(), row.value.clone()))
            .collect()
    }

    /// Draws the picker as rows of a two-column grid.
    pub fn ui(&mut self, ui: &mut Ui) {
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
            ui.label("");
            ui.weak(match self.kind {
                BackendKind::Local => "A folder on this machine or a mounted drive.",
                BackendKind::Rest => "The URL of a rustic or restic REST server.",
                _ => "remote:path. Needs the rclone binary on the PATH.",
            });
            ui.end_row();
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
            ui.horizontal(|ui| {
                ui.weak("Set the service's options below, such as bucket or root.");
                ui.hyperlink_to("Option names", OPTIONS_DOCS);
            });
            ui.end_row();
            ui.label("");
            ui.weak("opendal:sftp is not available in Windows builds.");
            ui.end_row();
        }
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
}
