//! The Custom games tab's games from other PCs: the repository's custom games that are new here
//! or differ from this PC's, which this PC uses only once the user accepts them (ADR-0030).

use eframe::egui::Ui;
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
    /// **Keep this PC's version**, or **Remove from the repository** for a game new here.
    Decline(CustomGame),
}

/// The offers of the last read whose name has no unsent change, each with its rows (beside this
/// PC's for a changed game) and its two answers; nothing when there are none.
pub(super) fn offers_ui(ui: &mut Ui, config: &AppConfig, jobs: &CustomGameJobs) -> Option<Answer> {
    let unsent = |name: &str| (config.custom_games_unsent.iter()).any(|entry| entry.name == name);
    let offers: Vec<_> = (jobs.all_offered().values())
        .filter(|game| !unsent(&game.name))
        .collect();
    if offers.is_empty() {
        return None;
    }
    ui.add_space(16.0);
    ui.heading("From other PCs, not used here");
    ui.weak(
        "A custom game decides which files and registry keys a restore may write. Use one only \
         if you recognise it.",
    );
    let mut answer = None;
    for offered in offers {
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
                    answer = Some(Answer::Accept(offered.clone()));
                }
                let (text, label, hover) = match local {
                    Some(_) => (
                        "Keep this PC's version",
                        format!("Keep this PC's version of {name}"),
                        "The next Back up saves this PC's version to the repository",
                    ),
                    None => (
                        "Remove from the repository",
                        format!("Remove {name} from the repository"),
                        "The next Back up removes it from the repository, and every PC that uses \
                         it removes it at its next read, without asking",
                    ),
                };
                let decline = named_button(ui, true, text, &label).on_hover_text(hover);
                if decline.clicked() {
                    answer = Some(Answer::Decline(offered.clone()));
                }
            });
        });
    }
    answer
}
