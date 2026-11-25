//! The Sort by row above the game lists of the Backup tab and the Restore sidebar, and the line
//! of backup figures under each game (ADR-0027).

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Button, ComboBox, Label, Response, Role, Sense, TextStyle, Ui, WidgetInfo};
use salvajocs_core::config::{ListSort, SortKey};
use salvajocs_core::stats::BackupStats;

use crate::tabs::{format_date, format_size, format_time};
use crate::theme::{self, icons};

/// The widest the dropdown grows, enough for its longest item.
const COMBO_WIDTH: f32 = 160.0;

/// The keys the Restore sidebar lists: every key but [`SortKey::Changes`], which compares with
/// what the Backup tab previews (ADR-0032).
pub(crate) const RESTORE_KEYS: &[SortKey] = &[
    SortKey::Name,
    SortKey::LastBackup,
    SortKey::LastBackupSize,
    SortKey::SpaceUsed,
];

/// The dropdown's name of `key`.
fn label(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "Name",
        SortKey::LastBackup => "Last backup",
        SortKey::LastBackupSize => "Last backup size",
        SortKey::SpaceUsed => "Space used",
        SortKey::Changes => "Changes",
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
        SortKey::Changes => {
            "Changes: how many files the next backup would add, change or remove compared with \
             the last backup"
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
        (SortKey::Changes, false) => "Fewest changes first",
        (SortKey::Changes, true) => "Most changes first",
        (_, false) => "Smallest first",
        (_, true) => "Largest first",
    }
}

/// The Sort by row: a dropdown of `keys` and a button that reverses the order, the dropdown
/// as wide as the row leaves up to its own width. Returns the sort the user picked, if any;
/// picking a key resets the direction to the key's natural one.
pub(crate) fn sort_ui(
    ui: &mut Ui,
    id_salt: &str,
    keys: &[SortKey],
    sort: ListSort,
) -> Option<ListSort> {
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
                    for &key in keys {
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
    fn text(&self) -> String {
        self.parts.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// Draws the line, wrapping; `None` when it has no parts.
    pub(crate) fn ui(&self, ui: &mut Ui) -> Option<Response> {
        if self.parts.is_empty() {
            return None;
        }
        let label = Label::new(self.job(ui, TextStyle::Body));
        Some(ui.add(label).on_hover_text(&self.hover))
    }

    /// Draws the line small and cut to the width there is, as a target that takes clicks.
    pub(crate) fn compact_ui(&self, ui: &mut Ui) -> Response {
        let label = Label::new(self.job(ui, TextStyle::Small))
            .truncate()
            .sense(Sense::click());
        ui.add(label).on_hover_text(&self.hover)
    }

    /// The parts in `style`, the emphasized ones in the text color and the rest weak.
    fn job(&self, ui: &Ui, style: TextStyle) -> LayoutJob {
        let font_id = style.resolve(ui.style());
        let (strong, weak) = (ui.visuals().text_color(), ui.visuals().weak_text_color());
        let mut job = LayoutJob::default();
        for (text, emphasized) in &self.parts {
            let color = if *emphasized { strong } else { weak };
            job.append(text, 0.0, TextFormat::simple(font_id.clone(), color));
        }
        job
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

/// The Restore sidebar's short line of a listed game's `stats` and `space` (`None` while it is
/// worked out): the figure `by` sorts by first and emphasized, the date without the time, and
/// an unknown size left out. Its hover text is the Backup tab's sentence.
pub(crate) fn restore_line(
    stats: &BackupStats,
    space: Option<Result<u64, String>>,
    by: SortKey,
) -> Line {
    let full = backup_line(Some(stats), space.clone(), by);
    let used = match space {
        None => "… used".to_owned(),
        Some(Ok(bytes)) => format!("{} used", format_size(bytes)),
        Some(Err(_)) => "? used".to_owned(),
    };
    let used = Some((used, by == SortKey::SpaceUsed));
    let figures = match stats.last {
        None => [Some(("Only safety backups".to_owned(), false)), used, None],
        Some(time) => {
            let date = Some((format_date(time), by == SortKey::LastBackup));
            let size =
                (stats.last_size).map(|size| (format_size(size), by == SortKey::LastBackupSize));
            match by {
                SortKey::LastBackupSize => [size, date, used],
                SortKey::SpaceUsed => [used, date, size],
                _ => [date, size, used],
            }
        }
    };
    let mut line = Line {
        hover: format!("{}\n\n{}", full.text(), full.hover),
        ..Line::default()
    };
    for (index, (text, emphasized)) in figures.into_iter().flatten().enumerate() {
        if index > 0 {
            line.push(", ", false);
        }
        line.push(text, emphasized);
    }
    line
}

#[cfg(test)]
mod tests;
