//! The "Old backups" section of the Settings tab: the repository's retention policy (ADR-0015).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui::{self, DragValue, Response, Ui};
use salvajocs_core::retention::{self, Retention, RetentionError, Unit, Within};

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::theme::{self, icons};

/// The units offered for the age rule, in order.
const UNITS: [Unit; 4] = [Unit::Days, Unit::Weeks, Unit::Months, Unit::Years];

fn unit_label(unit: Unit) -> &'static str {
    match unit {
        Unit::Days => "days",
        Unit::Weeks => "weeks",
        Unit::Months => "months",
        Unit::Years => "years",
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
        Retention {
            keep_last: self.keep_last.then(|| at_least_one(self.last)),
            keep_within: self.keep_within.then(|| Within {
                amount: at_least_one(self.amount),
                unit: self.unit,
            }),
        }
    }
}

/// Saves shared with the job that runs them. The job saves `next` until there is none, so the
/// latest edit reaches the repository even when the tab is no longer drawn.
#[derive(Default)]
struct Saves {
    /// The latest edited policy, not yet saved.
    next: Option<Retention>,
    /// Whether a job is saving; set and cleared under the lock, so no edit is left behind.
    running: bool,
    /// Why the last save failed.
    error: Option<String>,
    /// Problems to show in the banner.
    warnings: Vec<String>,
}

/// Why the policy could not be loaded.
struct LoadError {
    message: String,
    /// The repository could not be reached, so loading again may work. The other case, a policy
    /// that cannot be read, stays until another version replaces it.
    retry: bool,
}

/// The state of the "Old backups" section.
#[derive(Default)]
pub(super) struct RetentionSection {
    /// The policy as loaded and edited since; `None` until loaded.
    form: Option<Result<Form, LoadError>>,
    loading: Option<Job<Retention>>,
    saves: Arc<Mutex<Saves>>,
    /// The saves of repositories the app used before, until their jobs end.
    previous: Vec<Arc<Mutex<Saves>>>,
    /// The pass the section was last drawn in, to tell when the tab is shown again.
    drawn: u64,
}

impl RetentionSection {
    /// Whether a save runs, also one for a repository the app no longer uses.
    pub(super) fn busy(&self) -> bool {
        lock(&self.saves).running || self.previous.iter().any(|saves| lock(saves).running)
    }

    /// Forgets the policy, so the section loads it from the repository that is now open. Saves
    /// still running go on, for the repository they started on.
    pub(super) fn reset(&mut self) {
        let saves = std::mem::take(&mut self.saves);
        if lock(&saves).running {
            self.previous.push(saves);
        }
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
        } else if self.loading.is_none() {
            let idle = {
                let saves = lock(&self.saves);
                !saves.running && saves.next.is_none()
            };
            // Another machine may have changed the policy while the tab was hidden.
            if self.form.is_none() || (shown_again && idle) {
                self.load(ui.ctx(), state);
            }
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
                self.save(ui.ctx(), state, form.policy());
            }
        }
        if matches!(self.form, Some(Ok(_))) && form.policy().is_off() {
            ui.label("All backups are kept.");
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
        if let Some(error) = &lock(&self.saves).error {
            theme::status(ui, icons::X_CIRCLE, color, error);
        }
    }

    /// Collects a finished load and the saves' warnings, and drops the previous repositories'
    /// saves once they end, reporting their failures in the banner.
    fn poll(&mut self, state: &mut State) {
        if let Some(job) = &mut self.loading
            && let Some(result) = job.take_result()
        {
            self.loading = None;
            self.form = Some(result.map(Form::new).map_err(|err| {
                match err.downcast_ref::<RetentionError>() {
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
                }
            }));
        }
        state.notices.append(&mut lock(&self.saves).warnings);
        self.previous.retain(|saves| {
            let mut saves = lock(saves);
            state.notices.append(&mut saves.warnings);
            state.notices.extend(saves.error.take());
            saves.running
        });
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

    /// Saves `policy` in the background. A save already running saves it next, so the
    /// repository ends up with the latest policy.
    fn save(&self, ctx: &egui::Context, state: &State, policy: Retention) {
        let Some(session) = &state.session else {
            return;
        };
        let mut saves = lock(&self.saves);
        saves.next = Some(policy);
        if saves.running {
            return;
        }
        saves.running = true;
        let (storage, shared) = (session.storage.clone(), self.saves.clone());
        let stage_root = state.env.cache_dir.join("stage");
        // The job ends by itself and reports through `saves`, so its handle is not kept.
        drop(Job::spawn(ctx, move || {
            save_queued(&storage, &shared, &stage_root);
            Ok(())
        }));
    }
}

fn lock(saves: &Mutex<Saves>) -> MutexGuard<'_, Saves> {
    // Nothing panics while holding the lock, so its data is consistent.
    saves.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears [`Saves::running`] when a save job panics, so later edits start a new one.
struct PanicGuard<'a>(&'a Mutex<Saves>);

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        // Only on a panic: a job that ended normally cleared it under the lock, and a new job
        // may have set it since.
        if std::thread::panicking() {
            let mut saves = lock(self.0);
            saves.running = false;
            saves.error =
                Some("Could not save the cleanup rules: the save stopped unexpectedly.".to_owned());
        }
    }
}

/// Saves the queued policy until none is left, recording each outcome in `saves`.
fn save_queued(storage: &SharedStorage, saves: &Mutex<Saves>, stage_root: &Path) {
    let _guard = PanicGuard(saves);
    loop {
        let policy = {
            let mut saves = lock(saves);
            let Some(policy) = saves.next.take() else {
                saves.running = false;
                return;
            };
            policy
        };
        let result = lock_storage(storage)
            .and_then(|mut storage| Ok(retention::save(&mut storage, &policy, stage_root)?));
        let mut saves = lock(saves);
        match result {
            Ok(saved) => {
                saves.error = None;
                if let Some(err) = saved.forget_error {
                    saves.warnings.push(format!(
                        "The cleanup rules were saved, but the older copies could not be removed: {err}"
                    ));
                }
            }
            Err(err) => saves.error = Some(format!("Could not save the cleanup rules: {err:#}")),
        }
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
