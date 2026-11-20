//! The Backup tab: scan this machine for games, choose which to back up, see what differs from
//! the last backups, and what each backup and the cleanup after it did (ADR-0004, ADR-0010,
//! ADR-0015, ADR-0018).

use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use eframe::egui::emath::Rot2;
use eframe::egui::{
    self, Checkbox, Label, Rect, RichText, Role, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo,
};
use salvajocs_core::backup::{self, GameBackup, Stored};
use salvajocs_core::changes::{self, Comparison, Counts, FileState, Mapper, NoSkip};
use salvajocs_core::config::{AppConfig, PasswordMode};
use salvajocs_core::crossbuild::{self, Confidence, MappingEntry, ResolveError};
use salvajocs_core::layout::HASHES_FILE;
use salvajocs_core::retention::{self, Cleanup, Retention};
use salvajocs_core::scan::{self, Environment, GameScan, ProblemKind, ScanProblem, ScanReport};
use salvajocs_core::selection::{self, Change, GameSettings, Pending};
use salvajocs_core::slug::slug;
use salvajocs_core::storage::SnapshotInfo;

use crate::job::Job;
use crate::state::{SharedStorage, State, lock_storage};
use crate::status_bar;
use crate::tabs::details::Details;
use crate::tabs::mapping::{Dialog, DialogAction};
use crate::tabs::tree::{Badge, Clicked, FileTree, Mode, Row, Slot};
use crate::tabs::{count, format_size, format_time};
use crate::theme::{self, icons};
use choices::{AfterBackup, Choices, Games};

mod choices;

/// What a backup did for one game.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// A new snapshot was stored.
    Saved,
    /// Nothing changed since this machine's last snapshot.
    Unchanged,
    /// Nothing changed since this machine's last snapshot, but the newest one, from the host it
    /// names, differs.
    UnchangedNewerElsewhere(String),
    /// The saves equal the newest snapshot, from the host it names.
    SameAsNewest(String),
    /// The game could not be backed up; holds why.
    Failed(String),
}

/// One game's line in the results.
#[derive(Debug, PartialEq, Eq)]
struct GameResult {
    title: String,
    outcome: Outcome,
}

/// `error` and its sources, joined.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text = format!("{text}: {next}");
        source = next.source();
    }
    text
}

fn summarize(backup: &GameBackup) -> GameResult {
    let outcome = match &backup.result {
        Ok(Stored::New(_)) => Outcome::Saved,
        Ok(Stored::Unchanged {
            newer_elsewhere: None,
        }) => Outcome::Unchanged,
        Ok(Stored::Unchanged {
            newer_elsewhere: Some(newest),
        }) => Outcome::UnchangedNewerElsewhere(newest.host.clone()),
        Ok(Stored::SameAsNewest(newest)) => Outcome::SameAsNewest(newest.host.clone()),
        Ok(other) => Outcome::Failed(format!("unexpected backup result: {other:?}")),
        Err(err) => Outcome::Failed(chain(err)),
    };
    GameResult {
        title: backup.title.clone(),
        outcome,
    }
}

