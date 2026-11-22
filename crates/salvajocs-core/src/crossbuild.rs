//! Cross-build mapping: proposes where the roots of a snapshot go in the other OS family
//! (native Linux build <-> Windows build) and resolves the user's choices (ADR-0011).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::layout::{self, Base, Context, GameVars, LinuxFolders, SnapshotMeta, WinFolders};
use crate::manifest::{CustomFile, CustomGame, FileRule, GameEntry, Integration, Os, Pair, Tag};

/// A snapshot root, `(base, rest)` as in [`SnapshotMeta::roots`].
pub type SnapshotRoot = (Base, String);

/// The OS family a path template or base belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// A native Linux build: `Home`, `XdgData`, `XdgConfig`.
    Linux,
    /// A Windows build: the `Win*` bases.
    Windows,
}

impl Family {
    pub(crate) fn other(self) -> Self {
        match self {
            Family::Linux => Family::Windows,
            Family::Windows => Family::Linux,
        }
    }
}

/// How a proposed target was found, best first (ADR-0011).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Confidence {
    /// A pair or skip the user saved; a skip has no `proposed` target.
    Saved,
    /// An [`ENGINE_RULES`] entry.
    EngineRule,
    /// The manifest entry has exactly one Linux and one Windows save entry.
    AutoPair,
    /// No proposal; the user must choose.
    Unknown,
}

/// A Linux and a Windows path pattern of one engine. `{name}` matches one path component and
/// substitutes the same component on the other side.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EngineRule {
    /// Engine name.
    pub name: &'static str,
    /// Linux side, a placeholder template.
    pub linux: &'static str,
    /// Windows side, a placeholder template.
    pub windows: &'static str,
    /// Windows registry key (`/`-separated) where the engine keeps settings the mapping does not
    /// convert; a manifest entry with that key gets [`PLAYER_PREFS_NOTE`].
    pub prefs_key: Option<&'static str>,
}

impl EngineRule {
    fn side(&self, family: Family) -> &'static str {
        match family {
            Family::Linux => self.linux,
            Family::Windows => self.windows,
        }
    }
}

/// Engine layouts from primary sources (ADR-0011): Unity `Application.persistentDataPath`
/// (Unity 6000.6 docs; `PlayerPrefs` live in the registry on Windows) and Godot `user://`
/// (Godot stable docs, "Data paths").
pub const ENGINE_RULES: [EngineRule; 2] = [
    EngineRule {
        name: "Unity",
        linux: "<xdgConfig>/unity3d/{c}/{p}",
        windows: "<winLocalAppDataLow>/{c}/{p}",
        prefs_key: Some("HKEY_CURRENT_USER/Software/{c}/{p}"),
    },
    EngineRule {
        name: "Godot",
        linux: "<xdgData>/godot/app_userdata/{p}",
        windows: "<winAppData>/Godot/app_userdata/{p}",
        prefs_key: None,
    },
];

/// The note added when a Unity game keeps `PlayerPrefs` in the Windows registry.
pub const PLAYER_PREFS_NOTE: &str =
    "Unity settings (PlayerPrefs) are stored differently and are not converted";

/// What to do with one snapshot root on restore.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MappingEntry {
    /// The snapshot root being mapped.
    pub root: SnapshotRoot,
    /// Files staged under the root. [`propose`] sees only the metadata and leaves 0; the caller
    /// that knows the staged files fills it.
    pub files: usize,
    /// The target placeholder template; `None` for [`Confidence::Unknown`] and for a saved skip.
    pub proposed: Option<String>,
    /// How `proposed` was found.
    pub confidence: Confidence,
    /// The manifest's save templates for the target family, as glob-free templates.
    pub candidates: Vec<String>,
    /// Facts the user should see, such as the Unity `PlayerPrefs` note.
    pub notes: Vec<String>,
}

impl MappingEntry {
    /// A mapping entry from its fields.
    pub fn new(
        root: SnapshotRoot,
        files: usize,
        proposed: Option<String>,
        confidence: Confidence,
        candidates: Vec<String>,
        notes: Vec<String>,
    ) -> Self {
        Self {
            root,
            files,
            proposed,
            confidence,
            candidates,
            notes,
        }
    }
}

/// The user's decision for a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Restore the root under this placeholder template in the target family.
    Template(String),
    /// Do not restore the root.
    Skip,
}

