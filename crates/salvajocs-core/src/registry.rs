//! Registry keys as normalized data (ADR-0006), shared by the Wine and Windows backends.

#[cfg(windows)]
pub mod windows;
pub mod wine;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Root every key path starts with; only HKCU is supported in v1.
pub const HKCU: &str = "HKEY_CURRENT_USER";

/// Keys below `HKEY_CURRENT_USER` that run code at logon or change the shell, lowercase with
/// `/`; `Software/Classes` is handled apart, for its `VirtualStore` exception.
const PROTECTED_KEYS: [&str; 6] = [
    "software/microsoft/windows",
    "software/microsoft/windows nt",
    "software/microsoft/command processor",
    "software/policies",
    "environment",
    "control panel",
];

/// Whether a game must not declare the registry `key`, so a restore never writes it for one:
/// the key is not under `HKEY_CURRENT_USER`, has fewer than two parts below it (`Software`),
/// or equals, contains or lies below a system key: `Software/Microsoft/Windows`, `.../Windows
/// NT`, `.../Command Processor`, `Software/Policies`, `Software/Classes` (but not
/// `Classes/VirtualStore`), `Environment` and `Control Panel`. Ignores case, takes `\` and `/`
/// alike and reads `Software/Wow6432Node/...` as `Software/...`. The list is not exhaustive.
pub fn protected_key(key: &str) -> bool {
    let key = key.replace('\\', "/").to_ascii_lowercase();
    let key = key.trim_end_matches('/');
    let Some(rest) = (key.strip_prefix("hkey_current_user/")).filter(|rest| !rest.is_empty())
    else {
        return true;
    };
    let rest = match rest.strip_prefix("software/wow6432node") {
        Some(below) if below.is_empty() || below.starts_with('/') => format!("software{below}"),
        _ => rest.to_owned(),
    };
    if rest.split('/').count() < 2 {
        return true;
    }
    let under = |path: &str, dir: &str| {
        (path.strip_prefix(dir)).is_some_and(|below| below.is_empty() || below.starts_with('/'))
    };
    let related = |system: &str| under(&rest, system) || under(system, &rest);
    let classes = related("software/classes") && !under(&rest, "software/classes/virtualstore");
    classes || PROTECTED_KEYS.iter().any(|system| related(system))
}

/// Errors from reading, exporting or replacing registry data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The text does not start with Wine's `WINE REGISTRY Version 2` line.
    #[error("not a Wine registry file")]
    NotRegistryFile,
    /// A `[key]` line is unterminated or its name is not valid UTF-16.
    #[error("malformed key header at line {line}")]
    MalformedKey { line: usize },
    /// A value line in the key block starting at `line` cannot be parsed.
    #[error("malformed value in the key block at line {line}")]
    MalformedValue { line: usize },
    /// A key path is not under `HKEY_CURRENT_USER` (backslash separated).
    #[error("key is not under HKEY_CURRENT_USER: {key}")]
    UnsupportedRoot { key: String },
    /// A key path is the HKCU root itself or has an empty element (doubled or trailing `\`).
    #[error("invalid key path: {key}")]
    InvalidKey { key: String },
    /// A replace was asked to both write and keep `key` (ADR-0026).
    #[error("registry key is both written and kept: {key}")]
    KeptAndWritten { key: String },
}

/// A registry value's data, tagged by Windows value type.
///
/// Types without a typed form (e.g. a `REG_DWORD` holding 8 bytes, as Unity writes) are kept as
/// [`RegData::Other`] with their raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RegData {
    Sz(String),
    ExpandSz(String),
    /// Strings of a `REG_MULTI_SZ`, without the terminating empty string.
    MultiSz(Vec<String>),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
    /// Any other Windows value type `code`, as raw little-endian bytes.
    Other {
        code: u32,
        bytes: Vec<u8>,
    },
}

/// One named value of a key; the empty name is the key's default value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RegValue {
    pub name: String,
    #[serde(flatten)]
    pub data: RegData,
}

impl RegValue {
    /// The value `name` holding `data`.
    pub fn new(name: String, data: RegData) -> Self {
        Self { name, data }
    }
}

/// Values of the exported keys, keyed by full path (`HKEY_CURRENT_USER\Software\Foo`).
///
/// An export of key `K` holds `K` itself (possibly without values) plus every existing key
/// below it, or nothing when `K` is absent. Restoring replaces the subtree of each key present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[non_exhaustive]
pub struct RegistryExport {
    pub keys: BTreeMap<String, Vec<RegValue>>,
}

