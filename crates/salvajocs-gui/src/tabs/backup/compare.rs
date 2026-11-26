//! The compare window of a game that waits for the user's choice before Back up stores it
//! (ADR-0035): its file tree against the newest backup, and whether to back up this PC's saves.

use std::borrow::Cow;

use eframe::egui::{self, Button};
use salvajocs_core::selection::GameSettings;

use super::BackupTab;
use super::preview::{Header, files_ui, header_of};
use crate::state::State;
use crate::theme;
use crate::widgets::named_button;

/// What the user did in the compare window.
enum Choice {
    /// Back up this PC's saves at the next Back up.
    BackUp,
    /// Do not back them up.
    Skip,
    /// Close the window and change nothing.
    Close,
}

impl BackupTab {
    /// The compare window of the game [`BackupTab::comparing`] names, while the report has it:
    /// why the game waits for a choice, its file tree with the list's ticks, and two buttons.
    /// **Back up this PC's saves** adds the game to [`BackupTab::back_up_anyway`], **Don't back
    /// up** takes it out; both close the window. Once a tick makes the game no longer wait, the
    /// window says what Back up does with it and offers **Close** instead. Escape or a click
    /// outside closes it and changes nothing.
    pub(super) fn compare_ui(&mut self, ctx: &egui::Context, state: &mut State) {
        let Some(title) = &self.comparing else {
            return;
        };
        let report = self.report.as_ref();
        let found = report.and_then(|report| {
            let index = report.games.iter().position(|game| game.title == *title)?;
            Some((index, &report.games[index]))
        });
        let Some((index, game)) = found else {
            self.comparing = None;
            return;
        };
        let (settings, readable) = match self.choices.game(&game.title) {
            Ok(settings) => (settings, true),
            Err(_) => (Cow::Owned(GameSettings::default()), false),
        };
        let editable = self.choices.editable() && self.running.is_none();
        // A consent writes nothing, so it does not wait for a pending write.
        let consentable = self.choices.loaded() && self.running.is_none();
        let preview = self.previews.get(index);
        let listed = &mut self.listed[index];
        let header = (listed.header).get_or_insert_with(|| header_of(game, preview, &settings));
        let tree = &mut listed.tree;
        let mut clicked = (None, None);
        let (choice, dismissed) = theme::modal_dismissible(ctx, "backup_compare", 720.0, |ui| {
            ui.heading(&game.title);
            ui.label(match &*header {
                Header::Compare { line, .. } => line.as_str(),
                Header::Mark { text, .. } | Header::Chips(_, text) => text.as_str(),
                Header::Nothing => "Nothing of this game is ticked, so Back up leaves it out.",
            });
            ui.add_space(8.0);
            let enabled = editable && readable;
            clicked = files_ui(ui, game, tree, preview, &settings, enabled, true);
            ui.add_space(8.0);
            theme::row_end(ui, |ui| {
                if !header.asks() {
                    // Named apart from the Close of the app's title bar.
                    let close = named_button(ui, true, "Close", "Close comparison");
                    return close.clicked().then_some(Choice::Close);
                }
                if ui
                    .add_enabled(consentable, Button::new("Don't back up"))
                    .clicked()
                {
                    return Some(Choice::Skip);
                }
                let back_up = Button::new("Back up this PC's saves");
                (ui.add_enabled(consentable, back_up).clicked()).then_some(Choice::BackUp)
            })
        });
        let (change, action) = clicked;
        if let Some(change) = change {
            self.choices.change(ctx, state, &game.title, change);
            self.listed[index].invalidate();
        }
        if let Some(action) = action {
            self.file_action(ctx, state, index, action);
        }
        let Some(choice) = choice.or(dismissed.then_some(Choice::Close)) else {
            return;
        };
        match choice {
            Choice::BackUp => self.consent(index, true),
            Choice::Skip => self.consent(index, false),
            Choice::Close => {}
        }
        self.comparing = None;
    }
}
