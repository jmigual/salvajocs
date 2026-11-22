use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui::{self, Checkbox, RichText, Role, Ui, WidgetInfo};
use salvajocs_core::changes::{self, Comparison, Counts, FileState, Mapper, NoSkip};
use salvajocs_core::crossbuild::{self, Confidence, MappingEntry, ResolveError};
use salvajocs_core::layout::HASHES_FILE;
use salvajocs_core::scan::{Environment, GameScan};
use salvajocs_core::selection::{self, Change, GameSettings};
use salvajocs_core::slug::slug;
use salvajocs_core::stats;
use salvajocs_core::storage::SnapshotInfo;

use super::{BackupTab, lock};
use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::tabs::mapping::DialogAction;
use crate::tabs::tree::{Badge, Clicked, FileTree, Mode, Row, Slot};
use crate::tabs::{count, format_size, format_time};
use crate::theme::{self, icons};

/// What the scan found for a game: the count and size of the files backed up, given as
/// `included`, how many are excluded, whether registry keys are backed up, and unreadable paths.
pub(super) fn summary(
    game: &GameScan,
    (files, size): (usize, u64),
    settings: &GameSettings,
) -> String {
    let mut text = format!("{}, {}", count(files, "file"), format_size(size));
    let excluded = game.files.len() - files;
    if excluded > 0 {
        text += &format!(", {excluded} excluded");
    }
    let keys = &game.registry_keys;
    if game.registry_source.is_some() && keys.iter().any(|key| settings.registry_included(key)) {
        text += ", plus registry keys";
    }
    if !game.unreadable.is_empty() {
        text += &format!(
            ", {} (the files may be incomplete)",
            count(game.unreadable.len(), "unreadable path")
        );
    }
    text
}

/// What a game's files are compared with: its newest backup from any host (ADR-0018), mapped
/// when it is of the other build (ADR-0021).
pub(super) struct Baseline {
    pub(super) newest: changes::Baseline,
    pub(super) comparison: Comparison,
    /// The state of each found file, by its path key.
    pub(super) states: HashMap<String, FileState>,
    /// This machine's last backup of the game and its files; `None` when it has none or it has
    /// no hash list, which staging now adds, so rustic finds no backup unchanged.
    pub(super) parent: Option<(SnapshotInfo, BackedUpFiles)>,
}

impl Baseline {
    /// Whether the newest backup has a hash list that matches its files; without one, changes
    /// are judged by mtime and a backup is made even when nothing differs (ADR-0018).
    fn hashed(&self) -> bool {
        self.newest.compared().1.is_some()
    }
}

/// The size and mtime of each file below `files/` of a backup, by path key.
type BackedUpFiles = HashMap<String, (u64, Option<SystemTime>)>;

/// One game's preview: `None` when the repository has no backup of it, `Err` when it could not
/// be compared.
pub(super) type Preview = Result<Option<Baseline>, String>;

/// Lists each of `games`' newest backup and this machine's last one, and compares the found
/// files with the newest (ADR-0018), through `mapper` when it is of the other build (ADR-0021).
/// Hands the repository's snapshots to `listed` before comparing. Locks the repository for one
/// game at a time.
pub(super) fn preview(
    storage: &SharedStorage,
    games: &[GameScan],
    mapper: &Mapper,
    listed: impl FnOnce(&[SnapshotInfo]),
) -> anyhow::Result<Vec<Preview>> {
    let (snapshots, host) = {
        let mut storage = lock_storage(storage)?;
        storage.reindex()?;
        (storage.snapshots("salvajocs")?, storage.hostname()?)
    };
    listed(&snapshots);
    let previews = games.iter().map(|game| {
        preview_game(storage, game, &snapshots, &host, mapper).map_err(|err| format!("{err:#}"))
    });
    Ok(previews.collect())
}

