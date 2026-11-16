//! Cross-build mapping: proposes where the roots of a snapshot go in the other OS family
//! (native Linux build <-> Windows build) and resolves the user's choices (ADR-0011).

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::config::AppConfig;
use crate::layout::{self, Base, Context, LinuxFolders, SnapshotMeta, WinFolders};
use crate::manifest::{CustomGame, FileRule, GameEntry, Integration, Os, Pair, Tag};

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
pub enum Confidence {
    /// A pair the user saved.
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
pub struct MappingEntry {
    /// The snapshot root being mapped.
    pub root: SnapshotRoot,
    /// Files staged under the root. [`propose`] sees only the metadata and leaves 0; the caller
    /// that knows the staged files fills it.
    pub files: usize,
    /// The target placeholder template; `None` only for [`Confidence::Unknown`].
    pub proposed: Option<String>,
    /// How `proposed` was found.
    pub confidence: Confidence,
    /// The manifest's save templates for the target family, as glob-free templates.
    pub candidates: Vec<String>,
    /// Facts the user should see, such as the Unity `PlayerPrefs` note.
    pub notes: Vec<String>,
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
pub struct ResolvedMapping {
    /// The snapshot root.
    pub root: SnapshotRoot,
    /// Where the root's files go; `None` when the user skipped the root.
    pub target: Option<(Base, String)>,
}

/// Why [`resolve`] failed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
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
fn classify(template: &str, family: Family) -> Option<(Base, String)> {
    let classified = layout::classify(template, &family_context(family))?;
    let plain = components(&classified.1).all(layout::is_plain_component);
    plain.then_some(classified)
}

/// Classifies a template typed by the user: a placeholder template without globs, not a real
/// path (ADR-0004, ADR-0011).
fn classify_choice(template: &str, family: Family) -> Option<(Base, String)> {
    let classified = classify(template, family)?;
    let real_path = matches!(classified.0, Base::Absolute | Base::WinAbsolute);
    (!real_path && layout::glob_free_prefix(template) == template).then_some(classified)
}

/// A side of a [`Pair`] that is not a glob-free placeholder template of its OS family.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a placeholder template of the {side:?} side: {template}")]
pub struct PairError {
    /// The side that was rejected.
    pub side: Family,
    /// The rejected template.
    pub template: String,
}

/// Checks that `pair` has the same form [`resolve`] demands of a typed target: each side a
/// glob-free placeholder template of its own family, not a real path.
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

fn pair_side(pair: &Pair, family: Family) -> &str {
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

/// The entry's only Linux and only Windows save template as a pair of glob-free prefixes.
fn auto_pair(entry: &GameEntry) -> Option<Pair> {
    let (&[linux], &[windows]) = (
        &save_templates(entry, Family::Linux)[..],
        &save_templates(entry, Family::Windows)[..],
    ) else {
        return None;
    };
    Some(Pair {
        linux: layout::glob_free_prefix(linux).to_owned(),
        windows: layout::glob_free_prefix(windows).to_owned(),
    })
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
/// mapping, and `Absolute`/`WinAbsolute` roots restore to the same OS only (ADR-0005). Order of sources: saved `pairs`, [`ENGINE_RULES`], the manifest auto-pair, unknown.
pub fn propose(
    meta: &SnapshotMeta,
    entry: Option<&GameEntry>,
    pairs: &[Pair],
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
            let (proposed, confidence) = if let Some(mapped) = saved {
                (Some(mapped.target), Confidence::Saved)
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
/// confidence, and `Skip` drops the root. Every target is classified in the family opposite the
/// root's.
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
fn root_template((base, rest): &SnapshotRoot) -> String {
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

/// Saves each `Template` choice as a [`Pair`] of the root and the chosen template on the custom
/// game `title`, creating an `Extend` custom game when there is none, and replacing an earlier
/// pair for the same root. `Skip` choices have no pair form and are not saved. Templates must
/// have passed [`resolve`].
pub fn apply_choices(config: &mut AppConfig, title: &str, choices: &[(SnapshotRoot, Choice)]) {
    let new_pairs: Vec<(Family, &SnapshotRoot, Pair)> = choices
        .iter()
        .filter_map(|(root, choice)| {
            let (Choice::Template(target), Some(family)) = (choice, base_family(root.0)) else {
                return None;
            };
            let own = root_template(root);
            let target = target.clone();
            Some(match family {
                Family::Linux => (
                    family,
                    root,
                    Pair {
                        linux: own,
                        windows: target,
                    },
                ),
                Family::Windows => (
                    family,
                    root,
                    Pair {
                        linux: target,
                        windows: own,
                    },
                ),
            })
        })
        .collect();
    if new_pairs.is_empty() {
        return;
    }
    let index = config
        .custom_games
        .iter()
        .position(|game| game.name == title)
        .unwrap_or_else(|| {
            config.custom_games.push(CustomGame {
                name: title.to_owned(),
                entry: GameEntry::default(),
                integration: Integration::Extend,
                wine_prefix: None,
                pairs: Vec::new(),
            });
            config.custom_games.len() - 1
        });
    let game = &mut config.custom_games[index];
    for (family, root, pair) in new_pairs {
        game.pairs
            .retain(|saved| classify(pair_side(saved, family), family).as_ref() != Some(root));
        game.pairs.push(pair);
    }
}

#[cfg(test)]
mod tests;
