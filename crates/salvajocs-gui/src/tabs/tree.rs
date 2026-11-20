//! A tree of a game's files, with a checkbox on every folder and file to choose what to back up
//! or restore (ADR-0025), and a badge on each file that differs from a backup (ADR-0018).

use std::ops::Range;
use std::path::{Path, PathBuf};

use eframe::egui::collapsing_header::CollapsingState;
use eframe::egui::{Button, Checkbox, Color32, Label, Response, Role, Ui, WidgetInfo};
use salvajocs_core::selection;

use super::format_size;
use crate::tabs::count;
use crate::theme::{self, icons};

/// One row of a [`FileTree`].
pub(crate) struct Row {
    /// The `Base/dir/file` key.
    pub(crate) key: String,
    /// Size in bytes; for a removed row, the backup's.
    pub(crate) size: u64,
    /// The file on disk, whose last components are those of `key` below its Base; `None` when
    /// there is none.
    pub(crate) real: Option<PathBuf>,
    /// The file is only in the backup: a greyed row without a checkbox.
    pub(crate) removed: bool,
}

/// How a file differs from the backup it is compared with, or for a restore, how the file it
/// writes differs from the one at its target (ADR-0019).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    /// Not in the backup.
    New,
    /// In the backup with other content.
    Changed,
    /// In the backup, but the next backup will not hold it.
    Removed,
    /// Could not be read to compare it.
    Unknown,
    /// The restore refuses to write it.
    WillFail,
}

impl Badge {
    fn text(self) -> &'static str {
        match self {
            Badge::New => "new",
            Badge::Changed => "changed",
            Badge::Removed => "removed",
            Badge::Unknown => "could not be compared",
            Badge::WillFail => "will fail",
        }
    }

    /// Its icon of [`icons`]; each has its own shape, so it reads without its color.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Badge::New => icons::PLUS_CIRCLE,
            Badge::Changed => icons::PENCIL_SIMPLE,
            Badge::Removed => icons::MINUS_CIRCLE,
            Badge::Unknown => icons::QUESTION,
            Badge::WillFail => icons::X_CIRCLE,
        }
    }

    /// The color of its icon in the theme `ui` is drawn in.
    pub(crate) fn color(self, ui: &Ui) -> Color32 {
        match self {
            Badge::New => theme::palette(ui).accent,
            Badge::Changed => ui.visuals().warn_fg_color,
            Badge::Removed => ui.visuals().weak_text_color(),
            Badge::Unknown | Badge::WillFail => ui.visuals().error_fg_color,
        }
    }
}

/// How a [`FileTree`] is drawn.
pub(crate) enum Mode<'a> {
    /// A checkbox on every folder and present file; `included` says whether a file key is
    /// included, `enabled` false greys the checkboxes, not the open buttons.
    Select {
        included: &'a dyn Fn(&str) -> bool,
        enabled: bool,
    },
}

/// One file of the tree, from its [`Row`], with the last component of its key as `name`.
struct File {
    /// The position of its row among the rows [`FileTree::new`] was given.
    index: usize,
    key: String,
    name: String,
    size: u64,
    real: Option<PathBuf>,
    removed: bool,
}

/// Where [`FileTree::ui_with_rows`] calls its row slot on a file's row.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Slot<'a> {
    /// At the end of the row, after the file's badge, with the response of the file's name.
    End(&'a Response),
    /// Under the row.
    Below,
}

/// What the user clicked in a [`FileTree`].
pub(crate) enum Clicked {
    /// A folder's or file's checkbox: its key and new value. A folder's key covers every file
    /// below it.
    Toggle(String, bool),
    /// A folder's open button: the folder on disk.
    Open(PathBuf),
}

/// A folder, or the root that holds the Bases. `range` spans its files in [`FileTree::files`],
/// which are sorted so that every folder's files are contiguous.
struct Folder {
    key: String,
    name: String,
    /// The folder on disk, from its first present file; `None` for the root, for a folder with
    /// only removed files and, on Windows, for the `WinAbsolute` Base, which is above the drive
    /// roots.
    real: Option<PathBuf>,
    range: Range<usize>,
    folders: Vec<Folder>,
    /// Indices of the files directly in this folder.
    files: Vec<usize>,
}

impl Folder {
    fn new(key: &str, name: &str, at: usize, real: Option<&Path>) -> Self {
        Folder {
            key: key.to_owned(),
            name: name.to_owned(),
            real: real.map(Path::to_path_buf),
            range: at..at,
            folders: Vec::new(),
            files: Vec::new(),
        }
    }

