//! Moving the password out of a `rest:` URL that has one (ADR-0009).

use super::{AppConfig, RepoOption};
use crate::storage::{REST_PASSWORD_KEY, join_rest_password, split_rest_password};

impl AppConfig {
    /// Moves the password out of a `rest:` URL into the secret option [`REST_PASSWORD_KEY`], whose
    /// value is held in memory until [`AppConfig::save`] drops it. Returns the keyring scope the
    /// config had before, which held the password in its repository string, or `None` when the
    /// repository has no password in its URL. The marker of the custom games' repository and the
    /// scope of the set-aside ones follow the new scope.
    pub fn take_rest_password(&mut self) -> Option<String> {
        let (repository, password) = split_rest_password(&self.repository)?;
        let old_scope = self.secret_scope();
        self.repository = repository;
        self.repository_options
            .retain(|option| option.key != REST_PASSWORD_KEY);
        self.repository_options.push(RepoOption::new(
            REST_PASSWORD_KEY.to_string(),
            Some(password),
            true,
        ));
        self.rescope_custom_games(&old_scope);
        Some(old_scope)
    }

    /// Undoes [`AppConfig::take_rest_password`] for a password that could not be stored in the
    /// keyring: puts the option's in-memory value back into the URL, so a save keeps it instead
    /// of losing it. Does nothing when the option has no value.
    pub fn put_back_rest_password(&mut self) {
        let Some(password) = self
            .repository_options
            .iter()
            .find(|option| option.key == REST_PASSWORD_KEY)
            .and_then(|option| option.value.clone())
        else {
            return;
        };
        let scope = self.secret_scope();
        self.repository_options
            .retain(|option| option.key != REST_PASSWORD_KEY);
        self.repository = join_rest_password(&self.repository, &password);
        self.rescope_custom_games(&scope);
    }

    /// Moves the custom games' marker and set-aside scope that are `old_scope` to the current
    /// scope.
    fn rescope_custom_games(&mut self, old_scope: &str) {
        let scope = self.secret_scope();
        if self.custom_games_repository.as_deref() == Some(old_scope) {
            self.custom_games_repository = Some(scope.clone());
        }
        if let Some(not_now) = &mut self.custom_games_not_now
            && not_now.repository == old_scope
        {
            not_now.repository = scope;
        }
    }
}
