//! The Sort by row above the game lists of the Backup tab and the Restore sidebar, and the line
//! of backup figures under each game (ADR-0027).

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Button, ComboBox, Label, Response, Role, TextStyle, Ui, WidgetInfo};
use salvajocs_core::config::{ListSort, SortKey};
use salvajocs_core::stats::BackupStats;

use crate::tabs::{format_size, format_time};
use crate::theme::{self, icons};

/// The widest the dropdown grows, enough for its longest item.
const COMBO_WIDTH: f32 = 160.0;

/// The dropdown's name of `key`.
fn label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name",
        SortKey::LastBackup => "Last backup",
        SortKey::LastBackupSize => "Last backup size",
        SortKey::SpaceUsed => "Space used",
        _ => "Other",
    }
}

/// The full name and meaning of `key`, the hover text of its item.
fn definition(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name: the game's title",
        SortKey::LastBackup => {
            "Last backup time: the newest backup from any computer, not counting the safety \
             backups a restore makes"
        }
        SortKey::LastBackupSize => {
            "Latest backup size: the size of the files in the last backup, before compression"
        }
        SortKey::SpaceUsed => {
            "Space used by all backups: the space all of the game's backups use in the \
             repository, compressed, safety backups included"
        }
        _ => "",
    }
}

/// The order `sort` shows, in words: what the direction button says.
fn order_words(sort: ListSort) -> &'static str {
    match (sort.by, sort.descending) {
        (SortKey::Name, false) => "A to Z",
        (SortKey::Name, true) => "Z to A",
        (SortKey::LastBackup, false) => "Oldest first",
        (SortKey::LastBackup, true) => "Newest first",
        (_, false) => "Smallest first",
        (_, true) => "Largest first",
    }
}

/// The Sort by row: a dropdown of the keys and a button that reverses the order, the dropdown
/// as wide as the row leaves up to its own width. Returns the sort the user picked, if any;
/// picking a key resets the direction to the key's natural one.
pub(crate) fn sort_ui(ui: &mut Ui, id_salt: &str, sort: ListSort) -> Option<ListSort> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let name = ui.label("Sort by");
        let gap = ui.spacing().item_spacing.x;
        let toggle = 16.0 + 2.0 * ui.spacing().button_padding.x;
        let width = (ui.available_width() - toggle - gap).clamp(0.0, COMBO_WIDTH);
        ui.scope(|ui| {
            ui.set_max_width(width);
            ComboBox::from_id_salt(id_salt)
                .width(width)
                .truncate()
                .selected_text(label(sort.by))
                .show_ui(ui, |ui| {
                    theme::pill_list(ui);
                    for &key in SortKey::ALL {
                        let item = ui.selectable_label(key == sort.by, label(key));
                        if item.on_hover_text(definition(key)).clicked() && key != sort.by {
                            picked = Some(ListSort::new(key, key.descending_by_default()));
                        }
                    }
                })
                .response
                .on_hover_text(definition(sort.by))
                .labelled_by(name.id);
        });
        let glyph = if sort.descending {
            icons::ARROW_DOWN
        } else {
            icons::ARROW_UP
        };
        let words = order_words(sort);
        let button = ui.add(Button::new(theme::icon(glyph)).frame_when_inactive(false));
        button.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), words));
        if button.on_hover_text(words).clicked() {
            picked = Some(ListSort::new(sort.by, !sort.descending));
        }
    });
    picked
}

/// A line of figures under a game: its parts, the emphasized ones in the normal text color and
/// the rest weak, and its hover text. A line without parts is not drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    parts: Vec<(String, bool)>,
    hover: String,
}

impl Line {
    fn push(&mut self, text: impl Into<String>, emphasized: bool) {
        self.parts.push((text.into(), emphasized));
    }

    /// The line's text, without emphasis.
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        self.parts.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// Draws the line, wrapping; `None` when it has no parts.
    pub(crate) fn ui(&self, ui: &mut Ui) -> Option<Response> {
        if self.parts.is_empty() {
            return None;
        }
        let font_id = TextStyle::Body.resolve(ui.style());
        let (strong, weak) = (ui.visuals().text_color(), ui.visuals().weak_text_color());
        let mut job = LayoutJob::default();
        for (text, emphasized) in &self.parts {
            let color = if *emphasized { strong } else { weak };
            job.append(text, 0.0, TextFormat::simple(font_id.clone(), color));
        }
        Some(ui.add(Label::new(job)).on_hover_text(&self.hover))
    }
}

/// What the Backup tab's line of figures means, on hover.
const BACKUP_HOVER: &str = "The last backup is the newest one from any computer, not counting \
    the safety backups a restore makes. Its size is of the files before compression. All \
    backups is the space in the repository that this game's backups use, compressed, safety \
    backups included. Data another game's backups also use counts for both.";

/// The Backup tab's line of a game's `stats` (`None`: the repository has no backup of it) and
/// its `space` (`None` while it is worked out), with the figure `by` sorts by emphasized.
pub(crate) fn backup_line(
    stats: Option<&BackupStats>,
    space: Option<Result<u64, String>>,
    by: SortKey,
) -> Line {
    let mut line = Line {
        hover: BACKUP_HOVER.to_owned(),
        ..Line::default()
    };
    let Some(stats) = stats else {
        line.push("No backup.", false);
        return line;
    };
    match stats.last {
        Some(time) => {
            line.push("Last backup ", false);
            line.push(format_time(time), by == SortKey::LastBackup);
            line.push(", ", false);
            let size = stats
                .last_size
                .map_or("size unknown".to_owned(), format_size);
            line.push(size, by == SortKey::LastBackupSize);
            line.push(". All backups use ", false);
        }
        None => line.push("No backup. Safety backups use ", false),
    }
    let space_used = by == SortKey::SpaceUsed;
    match space {
        None => line.push("…", space_used),
        Some(Ok(bytes)) => {
            line.push(format_size(bytes), space_used);
            line.push(".", false);
        }
        Some(Err(err)) => {
            line.push("an unknown amount", space_used);
            line.push(".", false);
            line.hover += &format!("\n\nThe space could not be worked out: {err}");
        }
    }
    line
}

#[cfg(test)]
mod tests;
