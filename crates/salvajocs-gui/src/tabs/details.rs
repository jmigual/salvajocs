//! The dialog that shows what makes a changed file differ from its backup: a verdict, both
//! copies' size, mtime and SHA-256 side by side, their line endings, and the changed lines of a
//! text file (ADR-0024, ADR-0031). The Backup and Restore tabs share it.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Label, RichText, ScrollArea, Ui};
use salvajocs_core::changes::BackedUp;
use salvajocs_core::changes::details::{
    self, BackupHash, Content, FileDetails, Hunk, LineEndings, LineKind, LocalFile, TEXT_CAP,
};

use crate::job::Job;
use crate::state::{SharedStorage, lock_storage};
use crate::tabs::{format_size, format_time, format_time_seconds};
use crate::theme::{self, icons};
use crate::widgets::{copyable, form_grid};

/// The most diff lines the dialog draws; the rest are counted.
const MAX_LINES: usize = 1000;

/// The height the diff scrolls above, so the dialog stays short enough for its Close button.
const DIFF_HEIGHT: f32 = 400.0;

/// The details of one changed file, while they are read and once they are shown.
pub(crate) struct Details {
    /// The file's name, for the heading.
    name: String,
    /// The file's `<Base>/<tail>` key on this PC.
    key: String,
    storage: SharedStorage,
    /// The game's slug, to find its files in the repository.
    slug: String,
    local: PathBuf,
    backed_up: BackedUp,
    /// Reads the backup's copy and compares; it only reads, so it is not counted as busy.
    job: Option<Job<FileDetails>>,
    /// What the job gave, or why it failed.
    result: Option<Result<FileDetails, String>>,
}

impl Details {
    /// Starts reading the backup's copy `backed_up` of the file `key` (its `<Base>/<tail>` key on
    /// this PC) of the game `slug` and comparing it with this PC's file at `local`, holding the
    /// repository's lock only for the read. A copy over [`TEXT_CAP`] is not read until the user
    /// asks for it.
    pub(crate) fn start(
        ctx: &egui::Context,
        storage: &SharedStorage,
        slug: String,
        key: String,
        local: PathBuf,
        backed_up: BackedUp,
    ) -> Self {
        let name = key.rsplit('/').next().unwrap_or_default().to_owned();
        let mut details = Details {
            name,
            key,
            storage: storage.clone(),
            slug,
            local,
            backed_up,
            job: None,
            result: None,
        };
        details.read(ctx, Some(TEXT_CAP));
        details
    }

    /// Starts the job that reads and compares the two copies, within `cap` bytes each.
    fn read(&mut self, ctx: &egui::Context, cap: Option<u64>) {
        let storage = self.storage.clone();
        let (slug, path, copy) = (
            self.slug.clone(),
            self.local.clone(),
            self.backed_up.clone(),
        );
        self.result = None;
        self.job = Some(Job::spawn(ctx, move || {
            let bytes = {
                let mut storage = lock_storage(&storage)?;
                details::read_backed_up(&mut storage, &slug, &copy, &path, cap)?
            };
            Ok(details::details(&path, &copy, bytes.as_deref(), cap))
        }));
    }

    /// Takes the job's result once it is there.
    pub(crate) fn poll(&mut self) {
        if let Some(job) = &mut self.job
            && let Some(result) = job.take_result()
        {
            self.job = None;
            self.result = Some(result.map_err(|err| format!("{err:#}")));
        }
    }