    /// Drops `real` here and below where it is not a directory.
    fn keep_existing(&mut self) {
        self.real = self.real.take().filter(|real| real.is_dir());
        self.folders.iter_mut().for_each(Folder::keep_existing);
    }
}

/// How many present files are included and their size, and how many files there are of each
/// kind.
#[derive(Clone, Copy, Default)]
struct Included {
    files: usize,
    size: u64,
    present: usize,
    new: usize,
    changed: usize,
    removed: usize,
    unknown: usize,
    will_fail: usize,
}

/// The files of a game as a tree of Bases and folders. It holds no selection: the caller says
/// which file keys are included and gets back what the user toggled.
pub(crate) struct FileTree {
    files: Vec<File>,
    root: Folder,
    /// `included[i]` is the [`Included`] total of `files[..i]`, so a folder's is a difference of
    /// two entries. `None` until the next frame draws, after [`FileTree::invalidate`].
    included: Option<Vec<Included>>,
}

impl FileTree {
    /// Builds the tree from its rows.
    pub(crate) fn new(rows: impl IntoIterator<Item = Row>) -> Self {
        let mut files: Vec<File> = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let name = row.key.rsplit('/').next().unwrap_or_default().to_owned();
                File {
                    index,
                    key: row.key,
                    name,
                    size: row.size,
                    real: row.real,
                    removed: row.removed,
                }
            })
            .collect();
        // By component, not by text, so the folder `a` lists before its sibling `a-b`. Below a
        // Windows-family base, case variants such as `Foo` and `foo` sort together into one folder.
        files.sort_by_cached_key(|file| (folded(&file.key), file.key.clone()));

        let mut root = Folder::new("", "", 0, None);
        for (at, file) in files.iter().enumerate() {
            let mut folder = &mut root;
            folder.range.end = at + 1;
            let mut end = 0;
            // An empty component is a folder with an empty name, so the key stays a literal prefix.
            if let Some((dirs, _)) = file.key.rsplit_once('/') {
                let fold = selection::folds_case(dirs.split('/').next().unwrap_or_default());
                for dir in dirs.split('/') {
                    end += dir.len();
                    let same = |name: &str| {
                        if fold {
                            name.eq_ignore_ascii_case(dir)
                        } else {
                            name == dir
                        }
                    };
                    // For merged case variants, the first present one's.
                    let below = file.key[end..].matches('/').count();
                    let real = (file.real.as_deref())
                        .filter(|_| !file.removed)
                        .and_then(|real| real.ancestors().nth(below));
                    if folder.folders.last().is_none_or(|last| !same(&last.name)) {
                        folder
                            .folders
                            .push(Folder::new(&file.key[..end], dir, at, real));
                    }
                    end += 1;
                    folder = folder
                        .folders
                        .last_mut()
                        .expect("a folder was just pushed or matched");
                    if folder.real.is_none() {
                        folder.real = real.map(Path::to_path_buf);
                    }
                    folder.range.end = at + 1;
                }
            }
            folder.files.push(at);
        }
        FileTree {
            files,
            root,
            included: None,
        }
    }

    /// The tree without the open button of each folder that is not a directory now. Checks each
    /// folder once, so call it when the tree is built, not per frame.
    pub(crate) fn existing_folders_only(mut self) -> Self {
        self.root.keep_existing();
        self
    }

    /// Forgets the cached counts. Call it whenever the set of included keys or the badges change,
    /// such as after applying the toggle [`FileTree::ui_with_rows`] returned.
    pub(crate) fn invalidate(&mut self) {
        self.included = None;
    }

    /// How many present files `included` accepts and their size, from the cached counts.
    /// `badge` must be the one [`FileTree::ui_with_rows`] draws with, by key.
    pub(crate) fn included(
        &mut self,
        included: &dyn Fn(&str) -> bool,
        badge: &dyn Fn(&str) -> Option<Badge>,
    ) -> (usize, u64) {
        let badge = |_: usize, key: &str| badge(key);
        let totals = totals(&mut self.included, &self.files, included, &badge);
        let all = totals.last().copied().unwrap_or_default();
        (all.files, all.size)
    }

    /// Draws the tree in `mode` and returns what was clicked this frame. `badge` gives a present
    /// file's badge by its row's position and key; removed files always have
    /// [`Badge::Removed`]. `hint` gives the tooltip of a file's badge icon the same way, so rows
    /// that share a key can differ. `row` is called on each file row drawn: with the row's
    /// position, its key and where on the row it draws. A row's position is its place among the
    /// rows [`FileTree::new`] was given. The caller scopes the ids (`ui.push_id`) when it draws
    /// several trees.
    pub(crate) fn ui_with_rows(
        &mut self,
        ui: &mut Ui,
        mode: Mode<'_>,
        badge: &dyn Fn(usize, &str) -> Option<Badge>,
        hint: &dyn Fn(usize, &str, Badge) -> String,
        row: &mut dyn FnMut(&mut Ui, usize, &str, Slot<'_>),
    ) -> Option<Clicked> {
        let FileTree {
            files,
            root,
            included: cache,
        } = self;
        let Mode::Select { included, .. } = mode;
        let drawing = Drawing {
            files,
            totals: totals(cache, files, included, badge),
            mode,
            badge,
            hint,
        };
        let mut clicked = None;
        drawing.children(ui, root, &mut clicked, row);
        clicked
    }
}