impl RegistryExport {
    /// The export's keys as a tree, in preorder: every key, plus each missing key between a key
    /// and its nearest ancestor in the export (an added node: a Wine export leaves out a
    /// valueless key with subkeys). Nothing is added above a key without an ancestor. Keys equal
    /// but for case are one node, matched as the restore matches them; siblings sort by their
    /// lowercase elements, so `a` comes before `a-b`.
    pub fn tree(&self) -> Vec<KeyNode> {
        // Keyed by lowercase elements: case variants merge and every subtree is contiguous.
        let mut nodes: BTreeMap<Vec<String>, (String, Vec<String>)> = BTreeMap::new();
        for key in self.keys.keys() {
            let folded = key.to_lowercase().split('\\').map(String::from).collect();
            let node = nodes
                .entry(folded)
                .or_insert_with(|| (key.clone(), Vec::new()));
            node.1.push(key.clone());
        }
        let present: Vec<(Vec<String>, String)> = (nodes.iter())
            .map(|(folded, (key, _))| (folded.clone(), key.clone()))
            .collect();
        for (folded, key) in present {
            let nearest = (1..folded.len())
                .rev()
                .find(|&n| nodes.contains_key(&folded[..n]));
            let Some(ancestor) = nearest else {
                continue;
            };
            let elements: Vec<&str> = key.split('\\').collect();
            for n in ancestor + 1..folded.len() {
                (nodes.entry(folded[..n].to_vec()))
                    .or_insert_with(|| (elements[..n].join("\\"), Vec::new()));
            }
        }
        let mut tree: Vec<KeyNode> = Vec::with_capacity(nodes.len());
        let mut open: Vec<(usize, Vec<String>)> = Vec::new();
        for (folded, (key, keys)) in nodes {
            while let Some((at, above)) = open.last() {
                if folded.starts_with(above) {
                    break;
                }
                tree[*at].end = tree.len();
                open.pop();
            }
            open.push((tree.len(), folded));
            tree.push(KeyNode {
                key,
                keys,
                depth: open.len() - 1,
                end: 0,
            });
        }
        for (at, _) in open {
            tree[at].end = tree.len();
        }
        tree
    }

    /// What a restore of the ticked nodes of `tree` writes, and the keys it keeps (ADR-0026):
    /// the export of every ticked node with its values, under all of its spellings, and one
    /// spelling of every unticked node. A ticked added node is written, without values, only
    /// when its parent is unticked; under a ticked parent it would change nothing.
    ///
    /// `tree` must be [`Self::tree`] of this export and `ticked` hold one tick per node.
    ///
    /// # Panics
    /// When `ticked` and `tree` differ in length, or `tree` holds a key this export does not.
    pub fn select(&self, tree: &[KeyNode], ticked: &[bool]) -> (RegistryExport, Vec<String>) {
        assert_eq!(tree.len(), ticked.len(), "one tick per node");
        let mut write = RegistryExport::default();
        let mut kept = Vec::new();
        for (at, node) in tree.iter().enumerate() {
            if !ticked[at] {
                kept.push(node.key.clone());
            } else if node.keys.is_empty() {
                let parent = tree[..at]
                    .iter()
                    .rposition(|above| above.depth < node.depth);
                if parent.is_some_and(|parent| !ticked[parent]) {
                    write.keys.insert(node.key.clone(), Vec::new());
                }
            } else {
                for key in &node.keys {
                    write.keys.insert(key.clone(), self.keys[key].clone());
                }
            }
        }
        (write, kept)
    }
}

/// One key of [`RegistryExport::tree`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyNode {
    /// The full key path, as the export first spells it.
    pub key: String,
    /// The export's spellings of the key, which differ only in case; empty for an added node.
    pub keys: Vec<String>,
    /// How many nodes lie above it.
    pub depth: usize,
    /// The index past its subtree: the nodes below it are those before `end`.
    pub end: usize,
}

/// What owns a key of the target in a replace (ADR-0026).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// A written key: the target key loses its values, and goes unless it holds a kept key.
    Write,
    /// A kept key: the target key stays as it is.
    Keep,
    /// No key of the replace: the target key stays as it is.
    None,
}

/// The written and kept keys of a replace, lowercased once.
pub(crate) struct Owners {
    write: Vec<String>,
    kept: Vec<String>,
}

impl Owners {
    /// The owners of a replace that writes `write` and keeps `kept`, paths relative to HKCU.
    /// Refuses a key in both, ignoring case, with [`RegistryError::KeptAndWritten`].
    pub(crate) fn new(write: &[&str], kept: &[&str]) -> Result<Owners, RegistryError> {
        let lower = |keys: &[&str]| {
            keys.iter()
                .map(|key| key.to_lowercase())
                .collect::<Vec<_>>()
        };
        let owners = Owners {
            write: lower(write),
            kept: lower(kept),
        };
        if let Some(both) = (owners.kept.iter()).position(|key| owners.write.contains(key)) {
            let key = format!("{HKCU}\\{}", kept[both]);
            return Err(RegistryError::KeptAndWritten { key });
        }
        Ok(owners)
    }