    /// Draws the dialog; returns whether it was closed. **Show anyway** reads the copies again
    /// without the size cap.
    pub(crate) fn ui(&mut self, ctx: &egui::Context) -> bool {
        let (mut closed, mut anyway) = (false, false);
        let ((), dismissed) = theme::modal_dismissible(ctx, "backup_file_details", 720.0, |ui| {
            ui.heading(&self.name);
            copyable(ui, RichText::new(&self.key).weak(), &self.key);
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
                Some(Ok(found)) => self.found_ui(ui, found, &mut anyway),
            }
            theme::row_end(ui, |ui| closed |= ui.button("Close details").clicked());
        });
        if anyway {
            self.read(ctx, None);
        }
        closed || dismissed
    }

    /// How the content compares, the two copies' facts side by side, then the changed lines of a
    /// text; `anyway` is set when the **Show anyway** button of a copy over the cap is clicked.
    fn found_ui(&self, ui: &mut Ui, found: &FileDetails, anyway: &mut bool) {
        ui.add_space(4.0);
        verdict_ui(ui, found, anyway);
        ui.add_space(8.0);
        self.facts_ui(ui, found);
        if let Content::Diff(hunks) = &found.content {
            ui.add_space(8.0);
            diff_ui(ui, hunks);
        }
    }

    /// The facts grid: a column for this PC's copy and one for the backup's, with a mark after
    /// each label whose two values are the same or differ.
    fn facts_ui(&self, ui: &mut Ui, found: &FileDetails) {
        let (snapshot, backup) = (&self.backed_up.snapshot, &found.backup);
        let local = found.local.as_ref();
        let time = |time: Option<SystemTime>| {
            time.map_or_else(|| "unknown".to_owned(), format_time_seconds)
        };
        let ends = |ends: Option<LineEndings>, read: bool| {
            if !read {
                return Value::weak("not read");
            }
            Value::text(ends.map_or_else(
                || "not text".to_owned(),
                |ends| {
                    let mut text = format!("{} Windows (CRLF), {} Linux (LF)", ends.crlf, ends.lf);
                    if ends.cr > 0 {
                        text += &format!(", {} old Mac (CR)", ends.cr);
                    }
                    text
                },
            ))
        };
        let bom = |bom: bool, read: bool| {
            if !read {
                Value::weak("not read")
            } else {
                Value::text(if bom { "yes" } else { "no" })
            }
        };
        let here = |text: &dyn Fn(&LocalFile) -> String| {
            local.map_or_else(|_| Value::weak("unknown"), |file| Value::text(text(file)))
        };
        // `None` when this PC's copy could not be read, so there is nothing to compare.
        let same = |same: &dyn Fn(&LocalFile) -> bool| local.ok().map(same);
        // A copy over the size cap is not read, so it has no line endings or byte order mark to
        // compare; the backup's is not read whenever either copy is over the cap.
        let capped = matches!(found.content, Content::TooLarge);
        let local_read = !(capped && local.is_ok_and(|file| file.size > TEXT_CAP));
        let widths = Widths::new(ui);
        form_grid("backup_file_facts")
            .num_columns(4)
            .show(ui, |ui| {
                ui.label("");
                ui.label("");
                column(ui, widths.value, |ui| {
                    ui.label(theme::semibold("This PC"));
                });
                column(ui, widths.value, |ui| {
                    ui.label(theme::semibold("Backup"));
                    ui.weak(format!("{}, {}", snapshot.host, format_time(snapshot.time)));
                });
                ui.end_row();

                let (local_path, path) = (self.local.display().to_string(), &self.backed_up.path);
                row(
                    ui,
                    &widths,
                    "Location",
                    None,
                    Value::text(local_path),
                    Value::text(path),
                );

                let local_size = match local {
                    Ok(file) => Value::text(format_size(file.size)),
                    Err(error) => Value::error(ui, format!("could not be read: {error}")),
                };
                let same_size = same(&|file| file.size == backup.size);
                let size = Value::text(format_size(backup.size));
                row(ui, &widths, "Size", same_size, local_size, size);

                let modified = here(&|file| time(file.modified));
                // Only a time that both copies have can differ.
                let same_time = backup.modified.and_then(|there| {
                    let here = local.ok()?.modified?;
                    Some(format_time_seconds(here) == format_time_seconds(there))
                });
                let there = Value::text(time(backup.modified));
                row(ui, &widths, "Modified", same_time, modified, there);

                let local_hash = local.map_or_else(
                    |_| Value::weak("unknown"),
                    |file| Value::hash(&file.sha256, None),
                );
                let backup_hash = match &backup.sha256 {
                    BackupHash::List(hash) => Value::hash(hash, Some("from its hash list")),
                    BackupHash::Content(hash) => Value::hash(hash, Some("from its content")),
                    _ => Value::weak("not recorded in this backup"),
                };
                let same_hash =
                    (backup.sha256.hex()).and_then(|hex| same(&|file| file.sha256 == hex));
                row(ui, &widths, "SHA-256", same_hash, local_hash, backup_hash);

                if backup.line_endings.is_some() || local.is_ok_and(|f| f.line_endings.is_some()) {
                    let local_ends = local.map_or_else(
                        |_| Value::weak("unknown"),
                        |file| ends(file.line_endings, local_read),
                    );
                    let same_ends =
                        same(&|file| file.line_endings == backup.line_endings).filter(|_| !capped);
                    let there = ends(backup.line_endings, !capped);
                    row(ui, &widths, "Line endings", same_ends, local_ends, there);
                }
                // Only a mark is worth a row: most files have none.
                if backup.bom || local.is_ok_and(|file| file.bom) {
                    let local_bom = local
                        .map_or_else(|_| Value::weak("unknown"), |file| bom(file.bom, local_read));
                    let same_bom = same(&|file| file.bom == backup.bom).filter(|_| !capped);
                    let there = bom(backup.bom, !capped);
                    row(ui, &widths, WIDEST_LABEL, same_bom, local_bom, there);
                }
            });
    }
}