/// What the scan found for a game: the count and size of the files backed up, given as
/// `included`, how many are excluded, whether registry keys are backed up, and unreadable paths.
fn summary(game: &GameScan, (files, size): (usize, u64), settings: &GameSettings) -> String {
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

fn describe_problem(problem: &ScanProblem) -> String {
    let path = problem.path.display();
    match &problem.kind {
        ProblemKind::AmbiguousPrefix(users) => format!(
            "{path}: the Wine prefix has several users ({}) and none is steamuser",
            users.join(", ")
        ),
        ProblemKind::NoUser => format!("{path}: the Wine prefix has no user yet"),
        ProblemKind::NotAPrefix => format!("{path}: not a Wine prefix"),
        ProblemKind::Unreadable(error) => format!("{path}: cannot be read ({error})"),
        _ => format!("{path}: cannot be scanned"),
    }
}

/// What a game's files are compared with: its newest backup from any host (ADR-0018), mapped
/// when it is of the other build (ADR-0021).
struct Baseline {
    newest: changes::Baseline,
    comparison: Comparison,
    /// The state of each found file, by its path key.
    states: HashMap<String, FileState>,
    /// This machine's last backup of the game and its files; `None` when it has none or it has
    /// no hash list, which staging now adds, so rustic finds no backup unchanged.
    parent: Option<(SnapshotInfo, BackedUpFiles)>,
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
type Preview = Result<Option<Baseline>, String>;

/// Lists each of `games`' newest backup and this machine's last one, and compares the found
/// files with the newest (ADR-0018), through `mapper` when it is of the other build (ADR-0021).
/// Locks the repository for one game at a time.
fn preview(
    storage: &SharedStorage,
    games: &[GameScan],
    mapper: &Mapper,
) -> anyhow::Result<Vec<Preview>> {
    let (snapshots, host) = {
        let mut storage = lock_storage(storage)?;
        storage.reindex()?;
        (storage.snapshots("salvajocs")?, storage.hostname()?)
    };
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
fn badges<'a>(
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
fn hint(preview: Option<&Preview>, key: &str, badge: Badge) -> String {
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
enum Next<'a> {
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
    fn glyph(&self) -> Option<&'static str> {
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
fn next_backup<'a>(
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
fn preview_text(game: &GameScan, preview: &Preview, settings: &GameSettings) -> Option<String> {
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
fn to_choose(preview: Option<&Preview>) -> Option<&[MappingEntry]> {
    let Some(Ok(Some(baseline))) = preview else {
        return None;
    };
    let entries = &baseline.newest.mapped.as_ref()?.entries;
    (unknown(entries) > 0).then_some(entries.as_slice())
}

/// How many of `entries` have no known place.
fn unknown(entries: &[MappingEntry]) -> usize {
    (entries.iter())
        .filter(|entry| entry.confidence == Confidence::Unknown)
        .count()
}

/// `next` as the sentence its header shows on hover.
fn sentence(next: &Next<'_>) -> String {
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
enum Header {
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
fn header_of(game: &GameScan, preview: Option<&Preview>, settings: &GameSettings) -> Header {
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
fn preview_ui(ui: &mut Ui, header: &Header) {
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
struct Listed {
    tree: FileTree,
    /// `None` until the next frame draws, after [`Listed::invalidate`].
    header: Option<Header>,
}

impl Listed {
    /// `game`, with its tree by `preview`.
    fn new(game: &GameScan, preview: Option<&Preview>) -> Self {
        Listed {
            tree: file_tree(game, preview),
            header: None,
        }
    }

    /// Forgets the tree's cached counts and the header. Call it whenever the game's settings
    /// change.
    fn invalidate(&mut self) {
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

/// How far the running backup is, shared with its job.
#[derive(Default)]
struct Progress {
    /// The games the backup goes through, known once its scan is done.
    total: usize,
    /// The outcome of each game finished so far.
    done: Vec<GameResult>,
    /// Every game is done and old backups are being removed.
    cleaning: bool,
}

/// What saving the selection before a backup ran into.
#[derive(Default)]
struct SaveProblems {
    /// The games whose changes could not be saved, with why; their changes stay pending.
    failed: Vec<String>,
    /// Problems that lost no change.
    warnings: Vec<String>,
}

/// What the cleanup after a backup did.
struct Cleaned {
    /// Why the retention policy could not be read, so nothing was forgotten.
    policy_error: Option<String>,
    /// What the cleanup did, up to `error` if it stopped.
    done: Cleanup,
    /// Why the cleanup stopped, if it did.
    error: Option<String>,
}

/// What a finished backup job hands back.
struct Backed {
    /// The scan the backup went by, which replaces the shown list.
    report: ScanReport,
    /// Why the run stopped early, if it did.
    error: Option<String>,
    /// What saving the selection ran into.
    saving: SaveProblems,
    /// What the backup left of the selection.
    after: AfterBackup,
    /// The cleanup after the backup, `None` when the run stopped early.
    cleanup: Option<Cleaned>,
}

/// The state of the Backup tab.
#[derive(Default)]
pub struct BackupTab {
    scanning: Option<Job<ScanReport>>,
    report: Option<ScanReport>,
    scan_error: Option<String>,
    /// Each game of `report`, in its order.
    listed: Vec<Listed>,
    /// Compares each game of `report` with its last backups; not counted as busy, since it only
    /// reads.
    previewing: Option<Job<Vec<Preview>>>,
    /// The preview of each game of `report`, in its order; empty until it is known.
    previews: Vec<Preview>,
    /// The repository `previewing` and `previews` compare with.
    compared_in: Option<SharedStorage>,
    /// The `Wine prefix: …` lines of each game of `report`, in its order.
    prefixes: Vec<Vec<String>>,
    /// What to back up of each game.
    choices: Choices,
    running: Option<Job<Backed>>,
    progress: Arc<Mutex<Progress>>,
    /// The games the running scan, or the backup's, has checked and its total; 0 before it starts.
    checked: Arc<Mutex<(usize, usize)>>,
    run_error: Option<String>,
    saving: SaveProblems,
    cleanup: Option<Cleaned>,
    /// A cleanup forgot snapshots since [`BackupTab::take_removed`] last ran.
    removed: bool,
    /// Back up was clicked; the backup starts once the pending file is written.
    backup_requested: bool,
    /// The open mapping dialog: the game's title and the roots it shows.
    mapping: Option<(String, Vec<MappingEntry>, Dialog)>,
    /// The open details of a changed file; it only reads, so it is not counted as busy.
    details: Option<Details>,
}

impl BackupTab {
    /// Draws the tab. The scan and the backup run in jobs; `state` only gains notices.
    /// `manifest_loading` is whether the manifest job is still running.
    pub fn ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        self.poll(ui.ctx(), state);
        theme::page(ui, |ui| {
            if state.config.is_none() {
                ui.label("Choose a repository first.");
                return;
            }
            if self.report.is_some() {
                self.games_ui(ui, state, manifest_loading);
            } else {
                self.empty_ui(ui, state, manifest_loading);
            }
            if let Some(error) = &self.scan_error {
                let color = ui.visuals().error_fg_color;
                theme::status(
                    ui,
                    icons::X_CIRCLE,
                    color,
                    format!("The scan failed: {error}"),
                );
            }
            self.results_ui(ui);
        });
        self.mapping_ui(ui.ctx(), state);
        if (self.details.as_ref()).is_some_and(|details| details.ui(ui.ctx())) {
            self.details = None;
        }
    }

    /// Collects the tab's finished jobs and starts what waits on them. The app calls it every
    /// frame, whichever tab is in front, so a pending file write or a load never stalls.
    pub(crate) fn poll(&mut self, ctx: &egui::Context, state: &State) {
        if let Some(job) = &mut self.scanning
            && let Some(result) = job.take_result()
        {
            self.scanning = None;
            match result {
                Ok(report) => {
                    self.adopt(report);
                    self.start_preview(ctx, state);
                    self.scan_error = None;
                }
                Err(err) => self.scan_error = Some(format!("{err:#}")),
            }
        }
        if let Some(job) = &mut self.running
            && let Some(result) = job.take_result()
        {
            self.running = None;
            match result {
                Ok(Backed {
                    report,
                    error,
                    saving,
                    after,
                    cleanup,
                }) => {
                    self.adopt(report);
                    self.start_preview(ctx, state);
                    self.run_error = error;
                    self.saving = saving;
                    self.removed |= cleanup.as_ref().is_some_and(|c| c.done.forgotten > 0);
                    self.cleanup = cleanup;
                    if self.choices.backed_up(after) {
                        self.listed.iter_mut().for_each(Listed::invalidate);
                    }
                }
                Err(err) => self.run_error = Some(format!("{err:#}")),
            }
        }
        // A comparison with a repository Settings replaced is dropped and made again.
        let current = state.session.as_ref().map(|session| &session.storage);
        if (self.compared_in.as_ref())
            .is_some_and(|compared| !current.is_some_and(|current| Arc::ptr_eq(compared, current)))
        {
            self.compared_in = None;
            self.previewing = None;
            self.details = None;
            self.previews = Vec::new();
            if let Some(report) = &self.report {
                self.listed = (report.games.iter())
                    .map(|game| Listed::new(game, None))
                    .collect();
            }
            self.start_preview(ctx, state);
        }
        if let Some(job) = &mut self.previewing
            && let Some(result) = job.take_result()
        {
            self.previewing = None;
            let games = self.report.as_ref().map_or(0, |report| report.games.len());
            self.previews = match result {
                Ok(previews) => previews,
                Err(err) => (0..games).map(|_| Err(format!("{err:#}"))).collect(),
            };
            if let Some(report) = &self.report {
                let games = report.games.iter().zip(&self.previews);
                self.listed = games
                    .map(|(game, preview)| Listed::new(game, Some(preview)))
                    .collect();
            }
        }
        if let Some(details) = &mut self.details {
            details.poll();
        }
        if self.choices.poll(ctx, state) {
            self.listed.iter_mut().for_each(Listed::invalidate);
        }
        self.start_requested(ctx, state);
    }

    /// Whether a finished cleanup forgot snapshots since the last call, so a loaded list of
    /// backups may name one that is gone. A flag rather than `poll`'s result, since `ui` polls
    /// too and would swallow it.
    pub(crate) fn take_removed(&mut self) -> bool {
        std::mem::take(&mut self.removed)
    }

    /// Compares the shown report with the last backups when the repository is open, through
    /// the config's folder pairs for a backup of the other build. Replacing the job drops the
    /// comparison of an older report.
    fn start_preview(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config), Some(report)) =
            (&state.session, &state.config, &self.report)
        else {
            return;
        };
        let (storage, games) = (session.storage.clone(), report.games.clone());
        let (manifest, config) = (state.manifest_or_empty(), config.clone());
        let environment = state.env.environment.clone();
        self.compared_in = Some(storage.clone());
        self.previewing = Some(Job::spawn(ctx, move || {
            let environment = environment.unwrap_or_else(Environment::detect);
            let mapper = Mapper::new(&manifest, &config, &environment);
            preview(&storage, &games, &mapper)
        }));
    }

    /// Shows `report`, with a file tree per game and no preview yet.
    fn adopt(&mut self, report: ScanReport) {
        self.listed = (report.games.iter())
            .map(|game| Listed::new(game, None))
            .collect();
        self.previews = Vec::new();
        self.previewing = None;
        self.details = None;
        self.prefixes = report
            .games
            .iter()
            .map(|game| {
                let line = |prefix: &PathBuf| format!("Wine prefix: {}", prefix.display());
                game.prefixes.iter().map(line).collect()
            })
            .collect();
        self.report = Some(report);
    }

    /// Whether a scan, backup or other job of this tab is running or waits to.
    pub(crate) fn busy(&self) -> bool {
        self.scanning.is_some()
            || self.running.is_some()
            || self.backup_requested
            || self.choices.busy()
    }

    /// Whether a backup runs, which closing the app would cut short.
    pub(crate) fn writing(&self) -> bool {
        self.running.is_some()
    }

    /// Finishes the pending file writes; for when the app closes.
    pub(crate) fn flush(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        self.choices.flush(config_dir)
    }

    /// How many games of the report a backup would store: those its settings leave something
    /// of, as [`selection::apply`] decides, and those whose settings cannot be read. Counts
    /// files from each tree's cached totals.
    fn chosen(&mut self) -> usize {
        let Some(report) = &self.report else {
            return 0;
        };
        let choices = &self.choices;
        let games = report.games.iter().zip(&mut self.listed);
        let previews = &self.previews;
        games
            .enumerate()
            .map(|(index, (game, listed))| match choices.game(&game.title) {
                Ok(settings) => {
                    let badge = badges(game, previews.get(index), &settings);
                    let (files, _) = listed
                        .tree
                        .included(&|key| settings.path_included(key), &badge);
                    let keys = &game.registry_keys;
                    !settings.skip
                        && (files > 0
                            || game.registry_source.is_some()
                                && keys.iter().any(|key| settings.registry_included(key)))
                }
                Err(_) => true,
            })
            .filter(|&chosen| chosen)
            .count()
    }

    /// Before the first scan: what the tab is for and the scan as its one action.
    fn empty_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        ui.vertical_centered(|ui| {
            ui.add_space(48.0);
            let muted = ui.visuals().weak_text_color();
            theme::mark(ui, icons::GAME_CONTROLLER, 40.0, muted);
            ui.heading("Find your saves");
            ui.weak(
                "Scan this machine for the games in the ludusavi manifest and your custom games.",
            );
            ui.add_space(8.0);
            self.scan_ui(ui, state, manifest_loading, true);
            self.scan_notes_ui(ui, state, manifest_loading);
        });
    }

    /// The scan button: `primary` in the empty state, the one framed button of the list header
    /// otherwise.
    fn scan_ui(&mut self, ui: &mut Ui, state: &State, manifest_loading: bool, primary: bool) {
        let enabled = !manifest_loading && self.scanning.is_none() && self.running.is_none();
        let glyph = icons::MAGNIFYING_GLASS;
        let button = if primary {
            theme::primary(ui, enabled, Some(glyph), "Scan for games")
        } else {
            let button = egui::Button::new((theme::icon(glyph), "Scan for games"));
            ui.add_enabled(enabled, button)
        };
        if let (true, Some(config)) = (button.clicked(), &state.config) {
            let (manifest, config) = (state.manifest_or_empty(), config.clone());
            let environment = state.env.environment.clone();
            self.scan_error = None;
            self.checked = Arc::default();
            let progress = scan_progress(&self.checked, ui.ctx());
            self.scanning = Some(Job::spawn(ui.ctx(), move || {
                let environment = environment.unwrap_or_else(Environment::detect);
                Ok(scan::scan(&manifest, &config, &environment, progress))
            }));
        }
    }

    /// What limits the scan, and whether one runs.
    fn scan_notes_ui(&self, ui: &mut Ui, state: &State, manifest_loading: bool) {
        if manifest_loading {
            ui.weak("The game manifest is loading.");
        } else if state.manifest.is_none() {
            ui.weak("The game manifest is not available; only custom games are scanned.");
        }
        if self.scanning.is_some() {
            let (done, total) = *lock(&self.checked);
            if total == 0 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning");
                });
            } else {
                let text = format!("Checked {done} of {}", count(total, "game"));
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(text));
            }
        }
    }

    /// The scan report: its problems, a header with the count and the selection buttons, and the
    /// games as a boxed list.
    fn games_ui(&mut self, ui: &mut Ui, state: &mut State, manifest_loading: bool) {
        let Some(report) = &self.report else {
            return;
        };
        let games = report.games.len();
        let editable = self.choices.editable() && self.running.is_none();
        let mut select = None;
        theme::row(
            ui,
            |ui| {
                ui.label(format!("{} found", count(games, "game")));
            },
            |ui| {
                // Frameless until hovered, so the scan reads as the header's main action.
                let quiet = |text| egui::Button::new(text).frame_when_inactive(false);
                if ui.add_enabled(editable, quiet("Select all")).clicked() {
                    select = Some(true);
                }
                if ui.add_enabled(editable, quiet("Deselect all")).clicked() {
                    select = Some(false);
                }
                self.scan_ui(ui, state, manifest_loading, false);
            },
        );
        self.scan_notes_ui(ui, state, manifest_loading);
        if self.previewing.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Comparing with the last backups");
            });
        }
        self.choices.status_ui(ui, state);
        let Some(report) = &self.report else {
            return;
        };
        // By index into `report.games`, applied after drawing, like the folder to open.
        let mut changes = Vec::new();
        // The game's index and what was clicked in its tree.
        let mut clicked: Option<(usize, FileAction)> = None;
        let mut choose = None;
        if let Some(select) = select {
            for (index, game) in report.games.iter().enumerate() {
                if self
                    .choices
                    .game(&game.title)
                    .is_ok_and(|s| s.skip == select)
                {
                    changes.push((index, Change::Skip(!select)));
                }
            }
        }
        if !report.problems.is_empty() {
            let warn = ui.visuals().warn_fg_color;
            theme::status(
                ui,
                icons::WARNING,
                warn,
                format!("Scan problems ({})", report.problems.len()),
            );
            ui.indent("scan_problems", |ui| {
                for problem in &report.problems {
                    ui.colored_label(warn, describe_problem(problem));
                }
            });
        }
        if report.games.is_empty() {
            ui.weak(
                "No saves found. Add the folders where your games live in Settings, or add a \
                 custom game.",
            );
            return;
        }
        let (choices, listed, prefixes) = (&self.choices, &mut self.listed, &self.prefixes);
        let previews = &self.previews;
        theme::group(ui, |ui| {
            // A game's title and its summary read as one row.
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.spacing_mut().interact_size.y = 24.0;
            let games = report.games.iter().zip(listed).zip(prefixes);
            for (index, ((game, listed), prefixes)) in games.enumerate() {
                if index > 0 {
                    theme::row_separator(ui);
                }
                let (settings, error) = match choices.game(&game.title) {
                    Ok(settings) => (settings, None),
                    Err(error) => (Cow::Owned(GameSettings::default()), Some(error)),
                };
                let enabled = editable && error.is_none();
                let preview = previews.get(index);
                let header =
                    (listed.header).get_or_insert_with(|| header_of(game, preview, &settings));
                theme::row(
                    ui,
                    |ui| {
                        let mut checked = !settings.skip;
                        let checkbox = Checkbox::new(&mut checked, &game.title);
                        if ui.add_enabled(enabled, checkbox).changed() {
                            changes.push((index, Change::Skip(!checked)));
                        }
                    },
                    |ui| preview_ui(ui, header),
                );
                // Under the title, without the indent line: the row already groups them.
                ui.scope(|ui| {
                    ui.visuals_mut().indent_has_left_vline = false;
                    ui.indent(&game.title, |ui| {
                        if let Some(error) = error {
                            let color = ui.visuals().error_fg_color;
                            theme::status(
                                ui,
                                icons::X_CIRCLE,
                                color,
                                format!("What to back up cannot be read, so all of it is: {error}"),
                            );
                        }
                        for prefix in prefixes {
                            ui.weak(prefix);
                        }
                        if let Some(entries) = to_choose(preview) {
                            ui.horizontal(|ui| {
                                ui.weak(format!(
                                    "The newest backup has {} from the other build without a \
                                     place here.",
                                    count(unknown(entries), "folder")
                                ));
                                if ui.button("Choose folders…").clicked() {
                                    choose = Some(index);
                                }
                            });
                        }
                        let tree = &mut listed.tree;
                        let mut action = None;
                        let change =
                            files_ui(ui, game, tree, preview, &settings, enabled, &mut action);
                        if let Some(change) = change {
                            changes.push((index, change));
                        }
                        if let Some(action) = action {
                            clicked = Some((index, action));
                        }
                    });
                });
            }
        });
        for (index, change) in changes {
            self.choices
                .change(ui.ctx(), state, &report.games[index].title, change);
            self.listed[index].invalidate();
        }
        match clicked {
            Some((_, FileAction::Open(path))) => {
                if let Err(err) = (state.env.open_folder)(&path) {
                    let path = path.display();
                    state.notices.push(format!("Could not open {path}: {err}"));
                }
            }
            Some((index, FileAction::Details(file))) => {
                let game = &report.games[index];
                let key = selection::path_key(&game.files[file]);
                if let Some(Ok(Some(baseline))) = self.previews.get(index)
                    && let Some(backed_up) = baseline.newest.backed_up(&key)
                    && let Some(session) = &state.session
                {
                    let (ctx, storage) = (ui.ctx(), &session.storage);
                    let local = game.files[file].real.clone();
                    let details =
                        Details::start(ctx, storage, slug(&game.title), key, local, backed_up);
                    self.details = Some(details);
                }
            }
            None => {}
        }
        if let Some(index) = choose
            && let Some(entries) = to_choose(self.previews.get(index))
        {
            let title = report.games[index].title.clone();
            self.mapping = Some((title, entries.to_vec(), Dialog::new(entries.to_vec())));
        }
    }

    /// The mapping dialog of a game whose newest backup is of the other build. Its choices are
    /// saved as after a restore (ADR-0011), then the comparison runs again (ADR-0021).
    fn mapping_ui(&mut self, ctx: &egui::Context, state: &mut State) {
        let Some((title, entries, dialog)) = &mut self.mapping else {
            return;
        };
        let action = theme::modal(ctx, "backup_mapping", 720.0, |ui| {
            dialog.ui(ui, title, false, false, false, None)
        });
        match action {
            Some(DialogAction::Confirm(choices)) => match crossbuild::resolve(entries, &choices) {
                Ok(_) => {
                    if let Some(config) = &mut state.config {
                        crossbuild::apply_choices(config, title, &choices);
                        state.save_config(ctx);
                    }
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

    /// Whether the tab adds the repository and the backup button to the bottom bar. It waits for
    /// a scan report, so the empty state has one call to action.
    pub(crate) fn has_bar(&self, state: &State) -> bool {
        state.config.is_some() && self.report.is_some()
    }

    /// Draws the bottom bar in `bar`, the room [`status_bar::reserve`] took, with the app's
    /// `start` and `end`: then where backups go, or while a backup runs its progress, on the left,
    /// and the backup button last on the right. Call it after the page, when [`Self::has_bar`].
    pub(crate) fn bar_ui(
        &mut self,
        ui: &mut Ui,
        bar: Rect,
        state: &State,
        manifest_loading: bool,
        start: impl FnOnce(&mut Ui),
        mut end: impl FnMut(&mut Ui),
    ) {
        let selected = self.chosen();
        // A pending file write does not hold the button back: the click waits for it.
        let ready = state.session.is_some()
            && self.choices.loaded()
            && selected > 0
            && !manifest_loading
            && self.scanning.is_none()
            && self.running.is_none()
            && !self.backup_requested;
        let label = format!("Back up {}", count(selected, "game"));
        let button = |ui: &mut Ui| theme::primary(ui, ready, Some(icons::FLOPPY_DISK), &label);
        let back_up = status_bar::show(
            ui,
            bar,
            |ui| {
                let status =
                    |ui: &mut Ui| status_bar::status_text(ui, |ui| self.bar_status(ui, state));
                // A running backup's progress goes first, so the manifest status gives way to it.
                if self.running.is_some() {
                    status(ui);
                    start(ui);
                } else {
                    start(ui);
                    status(ui);
                }
            },
            |ui| {
                // `end` gets what the button leaves, so its text gives way and the button stays.
                let width = theme::natural_width(ui, |ui| {
                    button(ui);
                });
                let room = ui.available_width() - width - ui.spacing().item_spacing.x;
                ui.scope(|ui| {
                    ui.set_max_width(room.max(0.0));
                    end(ui);
                });
                button(ui)
            },
        );
        if back_up.clicked() {
            self.backup_requested = true;
            self.start_requested(ui.ctx(), state);
        }
        let progress = lock(&self.progress);
        let (checked, games) = *lock(&self.checked);
        // While the backup's scan runs, the rule shows the scan.
        let (done, total) = if checked < games {
            (checked, games)
        } else {
            (progress.done.len(), progress.total)
        };
        if self.running.is_some() && total > 0 {
            let done = done as f32 / total as f32;
            let rule = Rect::from_min_size(bar.left_top(), Vec2::new(bar.width() * done, 3.0));
            ui.painter()
                .rect_filled(rule, 0.0, theme::palette(ui).accent);
        }
    }

    /// Starts the requested backup once no pending file write runs, so the backup saves every
    /// change and is the file's only writer.
    fn start_requested(&mut self, ctx: &egui::Context, state: &State) {
        if self.backup_requested && !self.choices.busy() && self.running.is_none() {
            self.backup_requested = false;
            self.start_backup(ctx, state);
        }
    }

    /// The tab's part of the bottom bar's left: the repository and whether it is encrypted, or the
    /// running backup's progress.
    fn bar_status(&self, ui: &mut Ui, state: &State) {
        let muted = ui.visuals().weak_text_color();
        if self.running.is_some() {
            autosave_mark(ui);
            let progress = lock(&self.progress);
            let (checked, games) = *lock(&self.checked);
            if progress.cleaning {
                ui.label("Removing old backups");
            } else if checked < games {
                ui.label(format!("Checked {checked} of {}", count(games, "game")));
            } else {
                ui.label(format!(
                    "Backed up {} of {}",
                    progress.done.len(),
                    count(progress.total, "game")
                ));
            }
        } else if let (Some(config), Some(_)) = (&state.config, &state.session) {
            let (glyph, encryption) = match config.password {
                PasswordMode::None => (icons::LOCK_SIMPLE_OPEN, "Not encrypted"),
                PasswordMode::Keyring | PasswordMode::Ask => (icons::LOCK_SIMPLE, "Encrypted"),
                _ => (icons::LOCK_SIMPLE_OPEN, "Encryption unknown"),
            };
            theme::mark(ui, glyph, 16.0, muted)
                .on_hover_text(encryption)
                .widget_info(|| WidgetInfo::labeled(Role::Image, true, encryption));
            ui.add(Label::new(RichText::new(&config.repository).color(muted)).truncate());
        } else {
            ui.weak("The repository is not open.");
        }
    }

    /// Backs up the chosen games in a job, with the staging mirrors under the cache dir
    /// (ADR-0004). The job first saves the pending selection changes to the repository and reads
    /// every game's settings again, so another machine's newer choices count. Then it scans
    /// again, so a save made since the last scan is included, and filters each game by its
    /// settings. Last, it removes old backups by the repository's retention policy.
    fn start_backup(&mut self, ctx: &egui::Context, state: &State) {
        let (Some(session), Some(config)) = (&state.session, &state.config) else {
            return;
        };
        let Some((pending, settings)) = self.choices.for_backup() else {
            return;
        };
        let progress = Arc::new(Mutex::new(Progress {
            total: self.chosen(),
            done: Vec::new(),
            cleaning: false,
        }));
        self.progress = progress.clone();
        self.checked = Arc::default();
        let scanned = scan_progress(&self.checked, ctx);
        self.run_error = None;
        self.saving = SaveProblems::default();
        self.cleanup = None;
        let (manifest, config) = (state.manifest_or_empty(), config.clone());
        let (storage, stage_root, waker) = (
            session.storage.clone(),
            state.env.cache_dir.join("stage"),
            ctx.clone(),
        );
        let config_dir = state.env.config_dir.clone();
        let environment = state.env.environment.clone();
        self.running = Some(Job::spawn(ctx, move || {
            let mut pending = pending;
            let (mut saving, written) = match &mut pending {
                Some(pending) => save_selection(
                    &storage,
                    pending,
                    &config,
                    &settings,
                    &stage_root,
                    &config_dir,
                ),
                None => (SaveProblems::default(), None),
            };
            let reloaded = choices::load(&storage)
                .map(|games| choices::replay(games, pending.as_ref(), &config.secret_scope()));
            let filter = match &reloaded {
                Ok(games) => games,
                Err(err) => {
                    saving.warnings.push(format!(
                        "What to back up could not be read again, so the choices shown are \
                         used: {err:#}"
                    ));
                    &settings
                }
            };
            let environment = environment.unwrap_or_else(Environment::detect);
            let report = scan::scan(&manifest, &config, &environment, scanned);
            let games = chosen_files(&report, filter);
            lock(&progress).total = games.len();
            let mapper = Mapper::new(&manifest, &config, &environment);
            let backed_up = back_up(&storage, &games, &stage_root, &mapper, |backup| {
                lock(&progress).done.push(summarize(backup));
                waker.request_repaint();
            });
            // A failed game still cleans up; a run that stopped early does not.
            let (error, cleanup) = match backed_up {
                Ok(made) => {
                    lock(&progress).cleaning = true;
                    waker.request_repaint();
                    (None, Some(clean_up(&storage, &made)))
                }
                Err(err) => (Some(format!("{err:#}")), None),
            };
            Ok(Backed {
                report,
                error,
                saving,
                cleanup,
                after: AfterBackup {
                    source: Arc::downgrade(&storage),
                    pending,
                    written,
                    games: reloaded.ok(),
                },
            })
        }));
    }

    fn results_ui(&self, ui: &mut Ui) {
        let progress = lock(&self.progress);
        let error_color = ui.visuals().error_fg_color;
        if let Some(error) = &self.run_error {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("The backup failed: {error}"),
            );
        }
        for failed in &self.saving.failed {
            theme::status(
                ui,
                icons::X_CIRCLE,
                error_color,
                format!("What to back up was not saved for {failed}"),
            );
        }
        let warn = ui.visuals().warn_fg_color;
        for warning in &self.saving.warnings {
            theme::status(ui, icons::WARNING, warn, warning);
        }
        let accent = theme::palette(ui).accent;
        if let Some(Cleaned {
            policy_error,
            done,
            error,
        }) = &self.cleanup
        {
            if let Some(error) = policy_error {
                let text = "The cleanup rules could not be read, so no old backup was removed";
                theme::status(ui, icons::WARNING, warn, format!("{text}: {error}"));
            }
            if done.forgotten > 0 {
                let removed = count(done.forgotten, "old backup");
                let text = format!("Removed {removed}");
                theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
            }
            if done.append_only_kept > 0 {
                let kept = count(done.append_only_kept, "old backup");
                let text = format!("The repository is append-only, so {kept} stayed");
                theme::status(ui, icons::WARNING, warn, text);
            }
            if let Some(error) = error {
                let text = format!("Cleanup did not finish: {error}");
                theme::status(ui, icons::X_CIRCLE, error_color, text);
            }
        }
        if progress.done.is_empty() {
            return;
        }
        ui.add_space(16.0);
        ui.heading("Last backup");
        ui.spacing_mut().item_spacing.y = 4.0;
        for GameResult { title, outcome } in &progress.done {
            let text = match outcome {
                Outcome::Saved => format!("{title}: backed up"),
                Outcome::Unchanged => format!("{title}: up to date"),
                Outcome::UnchangedNewerElsewhere(host) => format!(
                    "{title}: up to date with this PC's last backup; the newest backup, from \
                     {host}, differs"
                ),
                Outcome::SameAsNewest(host) => {
                    format!("{title}: same as the newest backup, from {host}")
                }
                Outcome::Failed(error) => {
                    theme::status(
                        ui,
                        icons::X_CIRCLE,
                        error_color,
                        format!("{title}: failed: {error}"),
                    );
                    continue;
                }
            };
            theme::icon_label(ui, icons::CHECK_CIRCLE, accent, text);
        }
    }
}

/// The app icon's save mark, the arc with its arrowhead, turning once every 1.2 s in the accent
/// green while a backup runs: the "saving, do not turn off" sign of games.
fn autosave_mark(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    // The icon's mark spans about 92 of its 128 units; its arc is centered on (64, 63).
    let scale = rect.width() / 92.0;
    let turn = ui.input(|input| input.time) as f32 * TAU / 1.2;
    let rotation = Rot2::from_angle(turn);
    let at = |x: f32, y: f32| rect.center() + rotation * (Vec2::new(x, y) * scale);
    // 270° from the top, clockwise, ending on the left where the arrowhead sits.
    let arc = (0..=24)
        .map(|step| {
            let angle = -FRAC_PI_2 + 1.5 * PI * step as f32 / 24.0;
            at(39.0 * angle.cos(), 39.0 * angle.sin())
        })
        .collect();
    let accent = theme::palette(ui).accent;
    let painter = ui.painter();
    painter.add(Shape::line(arc, Stroke::new(11.0 * scale, accent)));
    painter.add(Shape::convex_polygon(
        vec![at(-53.0, 2.0), at(-25.0, 2.0), at(-39.0, -18.0)],
        accent,
        Stroke::NONE,
    ));
    ui.ctx().request_repaint();
}

fn lock<T>(progress: &Mutex<T>) -> MutexGuard<'_, T> {
    progress.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scan progress callback that stores into `checked` and wakes `ctx`.
fn scan_progress(
    checked: &Arc<Mutex<(usize, usize)>>,
    ctx: &egui::Context,
) -> impl FnMut(usize, usize) + Send + 'static {
    let (checked, ctx) = (checked.clone(), ctx.clone());
    move |done, total| {
        *lock(&checked) = (done, total);
        // Once per percent: a repaint per game would wake the UI tens of thousands of times.
        if (done - 1) * 100 / total != done * 100 / total {
            ctx.request_repaint();
        }
    }
}

/// Backs up `games` into `storage`, comparing each with its newest backup through `mapper`, and
/// calling `progress` as each game is done. Returns the ids of the new snapshots.
fn back_up(
    storage: &SharedStorage,
    games: &[GameScan],
    stage_root: &Path,
    mapper: &Mapper,
    progress: impl FnMut(&GameBackup),
) -> anyhow::Result<Vec<String>> {
    let mut storage = lock_storage(storage)?;
    let same_as_newest = Some(mapper);
    let backups = backup::backup_games(
        &mut storage,
        games,
        stage_root,
        &[],
        same_as_newest,
        progress,
    )?;
    let made = backups
        .into_iter()
        .filter_map(|backup| match backup.result {
            Ok(Stored::New(snapshot)) => Some(snapshot.id),
            _ => None,
        });
    Ok(made.collect())
}

/// Forgets and prunes by the repository's retention policy, never the snapshots `made`
/// (ADR-0015). A policy that cannot be read forgets nothing, but marked packs are still pruned.
fn clean_up(storage: &SharedStorage, made: &[String]) -> Cleaned {
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            return Cleaned {
                policy_error: None,
                done: Cleanup::default(),
                error: Some(format!("{err:#}")),
            };
        }
    };
    let (policy, policy_error) = match retention::load(&mut storage) {
        Ok(policy) => (policy, None),
        Err(err) => (Retention::default(), Some(chain(&err))),
    };
    let keep: Vec<&str> = made.iter().map(String::as_str).collect();
    let (done, error) = match retention::apply(&mut storage, &policy, &keep, SystemTime::now()) {
        Ok(done) => (done, None),
        Err(err) => {
            let mut done = Cleanup::default();
            done.forgotten = err.forgotten;
            (done, Some(chain(&err.source)))
        }
    };
    Cleaned {
        policy_error,
        done,
        error,
    }
}

