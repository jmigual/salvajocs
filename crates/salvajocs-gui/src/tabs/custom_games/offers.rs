//! The Custom games tab's games from other PCs: the repository's custom games that are new here
//! or differ from this PC's, which this PC uses only once the user accepts them (ADR-0030).

use eframe::egui::{self, Ui};
use salvajocs_core::config::AppConfig;
use salvajocs_core::manifest::CustomGame;

use crate::state::CustomGameJobs;
use crate::tabs::backup::unsent::{conflict_lines, game_lines, versions_ui};
use crate::theme;
use crate::widgets::named_button;

/// What the user answered to the offer of a game, with the copy drawn, so a read that replaced
/// the offer meanwhile does not apply the answer to a copy the user never saw.
pub(super) enum Answer {
    /// **Use on this PC**.
    Accept(CustomGame),
    /// **Not now**.
    NotNow(CustomGame),
}

/// The offers of the last read whose name has no unsent change, each with its rows (beside this
/// PC's for a changed game): those waiting, with **Use on this PC** and **Not now**, then those
/// set aside, collapsed, with **Use on this PC**; nothing when there are none.
pub(super) fn offers_ui(ui: &mut Ui, config: &AppConfig, jobs: &CustomGameJobs) -> Option<Answer> {
    let (aside, waiting): (Vec<_>, Vec<_>) =
        jobs.listed(config).partition(|game| config.set_aside(game));
    if aside.is_empty() && waiting.is_empty() {
        return None;
    }
    ui.add_space(16.0);
    ui.heading("From other PCs, not used here");
    ui.weak(
        "A custom game decides which files and registry keys a restore may write. Use one only \
         if you recognise it.",
    );
    let mut answer = None;
    for offered in waiting {
        offer_ui(ui, config, offered, true, &mut answer);
    }
    if !aside.is_empty() {
        egui::CollapsingHeader::new(format!("Set aside ({})", aside.len()))
            .id_salt("set aside custom games")
            .show(ui, |ui| {
                for offered in aside {
                    offer_ui(ui, config, offered, false, &mut answer);
                }
            });
    }
    answer
}

/// The offer `offered`, with **Not now** when it `waits`; a click puts its answer in `answer`.
fn offer_ui(
    ui: &mut Ui,
    config: &AppConfig,
    offered: &CustomGame,
    waits: bool,
    answer: &mut Option<Answer>,
) {
    let name = &offered.name;
    let local = config.custom_games.iter().find(|game| game.name == *name);
    theme::group(ui, |ui| {
        let status = if local.is_some() {
            "changed on another PC"
        } else {
            "new"
        };
        ui.label(theme::semibold(format!("{name} ({status})")));
        match local {
            Some(local) => {
                let (here, theirs) = conflict_lines(Some(local), Some(offered));
                versions_ui(ui, &here, &theirs);
            }
            None => {
                for line in game_lines(Some(offered)) {
                    ui.label(line);
                }
            }
        }
        ui.horizontal(|ui| {
            let accept = format!("Use {name} on this PC");
            if named_button(ui, true, "Use on this PC", &accept).clicked() {
                *answer = Some(Answer::Accept(offered.clone()));
            }
            if waits {
                let label = format!("Not now for {name}");
                let hover =
                    "Lists it under Set aside, without a notice, until another PC changes it";
                let not_now = named_button(ui, true, "Not now", &label).on_hover_text(hover);
                if not_now.clicked() {
                    *answer = Some(Answer::NotNow(offered.clone()));
                }
            }
        });
    });
}
