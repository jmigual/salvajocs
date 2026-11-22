//! The startup prompt for secrets the keyring does not hold (ADR-0009).

use eframe::egui::{self, Ui};
use salvajocs_core::config;
use salvajocs_core::secret::SecretKey;

use crate::startup::Credentials;
use crate::theme::{self, icons};
use crate::widgets::labelled_edit;

/// What the prompt calls a secret: a secret option of a REST server or an opendal service by the
/// label of its field, else by its key.
fn secret_label(repository: &str, key: &SecretKey) -> String {
    match key {
        SecretKey::Password => "Encryption password".to_string(),
        SecretKey::Option(key) => {
            let fields = if repository.starts_with("rest:") {
                config::rest_fields()
            } else {
                repository
                    .strip_prefix("opendal:")
                    .map_or(&[][..], config::opendal_fields)
            };
            fields
                .iter()
                .find(|f| f.key == *key)
                .map_or_else(|| format!("Option {key}"), |field| field.label.to_string())
        }
    }
}

/// Asks for exactly the secrets that are missing.
pub struct SecretPrompt {
    found: Credentials,
    answers: Vec<(SecretKey, String)>,
    /// The repository string, to name a secret option by the label of its field.
    repository: String,
    error: Option<&'static str>,
}

impl SecretPrompt {
    /// A prompt for `missing`, on top of the secrets already `found`, for the `repository`
    /// string of the config.
    pub fn new(found: Credentials, missing: Vec<SecretKey>, repository: &str) -> Self {
        Self {
            found,
            answers: missing
                .into_iter()
                .map(|key| (key, String::new()))
                .collect(),
            repository: repository.to_string(),
            error: None,
        }
    }

    /// A prompt for the password alone, after the one that was tried did not open the repository.
    pub fn wrong_password(found: Credentials) -> Self {
        Self {
            error: Some("Wrong encryption password."),
            ..Self::new(found, vec![SecretKey::Password], "")
        }
    }

    /// Whether the password is one of the secrets asked for.
    pub fn asks_password(&self) -> bool {
        self.answers
            .iter()
            .any(|(key, _)| *key == SecretKey::Password)
    }

    /// Draws one field per missing secret. Returns all credentials once the user confirms.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<Credentials> {
        if let Some(error) = self.error {
            let color = ui.visuals().error_fg_color;
            theme::status(ui, icons::X_CIRCLE, color, error);
        }
        egui::Grid::new("secret_prompt")
            .num_columns(2)
            .show(ui, |ui| {
                for (key, answer) in &mut self.answers {
                    labelled_edit(ui, &secret_label(&self.repository, key), answer, true);
                    ui.end_row();
                }
            });
        let open = theme::row_end(ui, |ui| theme::primary(ui, true, None, "Open repository"));
        if !open.clicked() {
            return None;
        }
        let mut credentials = self.found.clone();
        credentials.answer(std::mem::take(&mut self.answers));
        Some(credentials)
    }
}