/// Saves this repository's pending changes of each game whose settings could be read; each
/// saved game's changes leave `pending` and the pending file in `config_dir`. Settings stage
/// below `stage_root` (ADR-0014). Returns the problems and, when the file was written, how the
/// last write went.
fn save_selection(
    storage: &SharedStorage,
    pending: &mut Pending,
    config: &AppConfig,
    settings: &Games,
    stage_root: &Path,
    config_dir: &Path,
) -> (SaveProblems, Option<Result<(), String>>) {
    let mut problems = SaveProblems::default();
    let scope = config.secret_scope();
    let games = pending
        .repositories
        .get(&scope)
        .cloned()
        .unwrap_or_default();
    if games.is_empty() {
        return (problems, None);
    }
    let mut storage = match lock_storage(storage) {
        Ok(storage) => storage,
        Err(err) => {
            problems.failed.push(format!("any game: {err:#}"));
            return (problems, None);
        }
    };
    let mut written = None;
    for (title, changes) in games {
        // Settings that could not be read are never written over.
        if matches!(settings.get(&title), Some(Err(_))) {
            continue;
        }
        match selection::save(&mut storage, &title, &changes, stage_root) {
            Ok(saved) => {
                if let Some(err) = saved.forget_error {
                    problems.warnings.push(format!(
                        "{title}: older copies of what to back up stay in the repository: {}",
                        chain(&err)
                    ));
                }
                pending.remove(config, &title);
                written = Some(pending.save(config_dir).map_err(|err| chain(&err)));
            }
            Err(err) => problems.failed.push(format!("{title}: {}", chain(&err))),
        }
    }
    (problems, written)
}