/// The gap between the facts grid's columns, as [`form_grid`] sets it.
const COLUMN_GAP: f32 = 8.0;

/// The side of the mark after a label, which has a column of its own so the marks line up.
const MARK_SIZE: f32 = 16.0;

/// The widest label of the facts grid, the byte order mark's, which sizes its label column.
const WIDEST_LABEL: &str = "Byte order mark";

/// The widths of the cells of the facts grid.
struct Widths {
    /// Each of the two value columns, which split what the label and mark columns leave.
    value: f32,
}

impl Widths {
    /// The widths for a grid across `ui`. A grid's text only wraps below a column width: a long
    /// path would otherwise widen the dialog past the window and push Close out of sight.
    fn new(ui: &Ui) -> Self {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text =
            (ui.painter()).layout_no_wrap(WIDEST_LABEL.to_owned(), font, ui.visuals().text_color());
        let value = (ui.available_width() - text.size().x - MARK_SIZE - 3.0 * COLUMN_GAP) / 2.0;
        Widths {
            value: value.max(0.0),
        }
    }
}

/// One value of the facts grid: what it shows, what **Copy** copies and a weak line below it.
struct Value {
    shown: RichText,
    copied: String,
    note: Option<&'static str>,
}

impl Value {
    /// A value in the body text.
    fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Value {
            shown: RichText::new(&text),
            copied: text,
            note: None,
        }
    }

    /// A value in the weak text, for what is missing.
    fn weak(text: &'static str) -> Self {
        Value {
            shown: RichText::new(text).weak(),
            copied: text.to_owned(),
            note: None,
        }
    }

    /// A value in the error color.
    fn error(ui: &Ui, text: String) -> Self {
        Value {
            shown: RichText::new(&text).color(ui.visuals().error_fg_color),
            copied: text,
            note: None,
        }
    }

    /// A SHA-256 in monospace; it wraps in its column, and **Copy** gives the bare hex.
    fn hash(hex: &str, note: Option<&'static str>) -> Self {
        Value {
            shown: RichText::new(hex).monospace(),
            copied: hex.to_owned(),
            note,
        }
    }
}

/// A cell of `width` of the facts grid, holding what `add_contents` draws.
fn column(ui: &mut Ui, width: f32, add_contents: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.set_width(width);
        add_contents(ui);
    });
}

/// One row of the facts grid: `label` with a mark that says whether the copies are the `same`
/// (none when `None`), then the values of this PC's copy and the backup's.
fn row(ui: &mut Ui, widths: &Widths, label: &str, same: Option<bool>, local: Value, backup: Value) {
    // Vertical cells sit at the top of a row that a wrapped value makes tall.
    ui.vertical(|ui| ui.add(Label::new(label).extend()));
    ui.vertical(|ui| match same {
        Some(true) => {
            let weak = ui.visuals().weak_text_color();
            theme::labeled_mark(ui, icons::EQUALS, weak, "Same");
        }
        Some(false) => {
            let warn = ui.visuals().warn_fg_color;
            theme::labeled_mark(ui, icons::PENCIL_SIMPLE, warn, "Differs");
        }
        None => {}
    });
    for value in [local, backup] {
        column(ui, widths.value, |ui| {
            copyable(ui, value.shown, &value.copied);
            if let Some(note) = value.note {
                ui.weak(note);
            }
        });
    }
    ui.end_row();
}