/// A root with its final target; the target is already classified in the target family.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedMapping {
    /// The snapshot root.
    pub root: SnapshotRoot,
    /// Where the root's files go; `None` when the user skipped the root.
    pub target: Option<(Base, String)>,
}

/// Why [`resolve`] failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveError {
    /// A template is not a glob-free placeholder template of the target family.
    #[error("not a placeholder template of the target OS: {template}")]
    Unclassifiable {
        /// The root the template was meant for.
        root: SnapshotRoot,
        /// The rejected template.
        template: String,
    },
    /// An unknown root has no choice.
    #[error("no choice for an unmapped root")]
    MissingChoice {
        /// The root without a choice.
        root: SnapshotRoot,
    },
}

/// The family a base belongs to; `None` for the install-relative bases, which resolve on either.
pub(crate) fn base_family(base: Base) -> Option<Family> {
    match base {
        Base::Home | Base::XdgData | Base::XdgConfig | Base::Absolute => Some(Family::Linux),
        Base::WinHome
        | Base::WinDocuments
        | Base::WinAppData
        | Base::WinLocalAppData
        | Base::WinLocalAppDataLow
        | Base::WinPublic
        | Base::WinProgramData
        | Base::WinDir
        | Base::WinAbsolute => Some(Family::Windows),
        Base::GameDir | Base::StoreRoot => None,
    }
}

/// A context of `family` with empty folders; `classify` only looks at the family.
fn family_context(family: Family) -> Context {
    match family {
        Family::Linux => Context::Linux(LinuxFolders {
            user: String::new(),
            home: PathBuf::new(),
            xdg_data: PathBuf::new(),
            xdg_config: PathBuf::new(),
        }),
        Family::Windows => Context::WindowsNative(WinFolders::for_prefix(&PathBuf::new(), "")),
    }
}

/// `layout::classify` in `family`, rejecting a rest with a component that is not plain (`..`),
/// since templates come from config, the user and snapshot roots.
pub(crate) fn classify(template: &str, family: Family) -> Option<(Base, String)> {
    let classified = layout::classify(template, &family_context(family))?;
    let plain = components(&classified.1).all(layout::is_plain_component);
    plain.then_some(classified)
}

/// Classifies a template typed by the user: a placeholder template without globs, not a real
/// path (ADR-0004, ADR-0011).
pub(crate) fn classify_choice(template: &str, family: Family) -> Option<(Base, String)> {
    let classified = classify(template, family)?;
    let real_path = matches!(classified.0, Base::Absolute | Base::WinAbsolute);
    (!real_path && layout::glob_free_prefix(template) == template).then_some(classified)
}

/// A side of a [`Pair`] that is not a glob-free placeholder template of its OS family.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a placeholder template of the {side:?} side: {template}")]
#[non_exhaustive]
pub struct PairError {
    /// The side that was rejected.
    pub side: Family,
    /// The rejected template.
    pub template: String,
}

/// Checks that `pair` has the same form [`resolve`] demands of a typed target: each side a
/// glob-free placeholder template of its own family, not a real path. A side on an
/// install-relative base (`<base>`, `<root>`) passes but never matches a root.
///
/// # Errors
/// [`PairError`] for the first side that does not classify, Linux before Windows.
pub fn validate_pair(pair: &Pair) -> Result<(), PairError> {
    [Family::Linux, Family::Windows]
        .into_iter()
        .find(|&side| classify_choice(pair_side(pair, side), side).is_none())
        .map_or(Ok(()), |side| {
            Err(PairError {
                side,
                template: pair_side(pair, side).to_owned(),
            })
        })
}

/// A skip that is not a glob-free placeholder template.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a placeholder template: {template}")]
#[non_exhaustive]
pub struct SkipError {
    /// The rejected template.
    pub template: String,
}

/// Checks that `skip` has the form of a pair side: a glob-free placeholder template of either
/// OS family, not a real path. A skip has no family: `<home>/Foo` also matches the Windows root
/// `<home>/Foo`. One on an install-relative base (`<base>`, `<root>`) passes but never matches.
///
/// # Errors
/// [`SkipError`] when it classifies in neither family.
pub fn validate_skip(skip: &str) -> Result<(), SkipError> {
    [Family::Linux, Family::Windows]
        .into_iter()
        .any(|family| classify_choice(skip, family).is_some())
        .then_some(())
        .ok_or_else(|| SkipError {
            template: skip.to_owned(),
        })
}

