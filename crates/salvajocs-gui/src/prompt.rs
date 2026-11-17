//! The startup prompt for secrets the keyring does not hold (ADR-0009).

use eframe::egui::{self, Ui};
use salvajocs_core::secret::SecretKey;

use crate::startup::Credentials;
use crate::widgets::labelled_edit;

/// Asks for exactly the secrets that are missing.
pub struct SecretPrompt {
    found: Credentials,
    answers: Vec<(SecretKey, String)>,
    error: Option<&'static str>,
}

impl SecretPrompt {
    /// A prompt for `missing`, on top of the secrets already `found`.
    pub fn new(found: Credentials, missing: Vec<SecretKey>) -> Self {
        Self {
            found,
            answers: missing
                .into_iter()
                .map(|key| (key, String::new()))
                .collect(),
            error: None,
        }
    }

    /// A prompt for the password alone, after the one that was tried did not open the repository.
    pub fn wrong_password(found: Credentials) -> Self {
        Self {
            error: Some("Wrong repository password."),
            ..Self::new(found, vec![SecretKey::Password])
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
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        egui::Grid::new("secret_prompt")
            .num_columns(2)
            .show(ui, |ui| {
                for (key, answer) in &mut self.answers {
                    let label = match key {
                        SecretKey::Password => "Repository password".to_string(),
                        SecretKey::Option(key) => format!("Option {key}"),
                    };
                    labelled_edit(ui, &label, answer, true);
                    ui.end_row();
                }
            });
        if !ui.button("Open repository").clicked() {
            return None;
        }
        let mut credentials = self.found.clone();
        credentials.answer(std::mem::take(&mut self.answers));
        Some(credentials)
    }
}
