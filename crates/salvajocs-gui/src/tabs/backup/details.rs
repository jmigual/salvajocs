//! The dialog that shows what makes a changed file differ from its backup: both copies' size,
//! mtime and SHA-256, their line endings, and the changed lines of a text file (ADR-0024).

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Label, RichText, ScrollArea, Ui, WidgetText};
use salvajocs_core::changes::BackedUp;
use salvajocs_core::changes::details::{
    self, BackupHash, Content, FileDetails, Hunk, LineEndings, LineKind, LocalFile, TEXT_CAP,
};
use salvajocs_core::scan::GameScan;
use salvajocs_core::selection;
use salvajocs_core::slug::slug;

use crate::job::Job;
use crate::state::{SharedStorage, lock_storage};
use crate::tabs::{format_size, format_time, format_time_seconds};
use crate::theme::{self, icons};
use crate::widgets::form_grid;

/// The most diff lines the dialog draws; the rest are counted.
const MAX_LINES: usize = 1000;

/// The height the diff scrolls above, so the dialog stays short enough for its Close button.
const DIFF_HEIGHT: f32 = 400.0;

/// The details of one changed file, while they are read and once they are shown.
pub(super) struct Details {
    /// The file's name, for the heading.
    name: String,
    /// The file's `<Base>/<tail>` key on this PC.
    key: String,
    local: PathBuf,
    backed_up: BackedUp,
    /// Reads the backup's copy and compares; it only reads, so it is not counted as busy.
    job: Option<Job<FileDetails>>,
    /// What the job gave, or why it failed.
    result: Option<Result<FileDetails, String>>,
}

impl Details {
    /// Starts reading the backup's copy `backed_up` of the file number `file` of `game` and
    /// comparing it with this PC's, holding the repository's lock only for the read.
    pub(super) fn start(
        ctx: &egui::Context,
        storage: &SharedStorage,
        game: &GameScan,
        file: usize,
        backed_up: BackedUp,
    ) -> Self {
        let found = &game.files[file];
        let key = selection::path_key(found);
        let name = key.rsplit('/').next().unwrap_or_default().to_owned();
        let (storage, slug) = (storage.clone(), slug(&game.title));
        let (local, copy) = (found.real.clone(), backed_up.clone());
        let job = Job::spawn(ctx, move || {
            let bytes = {
                let mut storage = lock_storage(&storage)?;
                details::read_backed_up(&mut storage, &slug, &copy, &local)?
            };
            Ok(details::details(&local, &copy, bytes.as_deref()))
        });
        Details {
            name,
            key,
            local: found.real.clone(),
            backed_up,
            job: Some(job),
            result: None,
        }
    }

    /// Takes the job's result once it is there.
    pub(super) fn poll(&mut self) {
        if let Some(job) = &mut self.job
            && let Some(result) = job.take_result()
        {
            self.job = None;
            self.result = Some(result.map_err(|err| format!("{err:#}")));
        }
    }

    /// Draws the dialog; returns whether it was closed.
    pub(super) fn ui(&self, ctx: &egui::Context) -> bool {
        let mut closed = false;
        theme::modal(ctx, "backup_file_details", 720.0, |ui| {
            ui.heading(&self.name);
            ui.weak(&self.key);
            match &self.result {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the backup…");
                    });
                }
                Some(Err(error)) => {
                    let color = ui.visuals().error_fg_color;
                    let text = format!("Could not read the backup: {error}");
                    theme::status(ui, icons::X_CIRCLE, color, text);
                }
                Some(Ok(found)) => self.found_ui(ui, found),
            }
            theme::row_end(ui, |ui| closed |= ui.button("Close details").clicked());
        });
        closed
    }

    /// The two copies' facts, then how their content compares.
    fn found_ui(&self, ui: &mut Ui, found: &FileDetails) {
        let (snapshot, backup) = (&self.backed_up.snapshot, &found.backup);
        let local = found.local.as_ref();
        let unknown = || "unknown".to_owned();
        let time = |time: Option<SystemTime>| time.map_or_else(unknown, format_time_seconds);
        let ends = |ends: Option<LineEndings>| {
            ends.map_or_else(
                || "not text".to_owned(),
                |ends| {
                    let mut text = format!("{} Windows (CRLF), {} Linux (LF)", ends.crlf, ends.lf);
                    if ends.cr > 0 {
                        text += &format!(", {} old Mac (CR)", ends.cr);
                    }
                    text
                },
            )
        };
        let bom = |bom: bool| {
            if bom { "yes" } else { "no" }
        };
        let size = match local {
            Ok(file) => format_size(file.size),
            Err(error) => format!("could not be read: {error}"),
        };
        let here = |text: &dyn Fn(&LocalFile) -> String| local.map_or_else(|_| unknown(), text);
        let there = format!("Backup ({}, {})", snapshot.host, format_time(snapshot.time));
        // A grid's text only wraps below a column width: a long path would otherwise widen the
        // dialog past the window and push Close out of sight.
        form_grid("backup_file_facts")
            .num_columns(3)
            .max_col_width(ui.available_width() / 2.0)
            .show(ui, |ui| {
                row(ui, "", theme::semibold("This PC"), theme::semibold(there));
                row(
                    ui,
                    "Location",
                    self.local.display().to_string(),
                    &self.backed_up.path,
                );
                row(ui, "Size", size, format_size(backup.size));
                let modified = here(&|file| time(file.modified));
                row(ui, "Modified", modified, time(backup.modified));
                if backup.line_endings.is_some() || local.is_ok_and(|f| f.line_endings.is_some()) {
                    let local_ends = here(&|file| ends(file.line_endings));
                    row(ui, "Line endings", local_ends, ends(backup.line_endings));
                }
                // Only a mark is worth a row: most files have none.
                if backup.bom || local.is_ok_and(|file| file.bom) {
                    let local_bom = here(&|file| bom(file.bom).to_owned());
                    row(ui, "Byte order mark", local_bom, bom(backup.bom));
                }
            });
        if let Ok(file) = local {
            hash_ui(ui, "This PC", &file.sha256, None);
        }
        match &backup.sha256 {
            BackupHash::List(hash) => hash_ui(ui, "the backup", hash, Some("from its hash list")),
            BackupHash::Content(hash) => hash_ui(ui, "the backup", hash, Some("from its content")),
            _ => {
                ui.weak("SHA-256 of the backup: not recorded in this backup");
            }
        }
        ui.add_space(8.0);
        content_ui(ui, found);
    }
}

