//! The "Old backups" section of the Settings tab: the repository's retention policy (ADR-0015),
//! changed here until a Back up saves it (ADR-0029).

use std::num::NonZeroU32;

use eframe::egui::{self, DragValue, Response, Ui};
use salvajocs_core::retention::{self, Retention, RetentionError, Unit, Within};

use crate::job::Job;
use crate::state::{State, lock_storage};
use crate::theme::{self, icons};

/// The units offered for the age rule, in order.
const UNITS: [Unit; 4] = [Unit::Days, Unit::Weeks, Unit::Months, Unit::Years];

fn unit_label(unit: Unit) -> &'static str {
    match unit {
        Unit::Days => "days",
        Unit::Weeks => "weeks",
        Unit::Months => "months",
        Unit::Years => "years",
        _ => "units",
    }
}

/// The controls' values. A rule that is off keeps its number, so ticking it again restores it.
#[derive(Clone, Copy)]
struct Form {
    keep_last: bool,
    last: u32,
    keep_within: bool,
    amount: u32,
    unit: Unit,
}

impl Form {
    fn new(policy: Retention) -> Self {
        Self {
            keep_last: policy.keep_last.is_some(),
            last: policy.keep_last.map_or(10, NonZeroU32::get),
            keep_within: policy.keep_within.is_some(),
            amount: policy.keep_within.map_or(3, |within| within.amount.get()),
            unit: policy
                .keep_within
                .map_or(Unit::Months, |within| within.unit),
        }
    }

    fn policy(&self) -> Retention {
        let at_least_one = |n: u32| NonZeroU32::new(n).unwrap_or(NonZeroU32::MIN);
        let mut policy = Retention::default();
        policy.keep_last = self.keep_last.then(|| at_least_one(self.last));
        policy.keep_within = self.keep_within.then(|| Within {
            amount: at_least_one(self.amount),
            unit: self.unit,
        });
        policy
    }
}

/// Why the policy could not be loaded.
struct LoadError {
    message: String,
    /// The repository could not be reached, so loading again may work. The other case, a policy
    /// that cannot be read, stays until another version replaces it.
    retry: bool,
}

/// The state of the "Old backups" section. An edit stays on this PC until the next Back up
/// saves it (ADR-0029).
#[derive(Default)]
pub(super) struct RetentionSection {
    /// The policy shown and edited since: this PC's unsent one, else the loaded one; `None`
    /// until loaded.
    form: Option<Result<Form, LoadError>>,
    /// The repository's policy as last loaded, the base of a change that starts now.
    loaded: Retention,
    loading: Option<Job<Retention>>,
    /// The pass the section was last drawn in, to tell when the tab is shown again.
    drawn: u64,
    /// A Back up checks, asks, waits to start or runs, so it may be sending this PC's rules.
    pub(super) backing_up: bool,
}

impl RetentionSection {
    /// Forgets the policy, so the section loads it from the repository that is now open.
    pub(super) fn reset(&mut self) {
        self.form = None;
        self.loading = None;
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, state: &mut State) {
        ui.heading("Old backups");
        theme::group(ui, |ui| self.group_ui(ui, state));
    }