/// The sentence that says how the content compares, after its icon, and the button that lifts
/// the size cap when a copy is over it; `anyway` is set when it is clicked.
fn verdict_ui(ui: &mut Ui, found: &FileDetails, anyway: &mut bool) {
    let (warn, weak) = (ui.visuals().warn_fg_color, ui.visuals().weak_text_color());
    let accent = theme::palette(ui).accent;
    let not_shown = |text: String| (icons::MINUS, weak, RichText::new(text).weak());
    let (glyph, color, text) = match &found.content {
        // Identical text whose listed hash differs: the list is stale, the content is the same.
        Content::Same
        | Content::FormatOnly {
            line_endings: false,
            bom: false,
        } => (
            icons::CHECK_CIRCLE,
            accent,
            theme::semibold("Same content."),
        ),
        Content::FormatOnly { line_endings, bom } => {
            let what = match (line_endings, bom) {
                (true, true) => "the line endings and the byte order mark differ",
                (false, true) => "the byte order mark differs",
                _ => "the line endings differ",
            };
            let text = theme::semibold(format!("Only {what}."));
            (icons::PENCIL_SIMPLE, warn, text)
        }
        Content::Diff(hunks) => {
            let count = |kind| {
                let lines = hunks.iter().flat_map(|hunk| &hunk.lines);
                lines.filter(|line| line.kind == kind).count()
            };
            let removed = count(LineKind::Removed);
            let line = if removed == 1 { "line" } else { "lines" };
            let added = count(LineKind::Added);
            let text = format!("The text differs: {removed} {line} removed, {added} added.");
            (icons::PENCIL_SIMPLE, warn, theme::semibold(text))
        }
        Content::NotText => not_shown(not_text(found)),
        Content::TooLarge => not_shown(too_large(found)),
        _ => not_shown("Diff not shown: this PC's copy could not be read.".to_owned()),
    };
    theme::icon_label(ui, glyph, color, text);
    if matches!(found.content, Content::Same)
        && matches!(found.backup.sha256, BackupHash::Content(_))
    {
        ui.weak(
            "It was marked changed only by its modified time, as this backup has no list of \
             hashes.",
        );
    }
    if matches!(found.content, Content::TooLarge) {
        let button = ui.button("Show anyway");
        *anyway |= button
            .on_hover_text("Downloads the backup's copy and compares the whole file")
            .clicked();
    }
}

/// Why no diff is shown when a copy is not text, naming the copy.
fn not_text(found: &FileDetails) -> String {
    let local = (found.local.as_ref()).is_ok_and(|file| file.line_endings.is_none());
    let who = match (local, found.backup.line_endings.is_none()) {
        (true, true) => "neither copy is text",
        (true, false) => "this PC's copy is not text",
        _ => "the backup's copy is not text",
    };
    format!("Diff not shown: {who}.")
}

/// Why no diff is shown when a copy is over [`TEXT_CAP`], naming the copy.
fn too_large(found: &FileDetails) -> String {
    let cap = format_size(TEXT_CAP);
    let backup = found.backup.size;
    match found.local.as_ref().ok().map(|file| file.size) {
        Some(local) if local > TEXT_CAP && backup > TEXT_CAP => {
            format!("Diff not shown: both copies are over {cap}.")
        }
        Some(local) if local > TEXT_CAP => {
            let local = format_size(local);
            format!("Diff not shown: this PC's copy is {local}, over the {cap} limit.")
        }
        _ if backup > TEXT_CAP => {
            let backup = format_size(backup);
            format!("Diff not shown: the backup's copy is {backup}, over the {cap} limit.")
        }
        _ => format!("Diff not shown: a copy is over the {cap} limit."),
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
