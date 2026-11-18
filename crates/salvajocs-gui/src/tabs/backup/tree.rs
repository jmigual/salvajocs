//! A tree of a game's files, with a checkbox on every folder and file to choose what to back up.

use std::ops::Range;

use eframe::egui::collapsing_header::CollapsingState;
use eframe::egui::{Checkbox, Role, Ui, WidgetInfo};
use salvajocs_core::selection;

use super::format_size;
use crate::tabs::count;

/// One file: its `Base/dir/file` key, the last component of it, and its size.
struct File {
    key: String,
    name: String,
    size: u64,
}

/// A folder, or the root that holds the Bases. `range` spans its files in [`FileTree::files`],
/// which are sorted so that every folder's files are contiguous.
struct Folder {
    key: String,
    name: String,
    range: Range<usize>,
    folders: Vec<Folder>,
    /// Indices of the files directly in this folder.
    files: Vec<usize>,
}

impl Folder {
    fn new(key: &str, name: &str, at: usize) -> Self {
        Folder {
            key: key.to_owned(),
            name: name.to_owned(),
            range: at..at,
            folders: Vec::new(),
            files: Vec::new(),
        }
    }
}

/// How many files are included, and their size.
#[derive(Clone, Copy, Default)]
struct Included {
    files: usize,
    size: u64,
}

/// The files of a game as a tree of Bases and folders. It holds no selection: the caller says
/// which file keys are included and gets back what the user toggled.
pub(super) struct FileTree {
    files: Vec<File>,
    root: Folder,
    /// `included[i]` is the [`Included`] total of `files[..i]`, so a folder's is a difference of
    /// two entries. `None` until the next frame draws, after [`FileTree::invalidate`].
    included: Option<Vec<Included>>,
}

impl FileTree {
    /// Builds the tree from `(key, size)` pairs, `key` being `Base/dir/file`.
    pub(super) fn new(keys: impl IntoIterator<Item = (String, u64)>) -> Self {
        let mut files: Vec<File> = keys
            .into_iter()
            .map(|(key, size)| {
                let name = key.rsplit('/').next().unwrap_or_default().to_owned();
                File { key, name, size }
            })
            .collect();
        // By component, not by text, so the folder `a` lists before its sibling `a-b`. Below a
        // Windows-family base, case variants such as `Foo` and `foo` sort together into one folder.
        files.sort_by_cached_key(|file| (folded(&file.key), file.key.clone()));

        let mut root = Folder::new("", "", 0);
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
                    if folder.folders.last().is_none_or(|last| !same(&last.name)) {
                        folder.folders.push(Folder::new(&file.key[..end], dir, at));
                    }
                    end += 1;
                    folder = folder
                        .folders
                        .last_mut()
                        .expect("a folder was just pushed or matched");
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

    /// Forgets the cached counts. Call it whenever the set of included keys changes, such as after
    /// applying the toggle [`FileTree::ui`] returned.
    pub(super) fn invalidate(&mut self) {
        self.included = None;
    }

    /// How many files `included` accepts and their size, from the cached counts.
    pub(super) fn included(&mut self, included: &dyn Fn(&str) -> bool) -> (usize, u64) {
        let totals = totals(&mut self.included, &self.files, included);
        let all = totals.last().copied().unwrap_or_default();
        (all.files, all.size)
    }

    /// Draws the tree and returns the toggle clicked this frame, as the key of the folder or file
    /// and its new value. A folder's key covers every file below it. `included` says whether a
    /// file key is currently included; `enabled` false greys the checkboxes. The caller scopes the
    /// ids (`ui.push_id`) when it draws several trees.
    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        included: &dyn Fn(&str) -> bool,
        enabled: bool,
    ) -> Option<(String, bool)> {
        let FileTree {
            files,
            root,
            included: cache,
        } = self;
        let drawing = Drawing {
            files,
            totals: totals(cache, files, included),
            enabled,
            included,
        };
        let mut toggled = None;
        drawing.children(ui, root, &mut toggled);
        toggled
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

/// The running totals in `cache`, computed from `included` when missing.
fn totals<'a>(
    cache: &'a mut Option<Vec<Included>>,
    files: &[File],
    included: &dyn Fn(&str) -> bool,
) -> &'a [Included] {
    cache.get_or_insert_with(|| {
        let mut sum = Included::default();
        let mut totals = vec![sum];
        for file in files {
            if included(&file.key) {
                sum.files += 1;
                sum.size += file.size;
            }
            totals.push(sum);
        }
        totals
    })
}

/// What one call of [`FileTree::ui`] draws with.
struct Drawing<'a> {
    files: &'a [File],
    totals: &'a [Included],
    enabled: bool,
    included: &'a dyn Fn(&str) -> bool,
}

impl Drawing<'_> {
    /// The subfolders of `parent`, then the files directly in it.
    fn children(&self, ui: &mut Ui, parent: &Folder, toggled: &mut Option<(String, bool)>) {
        for folder in &parent.folders {
            self.folder(ui, folder, toggled);
        }
        for &at in &parent.files {
            let file = &self.files[at];
            ui.horizontal(|ui| {
                let mut checked = (self.included)(&file.key);
                let checkbox = Checkbox::new(&mut checked, &file.name);
                if ui.add_enabled(self.enabled, checkbox).changed() {
                    *toggled = Some((file.key.clone(), checked));
                }
                ui.weak(format_size(file.size));
            });
        }
    }

    /// A folder as a collapsing header whose children are only built while it is open.
    fn folder(&self, ui: &mut Ui, folder: &Folder, toggled: &mut Option<(String, bool)>) {
        let Range { start, end } = folder.range;
        let total = end - start;
        let files = self.totals[end].files - self.totals[start].files;
        let size = self.totals[end].size - self.totals[start].size;
        let id = ui.make_persistent_id(&folder.key);
        let state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let verb = if state.is_open() { "Close" } else { "Open" };
        let (arrow, ..) = state
            .show_header(ui, |ui| {
                // A mixed folder is drawn as unchecked, so a click on it checks everything.
                let mut checked = files == total;
                let mixed = files != 0 && !checked;
                let checkbox = Checkbox::new(&mut checked, &folder.name).indeterminate(mixed);
                if ui.add_enabled(self.enabled, checkbox).changed() {
                    *toggled = Some((folder.key.clone(), checked));
                }
                ui.weak(format!(
                    "{files} of {}, {}",
                    count(total, "file"),
                    format_size(size)
                ));
            })
            .body(|ui| self.children(ui, folder, toggled));
        // The arrow is a bare interaction, so name it for screen readers; not the bare folder
        // name, which the checkbox carries.
        arrow.widget_info(|| {
            WidgetInfo::labeled(
                Role::DisclosureTriangle,
                ui.is_enabled(),
                format!("{verb} {}", folder.name),
            )
        });
    }
}

#[cfg(test)]
mod tests;