fn preview_game(
    storage: &SharedStorage,
    game: &GameScan,
    snapshots: &[SnapshotInfo],
    host: &str,
    mapper: &Mapper,
) -> anyhow::Result<Option<Baseline>> {
    let slug = slug(&game.title);
    let Some(newest) = changes::newest(snapshots, &slug) else {
        return Ok(None);
    };
    let game_tag = format!("game:{slug}");
    // rustic's parent: the latest of this host, label and path, `pre-restore` included.
    let parent = snapshots.iter().find(|snapshot| {
        snapshot.host == host && snapshot.label == game.title && snapshot.tags.contains(&game_tag)
    });
    let as_path = format!("/salvajocs/{slug}");
    let (baseline, parent_listing) = {
        let mut storage = lock_storage(storage)?;
        let baseline = mapper.baseline(&mut storage, newest, game)?;
        let parent_listing = match parent {
            Some(parent) if parent.id != newest.id => {
                Some(storage.snapshot_files(&parent.id, &as_path)?)
            }
            _ => None,
        };
        (baseline, parent_listing)
    };
    let (listing, hashes) = baseline.compared();
    let comparison = changes::compare_game(game, listing, hashes);
    let states = (game.files.iter().map(selection::path_key))
        .zip(comparison.found.iter().cloned())
        .collect();
    // rustic compares trees by path, so (A) is predicted against the snapshot's own paths.
    let parent_listing = parent_listing.as_ref().unwrap_or(&baseline.listing);
    let parent = (parent.filter(|_| parent_listing.iter().any(|file| file.path == HASHES_FILE)))
        .map(|parent| {
            let prefix = "files/";
            let files = (parent_listing.iter())
                .filter_map(|file| {
                    let key = file.path.strip_prefix(prefix)?;
                    Some((key.to_owned(), (file.size, file.modified)))
                })
                .collect();
            (parent.clone(), files)
        });
    Ok(Some(Baseline {
        newest: baseline,
        comparison,
        states,
        parent,
    }))
}

/// The badge of each file key of `game` by `preview` and the game's `settings`: `new`,
/// `changed` and `could not be compared` for included files, `removed` for unticked ones the
/// newest backup has, unless the game has unreadable paths (staging then keeps every file) or
/// is skipped.
pub(super) fn badges<'a>(
    game: &'a GameScan,
    preview: Option<&'a Preview>,
    settings: &'a GameSettings,
) -> impl Fn(&str) -> Option<Badge> + 'a {
    let states = match preview {
        Some(Ok(Some(baseline))) if !settings.skip => Some(&baseline.states),
        _ => None,
    };
    let removes = game.unreadable.is_empty();
    move |key| match (settings.path_included(key), states?.get(key)?) {
        (true, FileState::New) => Some(Badge::New),
        (true, FileState::Changed) => Some(Badge::Changed),
        (true, FileState::Unknown(_)) => Some(Badge::Unknown),
        (true, FileState::Same) | (false, FileState::New) => None,
        (true, _) => Some(Badge::Unknown),
        (false, _) => removes.then_some(Badge::Removed),
    }
}

/// Why a change was judged by size and modification time: the backup has no hash list.
const BY_MTIME: &str = "by modified time: this backup has no list of hashes";

/// The tooltip of a file's `badge` by its key, with why it could not be compared when `preview`
/// says.
pub(super) fn hint(preview: Option<&Preview>, key: &str, badge: Badge) -> String {
    match badge {
        Badge::New => "New: not in the backup".to_owned(),
        Badge::Changed => match preview {
            Some(Ok(Some(baseline))) if !baseline.hashed() => {
                format!("Changed since the backup ({BY_MTIME})")
            }
            _ => "Changed since the backup".to_owned(),
        },
        Badge::Removed => match preview {
            Some(Ok(Some(baseline)))
                if (baseline.newest.mapped.as_ref())
                    .is_some_and(|mapped| mapped.unplaced.contains(key)) =>
            {
                "Removed: the newest backup is from the other build, and this file has no place \
                 here (its folder is unknown, or another file takes its path)"
                    .to_owned()
            }
            _ => "Removed: the next backup will not hold it".to_owned(),
        },
        Badge::Unknown | Badge::WillFail => {
            if let Some(Ok(Some(baseline))) = preview
                && let Some(FileState::Unknown(reason)) = baseline.states.get(key)
            {
                format!("Could not be compared: {reason}")
            } else {
                "Could not be compared".to_owned()
            }
        }
    }
}

