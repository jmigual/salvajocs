use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use eframe::egui::{self, Checkbox, RichText, Role, Ui, WidgetInfo};
use salvajocs_core::changes::{self, Comparison, Counts, FileState, Mapper, NoSkip};
use salvajocs_core::crossbuild::{self, Confidence, MappingEntry, ResolveError};
use salvajocs_core::scan::{Environment, GameScan};
use salvajocs_core::selection::{self, Change, GameSettings};
use salvajocs_core::slug::slug;
use salvajocs_core::stats;
use salvajocs_core::storage::SnapshotInfo;
use salvajocs_core::sync::{self, Saves};

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
    /// This PC's base for the game (ADR-0035); `None` when it has none.
    pub(super) base: Option<sync::Base>,
    /// When the base's snapshot was made; `None` when the repository no longer lists it.
    pub(super) base_time: Option<SystemTime>,
    /// The base's snapshot, read only when [`sync::Base::to_read`] names it.
    pub(super) base_backup: Option<changes::Baseline>,
}

impl Baseline {
    /// Whether the newest backup has a hash list that matches its files; without one, changes
    /// are judged by mtime and a backup is made even when nothing differs (ADR-0018).
    fn hashed(&self) -> bool {
        self.newest.compared().1.is_some()
    }
}

/// One game's preview: `None` when the repository has no backup of it, `Err` when it could not
/// be compared.
pub(super) type Preview = Result<Option<Baseline>, String>;

/// Lists each of `games`' newest backup and compares the found files with it (ADR-0018), through
/// `mapper` when it is of the other build (ADR-0021). Also reads this PC's bases for the
/// repository `scope` from `config_dir` (ADR-0035). Hands the repository's snapshots to `listed`
/// before comparing. Locks the repository for one game at a time.
pub(super) fn preview(
    storage: &SharedStorage,
    games: &[GameScan],
    mapper: &Mapper,
    (config_dir, scope): (&Path, &str),
    listed: impl FnOnce(&[SnapshotInfo]),
) -> anyhow::Result<Vec<Preview>> {
    let (snapshots, host) = {
        let mut storage = lock_storage(storage)?;
        storage.reindex()?;
        (storage.snapshots("salvajocs")?, storage.hostname()?)
    };
    listed(&snapshots);
    let bases = sync::load(config_dir);
    let previews = games.iter().map(|game| {
        let base = bases.base(scope, &slug(&game.title), &snapshots, &host);
        preview_game(storage, game, &snapshots, base, mapper).map_err(|err| format!("{err:#}"))
    });
    Ok(previews.collect())
}