/// The games of `report` as `settings` filter them; a game skipped or with nothing left is
/// left out, and one whose settings cannot be read is backed up whole.
fn chosen_files(report: &ScanReport, settings: &Games) -> Vec<GameScan> {
    report
        .games
        .iter()
        .filter_map(|game| match settings.get(&game.title) {
            Some(Ok(settings)) => selection::apply(game, settings),
            _ => Some(game.clone()),
        })
        .collect()
}

/// What the user clicked in a game's file tree besides a checkbox.
enum FileAction {
    /// A folder's open button: the folder on disk.
    Open(PathBuf),
    /// The Details button of the game's file at this position of its scan.
    Details(usize),
}

/// What the scan found for one game, with its files as a checkbox tree and its registry keys
/// folded away. Returns the change the user made this frame; a folder to open or a file's
/// details to show goes to `action`.
fn files_ui(
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

/// Gives the tab a backup that runs until the returned sender is dropped.
#[cfg(test)]
impl BackupTab {
    pub(crate) fn hold_backup(&mut self, ctx: &egui::Context) -> std::sync::mpsc::Sender<()> {
        let (release, job) = crate::job::held(ctx);
        self.running = Some(job);
        release
    }

    /// Whether the backup's job has ended, without taking its result.
    pub(crate) fn run_ended(&mut self) -> bool {
        self.running.as_mut().is_none_or(|job| !job.is_running())
    }
}

#[cfg(test)]
mod tests;