/// The components of `key`, ASCII-lowercased below a Windows-family base
/// ([`selection::folds_case`]): the order [`FileTree::new`] groups folders by.
fn folded(key: &str) -> Vec<String> {
    let mut parts = key.split('/');
    let base = parts.next().unwrap_or_default();
    let fold = selection::folds_case(base);
    std::iter::once(base.to_owned())
        .chain(parts.map(|part| {
            if fold {
                part.to_ascii_lowercase()
            } else {
                part.to_owned()
            }
        }))
        .collect()
}

/// The badge of `file`: [`Badge::Removed`] when it is a removed row, else what `badge` says.
fn badge_of(file: &File, badge: &dyn Fn(usize, &str) -> Option<Badge>) -> Option<Badge> {
    if file.removed {
        Some(Badge::Removed)
    } else {
        badge(file.index, &file.key)
    }
}

/// The running totals in `cache`, computed from `included` and `badge` when missing.
fn totals<'a>(
    cache: &'a mut Option<Vec<Included>>,
    files: &[File],
    included: &dyn Fn(&str) -> bool,
    badge: &dyn Fn(usize, &str) -> Option<Badge>,
) -> &'a [Included] {
    cache.get_or_insert_with(|| {
        let mut sum = Included::default();
        let mut totals = vec![sum];
        for file in files {
            if !file.removed {
                sum.present += 1;
                if included(&file.key) {
                    sum.files += 1;
                    sum.size += file.size;
                }
            }
            match badge_of(file, badge) {
                Some(Badge::New) => sum.new += 1,
                Some(Badge::Changed) => sum.changed += 1,
                Some(Badge::Removed) => sum.removed += 1,
                Some(Badge::Unknown) => sum.unknown += 1,
                Some(Badge::WillFail) => sum.will_fail += 1,
                None => {}
            }
            totals.push(sum);
        }
        totals
    })
}

/// How many files of `totals` have each badge, as icons with counts, named `2 new, 1 changed`
/// for screen readers and on hover; nothing when there are none.
fn badge_chips(ui: &mut Ui, totals: &Included) {
    let counts = [
        (Badge::New, totals.new),
        (Badge::Changed, totals.changed),
        (Badge::Removed, totals.removed),
        (Badge::Unknown, totals.unknown),
        (Badge::WillFail, totals.will_fail),
    ];
    let label: Vec<_> = (counts.iter())
        .filter(|&&(_, count)| count > 0)
        .map(|&(badge, count)| format!("{count} {}", badge.text()))
        .collect();
    if label.is_empty() {
        return;
    }
    let chips = counts.map(|(badge, count)| (badge.glyph(), badge.color(ui), count));
    theme::chips(ui, &chips, &label.join(", "));
}

/// What one call of [`FileTree::ui_with_rows`] draws with.
struct Drawing<'a> {
    files: &'a [File],
    totals: &'a [Included],
    mode: Mode<'a>,
    badge: &'a dyn Fn(usize, &str) -> Option<Badge>,
    hint: &'a dyn Fn(usize, &str, Badge) -> String,
}