    /// The owner of `path`: the deepest written or kept key it is at or below.
    pub(crate) fn owner(&self, path: &str) -> Owner {
        let path = path.to_lowercase();
        let deepest = |keys: &[String]| {
            (keys.iter())
                .filter(|key| path == **key || lowered_below(&path, key))
                .map(String::len)
                .max()
        };
        match (deepest(&self.write), deepest(&self.kept)) {
            (Some(write), Some(kept)) if kept > write => Owner::Keep,
            (Some(_), _) => Owner::Write,
            (None, Some(_)) => Owner::Keep,
            (None, None) => Owner::None,
        }
    }

    /// Whether a kept key lies strictly below `path`.
    #[cfg(windows)]
    pub(crate) fn kept_below(&self, path: &str) -> bool {
        let path = path.to_lowercase();
        self.kept.iter().any(|key| lowered_below(key, &path))
    }
}

/// Whether the `\`-separated key `path` is `root` or below it, comparing whole elements
/// case-insensitively.
pub(crate) fn is_under(path: &str, root: &str) -> bool {
    // Unicode lowercase approximates Wine's uppercase table; exotic names may differ.
    let (path, root) = (path.to_lowercase(), root.to_lowercase());
    path == root || lowered_below(&path, &root)
}

/// Whether the lowercased key `path` lies strictly below the lowercased key `root`.
pub(crate) fn lowered_below(path: &str, root: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_every_kind() {
        let value = |name: &str, data| RegValue {
            name: name.into(),
            data,
        };
        let mut export = RegistryExport::default();
        export.keys.insert(
            format!("{HKCU}\\Software\\Foo"),
            vec![
                value("", RegData::Sz("a".into())),
                value("e", RegData::ExpandSz("%TEMP%".into())),
                value("m", RegData::MultiSz(vec!["x".into(), "y".into()])),
                value("d", RegData::Dword(7)),
                value("q", RegData::Qword(u64::MAX)),
                value("b", RegData::Binary(vec![1, 2])),
                value(
                    "o",
                    RegData::Other {
                        code: 4,
                        bytes: vec![0; 8],
                    },
                ),
            ],
        );
        let json = serde_json::to_string(&export).unwrap();
        assert_eq!(
            serde_json::from_str::<RegistryExport>(&json).unwrap(),
            export
        );
        assert!(json.contains(r#""kind":"multi_sz""#));
    }

    #[test]
    fn protected_key_ignores_case_and_separators() {
        for key in [
            "HKEY_CURRENT_USER/Software/Microsoft/Windows/CurrentVersion/Run",
            r"hkey_current_user\SOFTWARE\microsoft\windows\CurrentVersion\Run\",
            "HKEY_CURRENT_USER/Software/Microsoft/Windows NT/CurrentVersion/Winlogon",
            "HKEY_CURRENT_USER/Software/Microsoft/Command Processor",
            "HKEY_CURRENT_USER/Software/Microsoft",
            "HKEY_CURRENT_USER/Software/Classes/exefile",
            "HKEY_CURRENT_USER/Software/Classes",
            "HKEY_CURRENT_USER/Software/Policies/Microsoft",
            "HKEY_CURRENT_USER/Software/Wow6432Node/Microsoft/Windows/CurrentVersion/Run",
            "HKEY_CURRENT_USER/Software/Wow6432Node",
            "HKEY_CURRENT_USER/Environment",
            "HKEY_CURRENT_USER/Control Panel/Desktop",
            "HKEY_CURRENT_USER/Software",
            "HKEY_CURRENT_USER",
            "HKEY_CURRENT_USER/",
            "HKEY_LOCAL_MACHINE/Software/Foo",
            "Software/Foo",
        ] {
            assert!(protected_key(key), "{key}");
        }
        for key in [
            "HKEY_CURRENT_USER/Software/Foo",
            "hkey_current_user/software/team cherry/hollow knight",
            r"HKEY_CURRENT_USER\Software\Microsoft\Foo",
            "HKEY_CURRENT_USER/Software/Classes/VirtualStore/MACHINE/SOFTWARE/Foo",
            "HKEY_CURRENT_USER/Software/Wow6432Node/Foo",
            "HKEY_CURRENT_USER/Software/Windows Foo",
        ] {
            assert!(!protected_key(key), "{key}");
        }
    }

    fn full(key: &str) -> String {
        format!("{HKCU}\\{key}")
    }

    /// An export of the given keys, each with one value naming its spelling.
    fn export_of(keys: &[&str]) -> RegistryExport {
        let mut export = RegistryExport::default();
        for key in keys {
            let value = RegValue::new("v".into(), RegData::Sz((*key).into()));
            export.keys.insert(full(key), vec![value]);
        }
        export
    }

    /// Each node of `tree` as `(key relative to HKCU, added, depth, end)`.
    fn shape(tree: &[KeyNode]) -> Vec<(String, bool, usize, usize)> {
        (tree.iter())
            .map(|node| {
                let key = node.key.strip_prefix(&full("")).unwrap().to_owned();
                (key, node.keys.is_empty(), node.depth, node.end)
            })
            .collect()
    }

    /// Two roots under a key the export lacks, and a root with a gap of two keys below it.
    const GAPPED: [&str; 4] = [
        r"Software\Bar\X",
        r"Software\Bar\Y",
        r"Software\Foo",
        r"Software\Foo\A\B\C",
    ];

    #[test]
    fn tree_fills_only_between_a_key_and_its_nearest_ancestor() {
        let tree = export_of(&GAPPED).tree();

        let want = [
            (r"Software\Bar\X", false, 0, 1),
            (r"Software\Bar\Y", false, 0, 2),
            (r"Software\Foo", false, 0, 6),
            (r"Software\Foo\A", true, 1, 6),
            (r"Software\Foo\A\B", true, 2, 6),
            (r"Software\Foo\A\B\C", false, 3, 6),
        ];
        let want: Vec<_> = (want.iter())
            .map(|&(key, added, depth, end)| (key.to_owned(), added, depth, end))
            .collect();
        assert_eq!(shape(&tree), want);
    }

    #[test]
    fn tree_merges_case_variants_into_one_node() {
        let export = export_of(&[r"Software\co", r"Software\Co", r"Software\CO\x"]);

        let tree = export.tree();

        let want = [
            (r"Software\Co", false, 0, 2),
            (r"Software\CO\x", false, 1, 2),
        ];
        let want: Vec<_> = (want.iter())
            .map(|&(key, added, depth, end)| (key.to_owned(), added, depth, end))
            .collect();
        assert_eq!(shape(&tree), want);
        assert_eq!(tree[0].keys, [full(r"Software\Co"), full(r"Software\co")]);
    }

    #[test]
    fn tree_ranges_span_each_subtree() {
        // Siblings sharing a text prefix, nested keys and case variants, out of map order.
        let export = export_of(&[
            r"Software\Co",
            r"Software\Co\Game",
            r"Software\Com",
            r"Software\Other\Deep",
            r"software\other",
            r"Software\Other\Deep\Er",
            r"Software\Other\Side",
        ]);

        let tree = export.tree();

        let order: Vec<_> = shape(&tree).into_iter().map(|(key, ..)| key).collect();
        let want = [
            r"Software\Co",
            r"Software\Co\Game",
            r"Software\Com",
            r"software\other",
            r"Software\Other\Deep",
            r"Software\Other\Deep\Er",
            r"Software\Other\Side",
        ];
        assert_eq!(order, want);
        for (at, node) in tree.iter().enumerate() {
            for (other_at, other) in tree.iter().enumerate() {
                let below = other_at != at && is_under(&other.key, &node.key);
                assert_eq!(
                    (at + 1..node.end).contains(&other_at),
                    below,
                    "{at} {other_at}"
                );
            }
        }
    }

    #[test]
    fn select_all_ticked_writes_the_export_and_keeps_nothing() {
        let export = export_of(&GAPPED);
        let tree = export.tree();

        let (write, kept) = export.select(&tree, &vec![true; tree.len()]);

        assert_eq!(write, export, "no added node under a ticked parent");
        assert!(kept.is_empty());
    }

    #[test]
    fn select_unticked_node_is_kept_and_not_written() {
        let export = export_of(&[r"Software\Co", r"Software\co", r"Software\Co\Game"]);
        let tree = export.tree();

        let (write, kept) = export.select(&tree, &[false, true]);

        assert_eq!(write, export_of(&[r"Software\Co\Game"]));
        assert_eq!(kept, [full(r"Software\Co")]);
    }

    #[test]
    fn select_ticked_added_node_is_written_without_values() {
        let export = export_of(&GAPPED);
        let tree = export.tree();

        // `Software\Foo` unticked; its added `A` and `A\B` ticked.
        let (write, kept) = export.select(&tree, &[true, true, false, true, true, true]);

        let mut want = export_of(&[r"Software\Bar\X", r"Software\Bar\Y", r"Software\Foo\A\B\C"]);
        want.keys.insert(full(r"Software\Foo\A"), Vec::new());
        assert_eq!(
            write, want,
            "only the added node right below the unticked one"
        );
        assert_eq!(kept, [full(r"Software\Foo")]);
    }
}