/// Why a custom game file row is rejected (ADR-0016).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FileError {
    /// Neither side is set.
    #[error("enter a Linux path, a Windows path or both")]
    Empty,
    /// A side is not a placeholder template or an absolute path of its OS family.
    #[error("not a {family:?} path: {template}")]
    Side {
        /// The rejected side.
        family: Family,
        /// Its template.
        template: String,
    },
    /// A placeholder template separates folders with `\`.
    #[error("use / between folders after a placeholder: {template}")]
    Backslash {
        /// The rejected side.
        family: Family,
        /// Its template.
        template: String,
    },
    /// A side names a whole base or drive (`<home>`, `<home>/*`, `/`), which a restore never
    /// trusts: a folder or a literal name must follow the base.
    #[error("name a folder below the base: {template}")]
    TooWide {
        /// The rejected side.
        family: Family,
        /// Its template.
        template: String,
    },
}

/// Checks a custom game file row: at least one side, each a placeholder template of its family
/// (folders separated by `/`) or an absolute path of that OS (`/...` on Linux, `C:\...` or
/// `C:/...` on Windows), without `.` or `..`. Textual, so every host gives the same answer.
///
/// # Errors
/// [`FileError`] for the first side that breaks a rule, Linux before Windows.
pub fn validate_file(file: &CustomFile) -> Result<(), FileError> {
    let sides = [
        (&file.linux, Family::Linux),
        (&file.windows, Family::Windows),
    ];
    if sides.iter().all(|(side, _)| side.is_none()) {
        return Err(FileError::Empty);
    }
    sides
        .into_iter()
        .filter_map(|(side, family)| Some((side.as_deref()?, family)))
        .try_for_each(|(template, family)| check_side(template, family))
}

/// Whether a file `template` of `family` trusts a whole base or real-path root: nothing but
/// the base (`<home>`, `/`, `C:/`, `<root>`), or a glob below it with no literal character at
/// all (`<home>/*`, `<home>/*/*`). Placeholders count as wild, since a restore matches
/// `<osUserName>` with `*`. Restore declares no root for it (ADR-0005). `<base>`, one game's
/// install folder, is never too wide; `<root>` is the whole Steam root.
pub(crate) fn too_wide(template: &str, family: Family) -> bool {
    let prefix = layout::glob_free_prefix(template);
    // Decided here, not by `classify`, so `/*` and a Windows host agree with every other host.
    let linux_absolute = family == Family::Linux && template.starts_with('/');
    // A drive path classifies only on a Windows host, and `C:` alone (before `/*`) never.
    let drive = |text: &str| match text.as_bytes() {
        [letter, b':'] if letter.is_ascii_alphabetic() => Some(char::from(*letter).to_string()),
        _ => layout::drive_absolute_tail(text),
    };
    let classified = if linux_absolute {
        Some((Base::Absolute, prefix.trim_start_matches('/').to_owned()))
    } else {
        classify(prefix, family).or_else(|| match family {
            Family::Windows => drive(prefix).map(|tail| (Base::WinAbsolute, tail)),
            Family::Linux => None,
        })
    };
    let Some((base, rest)) = classified else {
        return false;
    };
    // `WinAbsolute` rests start with the drive letter.
    let whole = usize::from(base == Base::WinAbsolute);
    if base == Base::GameDir || components(&rest).count() > whole {
        return false;
    }
    // `[...]` classes hold no literal; a `]` right after `[` or `[!` is a member (as in
    // `layout::fold_case`).
    let (mut in_class, mut first, mut in_placeholder) = (false, false, false);
    let literal = template[prefix.len()..].chars().any(|c| {
        if in_class {
            if c == ']' && !first {
                in_class = false;
            } else if c != '!' || !first {
                first = false;
            }
            return false;
        }
        if in_placeholder {
            in_placeholder = c != '>';
            return false;
        }
        match c {
            '[' => {
                (in_class, first) = (true, true);
                false
            }
            '<' => {
                in_placeholder = true;
                false
            }
            '*' | '?' | '/' => false,
            _ => true,
        }
    });
    !literal
}