impl Drawing<'_> {
    /// The subfolders of `parent`, then the files directly in it, each with the `row` slot.
    fn children(
        &self,
        ui: &mut Ui,
        parent: &Folder,
        clicked: &mut Option<Clicked>,
        row: &mut dyn FnMut(&mut Ui, usize, &str, Slot<'_>),
    ) {
        for folder in &parent.folders {
            self.folder(ui, folder, clicked, row);
        }
        for &at in &parent.files {
            let file = &self.files[at];
            let badge = badge_of(file, self.badge);
            // The badge joins the name for screen readers, so each row reads as one item.
            let name = match badge {
                Some(badge) => format!("{}, {}", file.name, badge.text()),
                None => file.name.clone(),
            };
            ui.horizontal(|ui| {
                let shown = match &self.mode {
                    Mode::Select { included, enabled } if !file.removed => {
                        let mut checked = included(&file.key);
                        let checkbox = Checkbox::new(&mut checked, &file.name);
                        let response = ui.add_enabled(*enabled, checkbox);
                        if response.changed() {
                            *clicked = Some(Clicked::Toggle(file.key.clone(), checked));
                        }
                        response.widget_info(|| {
                            WidgetInfo::selected(Role::CheckBox, *enabled, checked, &name)
                        });
                        response
                    }
                    _ => {
                        let label = if file.removed {
                            Label::new(weak_text(ui, &file.name))
                        } else {
                            Label::new(&file.name)
                        };
                        let response = ui.add(label);
                        response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &name));
                        response
                    }
                };
                ui.weak(format_size(file.size));
                if let Some(badge) = badge {
                    let hint = (self.hint)(file.index, &file.key, badge);
                    theme::labeled_mark(ui, badge.glyph(), badge.color(ui), &hint);
                }
                row(ui, file.index, &file.key, Slot::End(&shown));
            });
            row(ui, file.index, &file.key, Slot::Below);
        }
    }

    /// A folder as a collapsing header whose children are only built while it is open. A chain of
    /// folders that each hold only the next one is one row, labelled with their joined names. Its
    /// checkbox toggles the top folder, to clear saved choices on every level; counts and the open
    /// button are the deepest folder's. A folder with only removed files has no checkbox.
    fn folder(
        &self,
        ui: &mut Ui,
        first: &Folder,
        clicked: &mut Option<Clicked>,
        row: &mut dyn FnMut(&mut Ui, usize, &str, Slot<'_>),
    ) {
        let mut folder = first;
        let mut label = folder.name.clone();
        while let ([only], true) = (folder.folders.as_slice(), folder.files.is_empty()) {
            folder = only;
            label = format!("{label}/{}", folder.name);
        }
        let Range { start, end } = folder.range;
        let (to, from) = (&self.totals[end], &self.totals[start]);
        let totals = Included {
            files: to.files - from.files,
            size: to.size - from.size,
            present: to.present - from.present,
            new: to.new - from.new,
            changed: to.changed - from.changed,
            removed: to.removed - from.removed,
            unknown: to.unknown - from.unknown,
            will_fail: to.will_fail - from.will_fail,
        };
        let id = ui.make_persistent_id(&folder.key);
        let state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let verb = if state.is_open() { "Close" } else { "Open" };
        let (arrow, ..) = state
            .show_header(ui, |ui| {
                match &self.mode {
                    Mode::Select { enabled, .. } if totals.present > 0 => {
                        // A mixed folder is drawn as unchecked, so a click on it checks everything.
                        let mut checked = totals.files == totals.present;
                        let mixed = totals.files != 0 && !checked;
                        let checkbox = Checkbox::new(&mut checked, &label).indeterminate(mixed);
                        if ui.add_enabled(*enabled, checkbox).changed() {
                            *clicked = Some(Clicked::Toggle(first.key.clone(), checked));
                        }
                        ui.weak(format!(
                            "{} of {}, {}",
                            totals.files,
                            count(totals.present, "file"),
                            format_size(totals.size)
                        ));
                    }
                    // Only removed files: the chips say how many.
                    Mode::Select { .. } => {
                        ui.label(weak_text(ui, &label));
                    }
                }
                badge_chips(ui, &totals);
                if let Some(real) = &folder.real {
                    let name = format!("Show {label} in the file manager");
                    let button = Button::new(theme::icon(icons::ARROW_SQUARE_OUT))
                        .frame_when_inactive(false);
                    let response = ui.add(button);
                    response
                        .widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &name));
                    if response.on_hover_text(&name).clicked() {
                        *clicked = Some(Clicked::Open(real.clone()));
                    }
                }
            })
            .body(|ui| self.children(ui, folder, clicked, row));
        // The arrow is a bare interaction, so name it for screen readers; not the bare folder
        // name, which the checkbox carries.
        arrow.widget_info(|| {
            WidgetInfo::labeled(
                Role::DisclosureTriangle,
                ui.is_enabled(),
                format!("{verb} {label}"),
            )
        });
    }
}

/// `text` in the weak text color, for what is only in the backup.
fn weak_text(ui: &Ui, text: &str) -> eframe::egui::RichText {
    eframe::egui::RichText::new(text).color(ui.visuals().weak_text_color())
}

#[cfg(test)]
mod tests;