/// The tree of `game`'s files, with the files only in its newest backup as removed rows unless
/// staging keeps every file.
fn file_tree(game: &GameScan, preview: Option<&Preview>) -> FileTree {
    let found = game.files.iter().map(|file| Row {
        key: selection::path_key(file),
        size: file.size,
        real: Some(file.real.clone()),
        removed: false,
    });
    let gone = match preview {
        Some(Ok(Some(baseline))) if game.unreadable.is_empty() => {
            baseline.comparison.gone.as_slice()
        }
        _ => &[],
    };
    let removed = gone.iter().map(|file| Row {
        key: file.path.clone(),
        size: file.size,
        real: None,
        removed: true,
    });
    FileTree::new(found.chain(removed))
}

/// `2 new, 1 changed, 1 removed, 1 could not be compared`, leaving out the kinds there are none
/// of; empty when there is none of any.
fn counted(counts: &Counts) -> String {
    let parts = [
        (counts.new, "new"),
        (counts.changed, "changed"),
        (counts.removed, "removed"),
        (counts.unknown, "could not be compared"),
    ];
    let parts: Vec<_> = (parts.into_iter())
        .filter(|&(count, _)| count > 0)
        .map(|(count, kind)| format!("{count} {kind}"))
        .collect();
    parts.join(", ")
}

/// What the next backup would do to a game, by its preview: the state its header shows.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Next<'a> {
    /// The repository has no backup of the game.
    NoBackupYet,
    /// The preview failed; holds why.
    Failed(&'a str),
    /// The included files are those of the newest backup, `newest`. It is skipped unless
    /// `blocker` says why it is made all the same.
    Same {
        newest: &'a SnapshotInfo,
        blocker: Option<NoSkip>,
    },
    /// Unchanged since this PC's last backup, `parent`, so it is skipped, though the newest
    /// backup differs by `counts`.
    UnchangedHere {
        parent: &'a SnapshotInfo,
        newest: &'a SnapshotInfo,
        counts: Counts,
    },
    /// Made, with `counts` of differences from the newest backup.
    Changes {
        newest: &'a SnapshotInfo,
        counts: Counts,
        hashed: bool,
    },
}

impl Next<'_> {
    /// The one icon its header shows: a check when no backup will be made, an equals sign when
    /// the same files are backed up again, a plus when there is no backup yet. `None` when the
    /// header counts the changes, or for a failed preview, which is text.
    pub(super) fn glyph(&self) -> Option<&'static str> {
        match self {
            Next::NoBackupYet => Some(icons::PLUS_CIRCLE),
            Next::Same {
                blocker: Some(_), ..
            } => Some(icons::EQUALS),
            Next::Same { blocker: None, .. } | Next::UnchangedHere { .. } => {
                Some(icons::CHECK_CIRCLE)
            }
            Next::Failed(_) | Next::Changes { .. } => None,
        }
    }
}

/// What the next backup would do to `game` compared with its newest backup, by `settings`;
/// `None` when the game would not be backed up at all.
pub(super) fn next_backup<'a>(
    game: &GameScan,
    preview: &'a Preview,
    settings: &GameSettings,
) -> Option<Next<'a>> {
    let baseline = match preview {
        Ok(Some(baseline)) => baseline,
        Ok(None) => return selection::apply(game, settings).map(|_| Next::NoBackupYet),
        Err(err) => return Some(Next::Failed(err)),
    };
    let counts = changes::counts(game, &baseline.comparison, settings)?;
    let newest = &baseline.newest.snapshot;
    if counts == Counts::default() {
        // The scan the backup stores, whose registry keys may all be unticked.
        let applied = selection::apply(game, settings)?;
        return Some(Next::Same {
            newest,
            blocker: baseline.newest.skip_blocker(&applied),
        });
    }
    Some(match &baseline.parent {
        Some((parent, files)) if counts.unknown == 0 && unchanged_since(game, settings, files) => {
            Next::UnchangedHere {
                parent,
                newest,
                counts,
            }
        }
        _ => Next::Changes {
            newest,
            counts,
            hashed: baseline.hashed(),
        },
    })
}