/// Checks one side of a file row, see [`validate_file`].
fn check_side(template: &str, family: Family) -> Result<(), FileError> {
    let placeholder = |template: &str| {
        layout::classify(template, &family_context(family))
            .is_some_and(|(base, _)| !matches!(base, Base::Absolute | Base::WinAbsolute))
    };
    // `Path` splits on `\` only on Windows, so a `\` would make the result host-dependent.
    if template.contains('\\') && placeholder(&template.replace('\\', "/")) {
        return Err(FileError::Backslash {
            family,
            template: template.to_owned(),
        });
    }
    let absolute = match family {
        Family::Linux => template.starts_with('/'),
        Family::Windows => layout::drive_absolute_tail(template).is_some(),
    };
    let dots = template
        .split(['/', '\\'])
        .any(|part| part == "." || part == "..");
    if (placeholder(template) || absolute) && !dots {
        // `CustomGame::game_entry` stores a drive path with `/`, so restore sees it that way.
        if too_wide(&template.replace('\\', "/"), family) {
            return Err(FileError::TooWide {
                family,
                template: template.to_owned(),
            });
        }
        Ok(())
    } else {
        Err(FileError::Side {
            family,
            template: template.to_owned(),
        })
    }
}

/// The glob-free prefixes of `linux` and `windows` as a pair, when the text after them is the
/// same on both sides. A glob part only one side has (or an escaped `[[]` folder name) would cut
/// that side short of the folder that matches the other.
fn prefix_pair(linux: &str, windows: &str) -> Option<Pair> {
    let pair = Pair {
        linux: layout::glob_free_prefix(linux).to_owned(),
        windows: layout::glob_free_prefix(windows).to_owned(),
    };
    (linux[pair.linux.len()..] == windows[pair.windows.len()..]).then_some(pair)
}

/// The mapping a file row with the sides `linux` and `windows` declares: their glob-free
/// prefixes, when both pass [`validate_pair`] (so no absolute paths), the text after both
/// prefixes is the same (none, or the same glob such as `/*.sav`) and, on a Linux or Windows
/// base, each holds a folder below the placeholder; a bare base would also map every other root
/// of that base.
pub(crate) fn row_pair(linux: &str, windows: &str) -> Option<Pair> {
    let pair = prefix_pair(linux, windows)?;
    validate_pair(&pair).ok()?;
    let bare = |side| {
        classify(pair_side(&pair, side), side).is_some_and(|(base, rest)| {
            base_family(base).is_some() && components(&rest).next().is_none()
        })
    };
    (!bare(Family::Linux) && !bare(Family::Windows)).then_some(pair)
}

pub(crate) fn pair_side(pair: &Pair, family: Family) -> &str {
    match family {
        Family::Linux => &pair.linux,
        Family::Windows => &pair.windows,
    }
}

fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|part| !part.is_empty())
}

/// A root matched against one side of a rule.
struct Mapped {
    /// Components of the matched side; a deeper match wins.
    depth: usize,
    target: String,
    /// `{name}` captures as `(name, value)`.
    captures: Vec<(String, String)>,
}

/// Maps `root` from the `from` template (in `source`) to the `to` template: the bases are equal,
/// the root's rest starts with `from`'s rest on a component boundary (ASCII case ignored for
/// Windows), and the remaining components follow `to`. Only engine rules (`captures_allowed`)
/// treat `{name}` as a capture; elsewhere it is a literal name.
fn map_root(
    root: &SnapshotRoot,
    from: &str,
    to: &str,
    source: Family,
    captures_allowed: bool,
) -> Option<Mapped> {
    map_root_as_stored(root, from, to, source, captures_allowed).or_else(|| {
        // A root stored before `<home>/AppData/..` matched without case, as a template now says it.
        let root = layout::normalized_win_home(root.0, &root.1)?;
        map_root_as_stored(&root, from, to, source, captures_allowed)
    })
}