/// One row of the facts grid: `label`, then `local` and `backup`.
fn row(ui: &mut Ui, label: &str, local: impl Into<WidgetText>, backup: impl Into<WidgetText>) {
    ui.label(label);
    ui.label(local);
    ui.label(backup);
    ui.end_row();
}

/// The SHA-256 `hash` of `side`'s copy in monospace, selectable to copy, with where it comes
/// from.
fn hash_ui(ui: &mut Ui, side: &str, hash: &str, source: Option<&str>) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!("SHA-256 of {side}"));
        ui.add(Label::new(RichText::new(hash).monospace()).selectable(true));
        if let Some(source) = source {
            ui.weak(format!("({source})"));
        }
    });
}

/// What the content comparison found: a sentence, or the changed lines.
fn content_ui(ui: &mut Ui, found: &FileDetails) {
    match &found.content {
        Content::Same if matches!(found.backup.sha256, BackupHash::Content(_)) => {
            ui.label(
                "Same content: the file was marked changed only by its modified time, as this \
                 backup has no list of hashes.",
            );
        }
        // Identical text whose listed hash differs: the list is stale, the content is the same.
        Content::Same
        | Content::FormatOnly {
            line_endings: false,
            bom: false,
        } => {
            ui.label("Same content.");
        }
        Content::FormatOnly { line_endings, bom } => {
            let what = match (line_endings, bom) {
                (true, true) => "the line endings and the byte order mark differ",
                (false, true) => "the byte order mark differs",
                _ => "the line endings differ",
            };
            ui.label(format!("Only {what}."));
        }
        Content::Diff(hunks) => diff_ui(ui, hunks),
        Content::NotText => {
            ui.weak("Not shown: not text.");
        }
        Content::TooLarge => {
            let text = format!("Not shown: a copy is over {}.", format_size(TEXT_CAP));
            ui.weak(text);
        }
        // This PC's copy could not be read; the facts above say why.
        _ => {}
    }
}

/// The lines of `hunks`: the backup's removed, this PC's added, each in its color. At most
/// [`MAX_LINES`] are drawn.
fn diff_ui(ui: &mut Ui, hunks: &[Hunk]) {
    let (removed, added) = (ui.visuals().error_fg_color, theme::palette(ui).accent);
    ui.weak("Lines of the backup are marked −, lines on this PC +.");
    let total: usize = hunks.iter().map(|hunk| hunk.lines.len()).sum();
    let mut drawn = 0;
    // Its own scroll area, so the dialog's Close button stays in sight below up to MAX_LINES.
    ScrollArea::vertical()
        .id_salt("backup_file_diff")
        .max_height(DIFF_HEIGHT)
        .show(ui, |ui| {
            'hunks: for (at, hunk) in hunks.iter().enumerate() {
                if at > 0 {
                    ui.separator();
                }
                for line in &hunk.lines {
                    if drawn == MAX_LINES {
                        break 'hunks;
                    }
                    drawn += 1;
                    let (sign, color) = match line.kind {
                        LineKind::Removed => ('−', Some(removed)),
                        LineKind::Added => ('+', Some(added)),
                        _ => (' ', None),
                    };
                    let number =
                        |number: Option<usize>| number.map_or(String::new(), |n| n.to_string());
                    let mut text = format!(
                        "{:>5} {:>5} {sign} {}",
                        number(line.backup_number),
                        number(line.local_number),
                        line.text
                    );
                    if line.missing_newline {
                        text += " (no newline at end)";
                    }
                    let mut text = RichText::new(text).monospace();
                    if let Some(color) = color {
                        text = text.color(color);
                    }
                    ui.add(Label::new(text));
                }
            }
        });
    if total > drawn {
        ui.weak(format!("{} more lines not shown.", total - drawn));
    }
}