/// [`next_backup`] as a sentence, with what [`noted`] adds; `None` when the game would not be
/// backed up at all.
pub(super) fn preview_text(
    game: &GameScan,
    preview: &Preview,
    settings: &GameSettings,
) -> Option<String> {
    next_backup(game, preview, settings).map(|next| noted(sentence(&next), preview))
}

/// `text` followed, when `preview`'s newest backup is of the other build, by whether its
/// folders were matched to this machine's, which have no known place here, or that some of its
/// files have no place (ADR-0021).
fn noted(text: String, preview: &Preview) -> String {
    let mapped = (preview.as_ref().ok().and_then(Option::as_ref))
        .and_then(|baseline| baseline.newest.mapped.as_ref());
    let Some(mapped) = mapped else {
        return text;
    };
    if mapped.unplaced_roots.is_empty() {
        return if mapped.unplaced.is_empty() {
            format!(
                "{text}. The backup is from the other build; its folders were matched to this \
                 machine's."
            )
        } else {
            format!(
                "{text}. The newest backup is from the other build; some of its files have no \
                 place here."
            )
        };
    }
    let folders: Vec<String> = (mapped.unplaced_roots.iter())
        .map(crossbuild::root_template)
        .collect();
    let text = format!(
        "{text}. The newest backup is from the other build; these folders have no known place \
         here: {}.",
        folders.join(", ")
    );
    if unknown(&mapped.entries) > 0 {
        format!("{text} Choose folders… places them.")
    } else {
        text
    }
}

/// The roots of `preview`'s newest backup, of the other build, for the mapping dialog, when a
/// restore would ask about one of them.
pub(super) fn to_choose(preview: Option<&Preview>) -> Option<&[MappingEntry]> {
    let Some(Ok(Some(baseline))) = preview else {
        return None;
    };
    let entries = &baseline.newest.mapped.as_ref()?.entries;
    (unknown(entries) > 0).then_some(entries.as_slice())
}

/// How many of `entries` have no known place.
pub(super) fn unknown(entries: &[MappingEntry]) -> usize {
    (entries.iter())
        .filter(|entry| entry.confidence == Confidence::Unknown)
        .count()
}

/// `next` as the sentence its header shows on hover.
pub(super) fn sentence(next: &Next<'_>) -> String {
    let since = |newest: &SnapshotInfo| (newest.host.clone(), format_time(newest.time));
    match *next {
        Next::NoBackupYet => "No backup yet".to_owned(),
        Next::Failed(err) => format!("Could not compare: {err}"),
        Next::Same { newest, blocker } => {
            let (host, date) = since(newest);
            let same = format!("No changes since the backup from {host}, {date}");
            match blocker {
                None => format!("{same}; no backup will be made"),
                Some(NoSkip::NoHashList) => {
                    format!("{same}, but a backup will be made, as it has no list of hashes")
                }
                Some(NoSkip::Registry) => format!(
                    "No changes to the files since the backup from {host}, {date}, but a backup \
                     will be made, as that backup is from the other build and the registry keys \
                     are not compared"
                ),
                Some(NoSkip::Unmapped) => format!(
                    "{same}, but a backup will be made, as some of its files have no place here"
                ),
                Some(_) => format!("{same}, but a backup will be made"),
            }
        }
        Next::UnchangedHere {
            parent,
            newest,
            counts,
        } => {
            let (host, date) = since(newest);
            format!(
                "Unchanged since this PC's last backup ({}), so no backup will be made. The newest \
                 backup, from {host} on {date}, differs: {}",
                format_time(parent.time),
                counted(&counts)
            )
        }
        Next::Changes {
            newest,
            counts,
            hashed,
        } => {
            let (host, date) = since(newest);
            let text = format!("{} since the backup from {host}, {date}", counted(&counts));
            if hashed || counts.changed == 0 {
                text
            } else {
                format!("{text} ({BY_MTIME})")
            }
        }
    }
}

/// What a game's header shows of its next backup, each with [`preview_text`] for its hover.
pub(super) enum Header {
    /// Nothing: no preview yet, the game would not be backed up, or the preview failed, which
    /// [`files_ui`] says below.
    Nothing,
    /// How many files are new, changed, removed or could not be compared.
    Chips(Counts, String),
    /// One icon of [`Next::glyph`], `muted` when the same files are backed up again.
    Mark {
        glyph: &'static str,
        muted: bool,
        text: String,
    },
}