fn preview_game(
    storage: &SharedStorage,
    game: &GameScan,
    snapshots: &[SnapshotInfo],
    base: Option<Cow<'_, sync::Base>>,
    mapper: &Mapper,
) -> anyhow::Result<Option<Baseline>> {
    let Some(newest) = changes::newest(snapshots, &slug(&game.title)) else {
        return Ok(None);
    };
    let baseline = {
        let mut storage = lock_storage(storage)?;
        mapper.baseline(&mut storage, newest, game)?
    };
    let (listing, hashes) = baseline.compared();
    let comparison = changes::compare_game(game, listing, hashes);
    let states = (game.files.iter().map(selection::path_key))
        .zip(comparison.found.iter().cloned())
        .collect();
    let base = base.map(Cow::into_owned);
    let base_info = (base.as_ref()).and_then(|base| {
        snapshots
            .iter()
            .find(|snapshot| snapshot.id == base.snapshot)
    });
    // The job does not know which files the game's settings tick, so it decides with all of them.
    let settings = GameSettings::default();
    let saves = Saves::Scanned {
        game,
        settings: &settings,
        compared: &comparison,
    };
    let needed = (base.as_ref()).is_some_and(|base| base.to_read(&saves, &baseline).is_some());
    // A base the repository no longer lists is left unread: Back up asks.
    let base_backup = match base_info {
        Some(info) if needed => {
            let mut storage = lock_storage(storage)?;
            Some(mapper.baseline(&mut storage, info, game)?)
        }
        _ => None,
    };
    Ok(Some(Baseline {
        newest: baseline,
        comparison,
        states,
        base_time: base_info.map(|info| info.time),
        base,
        base_backup,
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
pub(super) fn counted(counts: &Counts) -> String {
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
    /// Another PC made the newest backup, which differs by `counts`, and this PC has not changed
    /// the saves since its base: no backup is made, the newest is restored instead (ADR-0035).
    Behind {
        newest: &'a SnapshotInfo,
        counts: Counts,
    },
    /// This PC and another changed the saves since the base, made at `base_time` when the
    /// repository lists it; the newest backup differs by `counts` and Back up asks.
    ChangedOnBoth {
        newest: &'a SnapshotInfo,
        counts: Counts,
        base_time: Option<SystemTime>,
    },
    /// This PC has no base for the game and the newest backup differs by `counts`: Back up asks.
    UnknownBase {
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
    /// The one icon its header shows: a check when the saves are the newest backup's, an equals
    /// sign when the same files are backed up again, a plus when there is no backup yet, a down
    /// arrow when this PC is behind, a warning sign when both changed and a question mark when
    /// the base is unknown. `None` when the header counts the changes, or for a failed preview,
    /// which is text.
    pub(super) fn glyph(&self) -> Option<&'static str> {
        match self {
            Next::NoBackupYet => Some(icons::PLUS_CIRCLE),
            Next::Same {
                blocker: Some(_), ..
            } => Some(icons::EQUALS),
            Next::Same { blocker: None, .. } => Some(icons::CHECK_CIRCLE),
            Next::Behind { .. } => Some(icons::ARROW_DOWN),
            Next::ChangedOnBoth { .. } => Some(icons::WARNING),
            Next::UnknownBase { .. } => Some(icons::QUESTION),
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
    let blocker = if counts == Counts::default() {
        // The scan the backup stores, whose registry keys may all be unticked.
        let applied = selection::apply(game, settings)?;
        let blocker = baseline.newest.skip_blocker(&applied);
        if blocker.is_none() {
            return Some(Next::Same {
                newest,
                blocker: None,
            });
        }
        blocker
    } else {
        None
    };
    let saves = Saves::Scanned {
        game,
        settings,
        compared: &baseline.comparison,
    };
    let (base, base_backup) = (baseline.base.as_ref(), baseline.base_backup.as_ref());
    Some(
        match sync::state(&saves, Some(&baseline.newest), base, base_backup) {
            sync::State::Behind => Next::Behind { newest, counts },
            sync::State::ChangedOnBoth => Next::ChangedOnBoth {
                newest,
                counts,
                base_time: baseline.base_time,
            },
            sync::State::UnknownBase => Next::UnknownBase { newest, counts },
            // Ahead, which is neither up to date nor missing here.
            _ if blocker.is_some() => Next::Same { newest, blocker },
            _ => Next::Changes {
                newest,
                counts,
                hashed: baseline.hashed(),
            },
        },
    )
}

/// How many files the next backup would add, change or remove, for sorting by changes
/// (ADR-0032): all included files, and the registry as one, when there is no backup yet; none
/// when the game is unchanged; the differences from the newest backup when it is behind or asks.
/// `None` when the preview failed or the game would not be backed up at all.
pub(super) fn change_count(
    game: &GameScan,
    preview: &Preview,
    settings: &GameSettings,
) -> Option<usize> {
    match next_backup(game, preview, settings)? {
        Next::NoBackupYet => (selection::apply(game, settings))
            .map(|kept| kept.files.len() + usize::from(kept.registry_source.is_some())),
        Next::Failed(_) => None,
        Next::Same { .. } => Some(0),
        Next::Behind { counts, .. }
        | Next::Changes { counts, .. }
        | Next::ChangedOnBoth { counts, .. }
        | Next::UnknownBase { counts, .. } => {
            Some(counts.new + counts.changed + counts.removed + counts.unknown)
        }
    }
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
                None => format!(
                    "Up to date with the backup from {host}, {date}; no backup will be made"
                ),
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
        Next::Behind { newest, counts } => {
            let (host, date) = since(newest);
            let differs = counted(&counts);
            let differs = if differs.is_empty() {
                String::new()
            } else {
                format!(": {differs}")
            };
            format!(
                "{host} has newer saves, from {date}{differs}. This PC has not changed them \
                 since it last synced, so no backup will be made. To use them, restore them on \
                 the Restore tab. To keep this PC's saves instead, use Back up anyway under the \
                 game."
            )
        }
        Next::ChangedOnBoth {
            newest,
            counts,
            base_time,
        } => {
            let (host, date) = since(newest);
            let synced =
                base_time.map_or(String::new(), |time| format!(" ({})", format_time(time)));
            let differs = counted(&counts);
            let against = if differs.is_empty() {
                format!("no file differs from the backup from {host}, {date}")
            } else {
                format!("{differs} against the backup from {host}, {date}")
            };
            format!(
                "Changed on this PC and on {host} since this PC last synced{synced}: {against}. \
                 Back up asks what to do."
            )
        }
        Next::UnknownBase { newest, counts } => {
            let (host, date) = since(newest);
            let differs = counted(&counts);
            let saves = if differs.is_empty() {
                format!("This PC's files match the newest backup, from {host}, {date}")
            } else {
                format!(
                    "This PC's saves differ from the newest backup, from {host}, {date}: {differs}"
                )
            };
            format!(
                "{saves}. salvajocs has no record of this PC syncing this game. Back up asks what \
                 to do."
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
    /// One icon of [`Next::glyph`], in its `tone`, followed by the `counts` of differences when
    /// there are any.
    Mark {
        glyph: &'static str,
        tone: Tone,
        counts: Counts,
        text: String,
    },
}

/// The color of a [`Header::Mark`].
#[derive(Clone, Copy)]
pub(super) enum Tone {
    /// The brand green.
    Accent,
    /// Weak: the same files are backed up again, which is no news.
    Muted,
    /// The warning color: this PC is behind, or Back up asks.
    Warn,
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
        let (tone, counts) = match next {
            Next::Same {
                blocker: Some(_), ..
            } => (Tone::Muted, Counts::default()),
            Next::Behind { counts, .. }
            | Next::ChangedOnBoth { counts, .. }
            | Next::UnknownBase { counts, .. } => (Tone::Warn, counts),
            _ => (Tone::Accent, Counts::default()),
        };
        Header::Mark {
            glyph,
            tone,
            counts,
            text,
        }
    } else {
        Header::Nothing
    }
}

impl Header {
    /// Whether Back up would not store the game (behind, changed on both PCs or of unknown
    /// base), so **Back up anyway** is offered for it.
    pub(super) fn offers_anyway(&self) -> bool {
        matches!(
            self,
            Header::Mark {
                tone: Tone::Warn,
                ..
            }
        )
    }
}

/// What the next backup would do to a game, in its header, by its `header`.
pub(super) fn preview_ui(ui: &mut Ui, header: &Header) {
    match header {
        Header::Nothing => {}
        Header::Chips(counts, text) => chips(ui, counts, text),
        Header::Mark {
            glyph,
            tone,
            counts,
            text,
        } => {
            let color = match tone {
                Tone::Accent => theme::palette(ui).accent,
                Tone::Muted => ui.visuals().weak_text_color(),
                Tone::Warn => ui.visuals().warn_fg_color,
            };
            theme::labeled_mark(ui, glyph, color, text);
            if *counts != Counts::default() {
                // Named by the counts alone: the mark already carries the sentence.
                chips(ui, counts, &counted(counts));
            }
        }
    }
}

/// The chips of `counts`, with `text` on hover.
fn chips(ui: &mut Ui, counts: &Counts, text: &str) {
    let chips = [
        (Badge::New, counts.new),
        (Badge::Changed, counts.changed),
        (Badge::Removed, counts.removed),
        (Badge::Unknown, counts.unknown),
    ]
    .map(|(badge, count)| (badge.glyph(), badge.color(ui), count));
    theme::chips(ui, &chips, text);
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
        let (config_dir, scope) = (state.env.config_dir.clone(), config.secret_scope());
        let environment = state.env.environment.clone();
        self.compared_in = Some(storage.clone());
        // A fresh slot, so a replaced job's listing never lands.
        self.listed_stats = Arc::default();
        let (listed_stats, waker) = (self.listed_stats.clone(), ctx.clone());
        self.previewing = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            let mapper = Mapper::new(&manifest, &config, &environment);
            preview(
                &storage,
                &games,
                &mapper,
                (&config_dir, &scope),
                |snapshots| {
                    *lock(&listed_stats) = Some(stats::by_game(snapshots));
                    waker.request_repaint();
                },
            )
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