    fn group_ui(&mut self, ui: &mut Ui, state: &mut State) {
        self.poll(state);
        let pass = ui.ctx().cumulative_pass_nr();
        let shown_again = pass > self.drawn + 1;
        self.drawn = pass;
        if state.session.is_none() {
            ui.label("The repository is not open.");
        } else if self.loading.is_none() && (self.form.is_none() || shown_again) {
            // Another machine may have changed the policy while the tab was hidden.
            self.load(ui.ctx(), state);
        }
        let editable = matches!(self.form, Some(Ok(_))) && self.loading.is_none();
        let mut form = match &self.form {
            Some(Ok(form)) => *form,
            _ => Form::new(Retention::default()),
        };
        let commit = ui
            .add_enabled_ui(editable, |ui| controls(ui, &mut form))
            .inner;
        if editable {
            self.form = Some(Ok(form));
            if commit {
                self.change(ui.ctx(), state, form.policy());
            }
        }
        if matches!(self.form, Some(Ok(_))) && form.policy().is_off() {
            ui.label("All backups are kept.");
        }
        if state
            .config
            .as_ref()
            .is_some_and(|c| c.retention_unsent.is_some())
        {
            ui.horizontal(|ui| {
                ui.weak("Saved to the repository with the next Back up.");
                // Also the way out when the repository's rules cannot be read or overwritten.
                let discard = egui::Button::new("Discard this PC's changes");
                // A discard during a Back up could clear rules it is sending.
                if ui.add_enabled(!self.backing_up, discard).clicked() {
                    self.discard(ui.ctx(), state);
                }
            });
        }
        ui.weak(
            "A backup is only made when a game's files changed. A backup is kept if any rule \
             keeps it, and the newest backup of each game, and of each computer, is always kept. \
             Old backups are removed after the next backup; their disk space comes back at a \
             later cleanup, at least a day later.",
        );
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading the rules from the repository");
            });
        }
        let color = ui.visuals().error_fg_color;
        if let Some(Err(error)) = &self.form {
            theme::status(ui, icons::X_CIRCLE, color, &error.message);
            if error.retry && ui.button("Retry").clicked() {
                self.form = None;
            }
        }
    }

    /// Collects a finished load: the form shows this PC's unsent policy over the loaded one.
    fn poll(&mut self, state: &State) {
        if let Some(job) = &mut self.loading
            && let Some(result) = job.take_result()
        {
            self.loading = None;
            let unsent = state.config.as_ref().and_then(|c| c.retention_unsent);
            self.form = Some(match result {
                Ok(policy) => {
                    self.loaded = policy;
                    Ok(Form::new(unsent.map_or(policy, |unsent| unsent.policy)))
                }
                Err(err) => Err(match err.downcast_ref::<RetentionError>() {
                    Some(RetentionError::NewerFormat(_) | RetentionError::Parse(_)) => LoadError {
                        message: format!(
                            "This repository's cleanup rules could not be read; nothing is removed ({err:#})"
                        ),
                        retry: false,
                    },
                    _ => LoadError {
                        message: format!("Could not load the cleanup rules: {err:#}"),
                        retry: true,
                    },
                }),
            });
        }
    }

    fn load(&mut self, ctx: &egui::Context, state: &State) {
        let Some(session) = &state.session else {
            return;
        };
        let storage = session.storage.clone();
        self.loading = Some(Job::spawn(ctx, move || {
            Ok(retention::load(&mut *lock_storage(&storage)?)?)
        }));
    }

    /// Drops this PC's unsent rules and shows the repository's again. Writes only the config.
    fn discard(&mut self, ctx: &egui::Context, state: &mut State) {
        if let Some(config) = &mut state.config {
            config.retention_unsent = None;
            state.save_config(ctx);
        }
        if let Some(Ok(_)) = self.form {
            self.form = Some(Ok(Form::new(self.loaded)));
        }
    }

    /// Keeps `policy` as this PC's unsent rules, based on the loaded ones unless a change is
    /// already unsent: the earliest base wins. Writes only the config.
    fn change(&self, ctx: &egui::Context, state: &mut State, policy: Retention) {
        let Some(config) = &mut state.config else {
            return;
        };
        let based_on = (config.retention_unsent).map_or(self.loaded, |unsent| unsent.based_on);
        config.retention_unsent = Some(retention::Unsent::new(policy, based_on));
        state.save_config(ctx);
    }
}

/// Draws the two rules. Returns whether the user finished a change, so it is to be saved.
fn controls(ui: &mut Ui, form: &mut Form) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let checkbox = ui.checkbox(&mut form.keep_last, "Keep the last");
        let number = number(ui, &mut form.last).labelled_by(checkbox.id);
        ui.label("backups of each game");
        changed |= checkbox.changed() || finished(&number);
    });
    ui.horizontal(|ui| {
        let checkbox = ui.checkbox(&mut form.keep_within, "Keep every backup newer than");
        let number = number(ui, &mut form.amount).labelled_by(checkbox.id);
        let unit = egui::ComboBox::from_id_salt("retention_unit")
            .selected_text(unit_label(form.unit))
            .show_ui(ui, |ui| {
                theme::pill_list(ui);
                UNITS
                    .into_iter()
                    .map(|unit| ui.selectable_value(&mut form.unit, unit, unit_label(unit)))
                    .fold(false, |changed, response| changed | response.changed())
            });
        unit.response.labelled_by(checkbox.id);
        changed |= checkbox.changed() || finished(&number) || unit.inner.unwrap_or(false);
    });
    changed
}

/// A number from 1 to 9999 that takes typed digits only on Enter or leaving the field, so
/// typing 15 never saves 1 on the way.
fn number(ui: &mut Ui, value: &mut u32) -> Response {
    ui.add(
        DragValue::new(value)
            .range(1..=9999)
            .update_while_editing(false),
    )
}

/// Whether a number edit is finished: a drag only when it is let go, so each step of it is not
/// saved.
fn finished(number: &Response) -> bool {
    (number.changed() && !number.dragged()) || number.drag_stopped()
}

#[cfg(test)]
mod tests;