/// The [`Header`] of `game` by `preview` and `settings`. [`next_backup`] keys and clones every
/// file of the game, so [`Listed`] keeps it rather than each frame working it out.
pub(super) fn header_of(
    game: &GameScan,
    preview: Option<&Preview>,
    settings: &GameSettings,
) -> Header {
    let Some(preview) = preview else {
        return Header::Nothing;
    };
    let Some(next) = next_backup(game, preview, settings) else {
        return Header::Nothing;
    };
    let text = noted(sentence(&next), preview);
    if let Next::Changes { counts, .. } = next {
        Header::Chips(counts, text)
    } else if let Some(glyph) = next.glyph() {
        // The same files backed up again is no news, so it is muted.
        let muted = matches!(
            next,
            Next::Same {
                blocker: Some(_),
                ..
            }
        );
        Header::Mark { glyph, muted, text }
    } else {
        Header::Nothing
    }
}

/// What the next backup would do to a game, in its header, by its `header`.
pub(super) fn preview_ui(ui: &mut Ui, header: &Header) {
    match header {
        Header::Nothing => {}
        Header::Chips(counts, text) => {
            let chips = [
                (Badge::New, counts.new),
                (Badge::Changed, counts.changed),
                (Badge::Removed, counts.removed),
                (Badge::Unknown, counts.unknown),
            ]
            .map(|(badge, count)| (badge.glyph(), badge.color(ui), count));
            theme::chips(ui, &chips, text);
        }
        Header::Mark { glyph, muted, text } => {
            let color = if *muted {
                ui.visuals().weak_text_color()
            } else {
                theme::palette(ui).accent
            };
            theme::labeled_mark(ui, glyph, color, text);
        }
    }
}

/// A game of the report as the list shows it: its file tree and its header, kept until
/// [`Listed::invalidate`].
pub(super) struct Listed {
    pub(super) tree: FileTree,
    /// `None` until the next frame draws, after [`Listed::invalidate`].
    pub(super) header: Option<Header>,
}

impl Listed {
    /// `game`, with its tree by `preview`.
    pub(super) fn new(game: &GameScan, preview: Option<&Preview>) -> Self {
        Listed {
            tree: file_tree(game, preview),
            header: None,
        }
    }

    /// Forgets the tree's cached counts and the header. Call it whenever the game's settings
    /// change.
    pub(super) fn invalidate(&mut self) {
        self.tree.invalidate();
        self.header = None;
    }
}

/// Whether the files of `game` that `settings` include are exactly `files` (a backup's, by path
/// key) by size and mtime, so rustic would find the game unchanged (ADR-0010).
fn unchanged_since(game: &GameScan, settings: &GameSettings, files: &BackedUpFiles) -> bool {
    let mut included = 0;
    game.files.iter().all(|file| {
        let key = selection::path_key(file);
        if !settings.path_included(&key) {
            return true;
        }
        included += 1;
        files.get(&key) == Some(&(file.size, file.modified))
    }) && included == files.len()
}

/// What the user clicked in a game's file tree besides a checkbox.
pub(super) enum FileAction {
    /// A folder's open button: the folder on disk.
    Open(PathBuf),
    /// The Details button of the game's file at this position of its scan.
    Details(usize),
}