fn map_root_as_stored(
    root: &SnapshotRoot,
    from: &str,
    to: &str,
    source: Family,
    captures_allowed: bool,
) -> Option<Mapped> {
    let (base, rest) = classify(from, source)?;
    if base != root.0 {
        return None;
    }
    let pattern: Vec<&str> = components(&rest).collect();
    let tail: Vec<&str> = components(&root.1).collect();
    if tail.len() < pattern.len() {
        return None;
    }
    let mut captures = Vec::new();
    for (part, actual) in pattern.iter().zip(&tail) {
        if let Some(name) = captures_allowed
            .then(|| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')))
            .flatten()
        {
            captures.push((name.to_owned(), (*actual).to_owned()));
        } else {
            let same = match source {
                Family::Windows => part.eq_ignore_ascii_case(actual),
                Family::Linux => part == actual,
            };
            if !same {
                return None;
            }
        }
    }
    let mut target = to.to_owned();
    for (name, value) in &captures {
        target = target.replace(&format!("{{{name}}}"), value);
    }
    let below = tail[pattern.len()..].join("/");
    if !below.is_empty() {
        target = format!("{}/{below}", target.trim_end_matches('/'));
    }
    Some(Mapped {
        depth: pattern.len(),
        target,
        captures,
    })
}

/// The family a manifest file entry applies to: its `when.os`, or without one the leading
/// `<xdg*>`/`<win*>` placeholder. A bare `<home>` says nothing (ADR-0011).
fn rule_family(template: &str, rule: &FileRule) -> Option<Family> {
    let oses: Vec<Os> = rule.when.iter().filter_map(|when| when.os).collect();
    let linux = oses.contains(&Os::Linux);
    let windows = oses.contains(&Os::Windows);
    match (linux, windows) {
        (true, false) => Some(Family::Linux),
        (false, true) => Some(Family::Windows),
        (false, false) if oses.is_empty() => {
            if template.starts_with("<xdg") {
                Some(Family::Linux)
            } else if template.starts_with("<win") {
                Some(Family::Windows)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The save-tagged file templates of `family`.
fn save_templates(entry: &GameEntry, family: Family) -> Vec<&str> {
    entry
        .files
        .iter()
        .filter(|(template, rule)| {
            rule.tags.contains(&Tag::Save) && rule_family(template, rule) == Some(family)
        })
        .map(|(template, _)| template.as_str())
        .collect()
}

/// The entry's only Linux and only Windows save template as a pair of glob-free prefixes, when
/// both end the same after them (see `prefix_pair`).
fn auto_pair(entry: &GameEntry) -> Option<Pair> {
    let (&[linux], &[windows]) = (
        &save_templates(entry, Family::Linux)[..],
        &save_templates(entry, Family::Windows)[..],
    ) else {
        return None;
    };
    prefix_pair(linux, windows)
}

/// Whether the manifest entry has a registry key at or below `key`, ignoring case and `\`.
fn has_registry_key(entry: &GameEntry, key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    entry.registry.keys().any(|candidate| {
        let candidate = candidate.replace('\\', "/").to_ascii_lowercase();
        candidate == key || candidate.starts_with(&format!("{key}/"))
    })
}

/// Proposes a target for every root of `meta` that belongs to the family opposite
/// `target_family`; roots already in the target family and install-relative roots need no
/// mapping, and `Absolute`/`WinAbsolute` roots restore to the same OS only (ADR-0005). Order of
/// sources: saved `pairs` and `skips` (the deeper match wins, a pair on a tie; a skip proposes
/// no target), [`ENGINE_RULES`], the manifest auto-pair, unknown.
pub fn propose(
    meta: &SnapshotMeta,
    entry: Option<&GameEntry>,
    pairs: &[Pair],
    skips: &[String],
    target_family: Family,
) -> Vec<MappingEntry> {
    let source = target_family.other();
    let candidates: BTreeSet<&str> = entry
        .map(|entry| save_templates(entry, target_family))
        .unwrap_or_default()
        .into_iter()
        .map(layout::glob_free_prefix)
        .filter(|template| classify_choice(template, target_family).is_some())
        .collect();
    let auto = entry.and_then(auto_pair);
    // A saved pair from the config may not classify; it then maps nothing.
    let usable = |mapped: &Mapped| classify(&mapped.target, target_family).is_some();
    let through_pair = |root: &SnapshotRoot, pair: &Pair| {
        map_root(
            root,
            pair_side(pair, source),
            pair_side(pair, target_family),
            source,
            false,
        )
        .filter(usable)
    };
    meta.roots
        .iter()
        .filter(|root| {
            base_family(root.0) == Some(source)
                && !matches!(root.0, Base::Absolute | Base::WinAbsolute)
        })
        .map(|root| {
            let saved = pairs
                .iter()
                .filter_map(|pair| through_pair(root, pair))
                .max_by_key(|mapped| mapped.depth);
            let skipped = skips
                .iter()
                .filter_map(|skip| map_root(root, skip, skip, source, false))
                .map(|mapped| mapped.depth)
                .max();
            let engine = ENGINE_RULES.iter().find_map(|rule| {
                let mapped = map_root(
                    root,
                    rule.side(source),
                    rule.side(target_family),
                    source,
                    true,
                )?;
                usable(&mapped).then_some((rule, mapped))
            });
            // The note describes the game, so it stays once the mapping is a saved pair.
            let notes: Vec<String> = engine
                .as_ref()
                .and_then(|(rule, mapped)| {
                    let key = mapped
                        .captures
                        .iter()
                        .fold(rule.prefs_key?.to_owned(), |key, (name, value)| {
                            key.replace(&format!("{{{name}}}"), value)
                        });
                    has_registry_key(entry?, &key).then(|| PLAYER_PREFS_NOTE.to_owned())
                })
                .into_iter()
                .collect();
            let (proposed, confidence) = if let Some(mapped) =
                saved.filter(|mapped| Some(mapped.depth) >= skipped)
            {
                (Some(mapped.target), Confidence::Saved)
            } else if skipped.is_some() {
                (None, Confidence::Saved)
            } else if let Some((_, mapped)) = engine {
                (Some(mapped.target), Confidence::EngineRule)
            } else if let Some(mapped) = auto.as_ref().and_then(|pair| through_pair(root, pair)) {
                (Some(mapped.target), Confidence::AutoPair)
            } else {
                (None, Confidence::Unknown)
            };
            MappingEntry {
                root: root.clone(),
                files: 0,
                proposed,
                confidence,
                candidates: candidates.iter().map(|&c| c.to_owned()).collect(),
                notes,
            }
        })
        .collect()
}

/// Applies `choices` to `entries`: a choice for a root replaces its proposal whatever the
/// confidence, and `Skip` drops the root; a saved skip drops it too. Every target is classified
/// in the family opposite the root's.
///
/// # Errors
/// [`ResolveError::Unclassifiable`] for a chosen template that is not a glob-free placeholder
/// template of the target family; [`ResolveError::MissingChoice`] for an unknown root without
/// a choice.
pub fn resolve(
    entries: &[MappingEntry],
    choices: &[(SnapshotRoot, Choice)],
) -> Result<Vec<ResolvedMapping>, ResolveError> {
    entries
        .iter()
        .map(|entry| {
            let choice = choices
                .iter()
                .find(|(root, _)| *root == entry.root)
                .map(|(_, choice)| choice);
            let target_family = base_family(entry.root.0).map(Family::other);
            let unclassifiable = |template: &String| ResolveError::Unclassifiable {
                root: entry.root.clone(),
                template: template.clone(),
            };
            let target = match (choice, &entry.proposed) {
                (Some(Choice::Skip), _) => None,
                (Some(Choice::Template(template)), _) => Some(
                    target_family
                        .and_then(|family| classify_choice(template, family))
                        .ok_or_else(|| unclassifiable(template))?,
                ),
                (None, Some(template)) => Some(
                    target_family
                        .and_then(|family| classify(template, family))
                        .ok_or_else(|| unclassifiable(template))?,
                ),
                (None, None) if entry.confidence == Confidence::Saved => None,
                (None, None) => {
                    return Err(ResolveError::MissingChoice {
                        root: entry.root.clone(),
                    });
                }
            };
            Ok(ResolvedMapping {
                root: entry.root.clone(),
                target,
            })
        })
        .collect()
}

/// The placeholder template that classifies back to `root` in its own family.
pub fn root_template((base, rest): &SnapshotRoot) -> String {
    let leading = match base {
        Base::WinHome | Base::Home => "<home>",
        Base::WinDocuments => "<winDocuments>",
        Base::WinAppData => "<winAppData>",
        Base::WinLocalAppData => "<winLocalAppData>",
        Base::WinLocalAppDataLow => "<winLocalAppDataLow>",
        Base::WinPublic => "<winPublic>",
        Base::WinProgramData => "<winProgramData>",
        Base::WinDir => "<winDir>",
        Base::XdgData => "<xdgData>",
        Base::XdgConfig => "<xdgConfig>",
        Base::GameDir => "<base>",
        Base::StoreRoot => "<root>",
        Base::Absolute => "",
        Base::WinAbsolute => {
            return match rest.split_once('/') {
                Some((drive, below)) => format!("{drive}:/{below}"),
                None => format!("{rest}:/"),
            };
        }
    };
    format!("{leading}/{rest}")
}

/// The path template for a folder the user picked, glob-escaped: the deepest base of `ctx`'s
/// family, `GameDir` or `StoreRoot` that contains `path`. When the template would classify back
/// to a different directory (`XDG_CONFIG_HOME` outside `<home>/.config`, a redirected
/// `%APPDATA%`) or nothing contains `path`, the absolute path on a native context, else `None`.
pub fn template_for(path: &Path, ctx: &Context, vars: &GameVars) -> Option<String> {
    let leading = if ctx.win().is_some() {
        &layout::WIN_LEADING[..]
    } else {
        &layout::LINUX_LEADING[..]
    };
    // `max_by_key` keeps the last maximum, so at equal depth `GameDir`/`StoreRoot` win.
    let deepest = leading
        .iter()
        .chain(&layout::ANY_LEADING)
        .flat_map(|&(_, base)| {
            layout::base_dirs(base, ctx, vars)
                .into_iter()
                .map(move |dir| (base, dir))
        })
        .filter_map(|(base, dir)| {
            Some((base, layout::tail_of(path, &dir)?, dir.components().count()))
        })
        .max_by_key(|&(.., depth)| depth);
    let exact = deepest.and_then(|(base, tail, _)| {
        let root = (base, glob::Pattern::escape(&tail));
        let template = root_template(&root);
        (layout::classify(&template, ctx).as_ref() == Some(&root)).then_some(template)
    });
    let template = exact.or_else(|| {
        let root = match ctx {
            Context::Linux(_) => (Base::Absolute, layout::tail_of(path, Path::new("/"))?),
            Context::WindowsNative(_) => (Base::WinAbsolute, layout::tail_of(path, Path::new(""))?),
            Context::WinePrefix { .. } => return None,
        };
        Some(root_template(&(root.0, glob::Pattern::escape(&root.1))))
    })?;
    // A pick of a base dir itself has an empty tail: `<home>`, not `<home>/`.
    Some(match template.strip_suffix('/') {
        Some(bare) if !bare.is_empty() && !bare.ends_with(':') => bare.to_owned(),
        _ => template,
    })
}

/// Saves each choice on the custom game `title` of `games`, creating an `Extend` custom game when
/// there is none: `Template` as a [`Pair`] of the root and the chosen template, `Skip` as the
/// root's template in `skips`. A choice replaces any earlier pair or skip for the same root; a
/// pair one of the game's file rows already declares ([`CustomGame::file_pairs`]) is not stored.
/// Templates must have passed [`resolve`].
pub fn apply_choices(games: &mut Vec<CustomGame>, title: &str, choices: &[(SnapshotRoot, Choice)]) {
    let choices: Vec<_> = choices
        .iter()
        .filter_map(|(root, choice)| Some((base_family(root.0)?, root, choice)))
        .collect();
    if choices.is_empty() {
        return;
    }
    let index = games
        .iter()
        .position(|game| game.name == title)
        .unwrap_or_else(|| {
            games.push(CustomGame {
                name: title.to_owned(),
                files: Vec::new(),
                entry: GameEntry::default(),
                integration: Integration::Extend,
                wine_prefix: None,
                pairs: Vec::new(),
                skips: Vec::new(),
            });
            games.len() - 1
        });
    let game = &mut games[index];
    let rows = game.file_pairs();
    for (family, root, choice) in choices {
        let normalized = layout::normalized_win_home(root.0, &root.1);
        let is_root = |template: &str| {
            let classified = classify(template, family);
            classified.is_some() && (classified.as_ref() == Some(root) || classified == normalized)
        };
        game.pairs
            .retain(|saved| !is_root(pair_side(saved, family)));
        game.skips.retain(|saved| !is_root(saved));
        let own = root_template(root);
        let pair = match (choice, family) {
            (Choice::Skip, _) => {
                game.skips.push(own);
                continue;
            }
            (Choice::Template(target), Family::Linux) => Pair {
                linux: own,
                windows: target.clone(),
            },
            (Choice::Template(target), Family::Windows) => Pair {
                linux: target.clone(),
                windows: own,
            },
        };
        let from_row = rows.iter().any(|row| {
            [Family::Linux, Family::Windows].into_iter().all(|side| {
                classify(pair_side(row, side), side) == classify(pair_side(&pair, side), side)
            })
        });
        if !from_row {
            game.pairs.push(pair);
        }
    }
}

#[cfg(test)]
mod tests;