/// What the scan found for one game, with its files as a checkbox tree and its registry keys
/// folded away. Returns the change the user made this frame; a folder to open or a file's
/// details to show goes to `action`.
pub(super) fn files_ui(
    ui: &mut Ui,
    game: &GameScan,
    tree: &mut FileTree,
    preview: Option<&Preview>,
    settings: &GameSettings,
    enabled: bool,
    action: &mut Option<FileAction>,
) -> Option<Change> {
    let included = |key: &str| settings.path_included(key);
    let badge = badges(game, preview, settings);
    let summary = summary(game, tree.included(&included, &badge), settings);
    // An error stays in sight rather than only on hover; the other previews are in the header.
    if let Some(preview @ Err(_)) = preview
        && let Some(text) = preview_text(game, preview, settings)
    {
        ui.weak(text);
    }
    if game.files.is_empty() && game.registry_source.is_none() {
        ui.weak(summary);
        return None;
    }
    let summary = RichText::new(summary).color(ui.visuals().weak_text_color());
    egui::CollapsingHeader::new(summary)
        .id_salt(&game.title)
        .show(ui, |ui| {
            let mode = Mode::Select {
                included: &included,
                enabled,
            };
            let hint = |_: usize, key: &str, badge| hint(preview, key, badge);
            let by_row = |_: usize, key: &str| badge(key);
            // Rows of found files come first in the tree, in the scan's order.
            let mut row = |ui: &mut Ui, index: usize, key: &str, slot: Slot<'_>| {
                if let Slot::End(_) = slot
                    && index < game.files.len()
                    && by_row(index, key) == Some(Badge::Changed)
                {
                    let name = format!("Details of {}", key.rsplit('/').next().unwrap_or(key));
                    // Small, as the restore plan's Override button, to keep the row's height.
                    let button = ui.add(egui::Button::new("Details").small());
                    button.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
                    if button.clicked() {
                        *action = Some(FileAction::Details(index));
                    }
                }
            };
            let clicked = ui.push_id(&game.title, |ui| {
                tree.ui_with_rows(ui, mode, &by_row, &hint, &mut row)
            });
            let mut change = match clicked.inner {
                Some(Clicked::Toggle(node, included)) => Some(Change::Path { node, included }),
                Some(Clicked::Open(path)) => {
                    *action = Some(FileAction::Open(path));
                    None
                }
                None => None,
            };
            if game.registry_source.is_some() {
                ui.weak("Registry");
                for key in &game.registry_keys {
                    let mut included = settings.registry_included(key);
                    if ui
                        .add_enabled(enabled, Checkbox::new(&mut included, key))
                        .changed()
                    {
                        change = Some(Change::Registry {
                            key: key.clone(),
                            included,
                        });
                    }
                }
            }
            change
        })
        .body_returned
        .flatten()
}

impl BackupTab {
    /// Compares the shown report with the last backups when the repository is open, through
    /// the config's folder pairs for a backup of the other build. Replacing the job drops the
    /// comparison of an older report. Does nothing before the first scan.
    pub(crate) fn start_preview(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config), Some(report)) =
            (&state.session, &state.config, &self.report)
        else {
            return;
        };
        let (storage, games) = (session.storage.clone(), report.games.clone());
        let (manifest, config) = (state.manifest_or_empty(), config.clone());
        let environment = state.env.environment.clone();
        self.compared_in = Some(storage.clone());
        // A fresh slot, so a replaced job's listing never lands.
        self.listed_stats = Arc::default();
        let (listed_stats, waker) = (self.listed_stats.clone(), ctx.clone());
        self.previewing = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            let mapper = Mapper::new(&manifest, &config, &environment);
            preview(&storage, &games, &mapper, |snapshots| {
                *lock(&listed_stats) = Some(stats::by_game(snapshots));
                waker.request_repaint();
            })
        }));
    }

    /// The mapping dialog of a game whose newest backup is of the other build. Its choices are
    /// saved as after a restore (ADR-0011), then the comparison runs again (ADR-0021).
    pub(super) fn mapping_ui(&mut self, ctx: &egui::Context, state: &mut State) {
        let Some((title, entries, dialog)) = &mut self.mapping else {
            return;
        };
        // Not dismissible: Escape in a target field or a click closing its candidates list would
        // lose the typed targets.
        let action = theme::modal(ctx, "backup_mapping", 720.0, |ui| {
            dialog.ui(ui, title, false, false, false, None)
        });
        match action {
            Some(DialogAction::Confirm(choices)) => match crossbuild::resolve(entries, &choices) {
                Ok(_) => {
                    state.apply_mapping_choices(ctx, title, &choices);
                    self.mapping = None;
                    self.start_preview(ctx, state);
                }
                Err(ResolveError::Unclassifiable { root, template }) => {
                    dialog.reject(&root, &template);
                }
                // The dialog confirms only once every root has a choice.
                Err(_) => {}
            },
            Some(DialogAction::Cancel) => self.mapping = None,
            Some(DialogAction::UsePrefix | DialogAction::SkipGame) | None => {}
        }
    }
}
