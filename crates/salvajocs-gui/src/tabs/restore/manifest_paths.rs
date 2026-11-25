//! The dialog behind the plan's **Manifest paths** button: the save path templates and registry
//! keys the manifest, or the custom game, gives the game.

use eframe::egui::{self, RichText, ScrollArea, Ui};
use salvajocs_core::manifest::{CustomGame, Integration, Manifest, Os, Store, Tag, When};

use super::RestoreTab;
use crate::theme;
use crate::widgets::copyable;

/// Where a game's entry comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// The Ludusavi manifest alone.
    Manifest,
    /// The manifest entry with a custom game extending it.
    Extended,
    /// A custom game replacing the manifest entry.
    Overridden,
    /// A custom game of a title the manifest lacks.
    Custom,
}

impl Source {
    /// Where the entry of `title` comes from, or `None` when neither the manifest nor a custom
    /// game has the title. As `manifest::merge_custom` applies them, any override replaces the
    /// manifest entry and otherwise any extension adds to it.
    pub(super) fn of(title: &str, manifest: &Manifest, custom: &[CustomGame]) -> Option<Self> {
        let mut modes = (custom.iter())
            .filter(|game| game.name == title)
            .map(|game| game.integration)
            .peekable();
        let any = modes.peek().is_some();
        let overridden = modes.any(|mode| mode == Integration::Override);
        match (manifest.games.contains_key(title), any) {
            (true, false) => Some(Self::Manifest),
            (true, true) if overridden => Some(Self::Overridden),
            (true, true) => Some(Self::Extended),
            (false, true) => Some(Self::Custom),
            (false, false) => None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Manifest => "From the Ludusavi manifest.",
            Self::Extended => "From the Ludusavi manifest, extended by your custom game.",
            Self::Overridden => "Your custom game, which overrides the manifest entry.",
            Self::Custom => "Custom game.",
        }
    }
}

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "Windows",
        Os::Linux => "Linux",
        Os::Mac => "macOS",
        Os::Dos => "DOS",
        _ => "another system",
    }
}

fn store_name(store: Store) -> &'static str {
    match store {
        Store::Steam => "Steam",
        Store::Gog => "GOG",
        Store::Epic => "Epic Games Store",
        Store::Microsoft => "Microsoft Store",
        Store::Uplay => "Ubisoft Connect",
        _ => "another store",
    }
}

/// The weak line under a template or key: where it applies, then what it holds. `None` when the
/// rule has no conditions and no tags. A `when` that sets neither OS nor store matches anything,
/// so a rule with one has no "Applies to" part.
fn conditions(whens: &[When], tags: &[Tag]) -> Option<String> {
    let whens: Vec<String> = if whens
        .iter()
        .any(|when| when.os.is_none() && when.store.is_none())
    {
        Vec::new()
    } else {
        (whens.iter())
            .map(|when| {
                let parts: Vec<_> = (when.os.map(os_name).into_iter())
                    .chain(when.store.map(store_name))
                    .collect();
                parts.join(" with ")
            })
            .collect()
    };
    let tags: Vec<_> = tags
        .iter()
        .map(|tag| match tag {
            Tag::Save => "save",
            Tag::Config => "config",
            _ => "other",
        })
        .collect();
    let mut parts = Vec::new();
    if !whens.is_empty() {
        parts.push(format!("Applies to {}", whens.join(" or ")));
    }
    if !tags.is_empty() {
        parts.push(format!("Holds {}", tags.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// A heading and one copyable line per rule, with its conditions in weak text under it.
fn rules_ui<'a>(
    ui: &mut Ui,
    heading: &str,
    rules: impl ExactSizeIterator<Item = (&'a String, &'a [When], &'a [Tag])>,
) {
    if rules.len() == 0 {
        return;
    }
    ui.add_space(8.0);
    ui.strong(format!("{heading} ({})", rules.len()));
    for (path, whens, tags) in rules {
        copyable(ui, RichText::new(path).monospace(), path);
        if let Some(line) = conditions(whens, tags) {
            ui.weak(line);
        }
    }
}

impl RestoreTab {
    /// The dialog listing the save paths of the game whose plan the **Manifest paths** button
    /// opened it for; closes it on **Close** or Escape.
    pub(super) fn manifest_paths_ui(&mut self, ctx: &egui::Context) {
        let Some(fetched) = &self.manifest_paths else {
            return;
        };
        let mut closed = false;
        let ((), dismissed) =
            theme::modal_dismissible(ctx, "restore_manifest_paths", 720.0, |ui| {
                ui.heading(format!("Save paths of {}", fetched.title));
                match (fetched.source, &fetched.info.entry) {
                    (Some(source), Some(entry)) => {
                        ui.weak(source.text());
                        ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            let files = (entry.files.iter())
                                .map(|(path, rule)| (path, &rule.when[..], &rule.tags[..]));
                            rules_ui(ui, "Files", files);
                            let keys = (entry.registry.iter())
                                .map(|(key, rule)| (key, &rule.when[..], &rule.tags[..]));
                            rules_ui(ui, "Registry", keys);
                            if entry.files.is_empty() && entry.registry.is_empty() {
                                ui.label("The entry lists no paths.");
                            }
                        });
                    }
                    _ => {
                        ui.label("No manifest entry for this game.");
                    }
                }
                theme::row_end(ui, |ui| closed |= ui.button("Close").clicked());
            });
        if closed || dismissed {
            self.manifest_paths = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use salvajocs_core::manifest::GameEntry;

    use super::*;

    fn manifest(title: &str) -> Manifest {
        let mut manifest = Manifest::default();
        manifest.games.insert(title.into(), GameEntry::default());
        manifest
    }

    fn custom(title: &str, integration: Integration) -> CustomGame {
        CustomGame::new(title.into(), GameEntry::default(), integration)
    }

    fn when(os: Option<Os>, store: Option<Store>) -> When {
        let mut when = When::default();
        when.os = os;
        when.store = store;
        when
    }

    #[test]
    fn a_title_only_the_manifest_has_comes_from_the_manifest() {
        let source = Source::of(
            "Mix",
            &manifest("Mix"),
            &[custom("Other", Integration::Override)],
        );
        assert_eq!(source, Some(Source::Manifest));
    }

    #[test]
    fn an_extending_custom_game_extends_the_manifest_entry() {
        let custom = [custom("Mix", Integration::Extend)];
        let source = Source::of("Mix", &manifest("Mix"), &custom);
        assert_eq!(source, Some(Source::Extended));
    }

    #[test]
    fn an_overriding_custom_game_replaces_the_manifest_entry() {
        let custom = [custom("Mix", Integration::Override)];
        let source = Source::of("Mix", &manifest("Mix"), &custom);
        assert_eq!(source, Some(Source::Overridden));
    }

    #[test]
    fn a_custom_game_of_a_title_the_manifest_lacks_is_custom() {
        let custom = [custom("Mix", Integration::Extend)];
        let source = Source::of("Mix", &Manifest::default(), &custom);
        assert_eq!(source, Some(Source::Custom));
    }

    #[test]
    fn a_title_nobody_has_has_no_source() {
        let source = Source::of(
            "Mix",
            &manifest("Other"),
            &[custom("Other", Integration::Extend)],
        );
        assert_eq!(source, None);
    }

    #[test]
    fn an_override_wins_over_an_extension_in_either_order() {
        let (over, extend) = (
            custom("Mix", Integration::Override),
            custom("Mix", Integration::Extend),
        );
        for custom in [[over.clone(), extend.clone()], [extend, over]] {
            let source = Source::of("Mix", &manifest("Mix"), &custom);
            assert_eq!(source, Some(Source::Overridden));
        }
    }

    #[test]
    fn a_when_names_its_os_with_its_store() {
        let whens = [when(Some(Os::Windows), Some(Store::Steam))];
        assert_eq!(
            conditions(&whens, &[Tag::Save]).as_deref(),
            Some("Applies to Windows with Steam. Holds save")
        );
    }

    #[test]
    fn several_whens_are_alternatives() {
        let whens = [
            when(Some(Os::Windows), Some(Store::Steam)),
            when(Some(Os::Linux), None),
        ];
        assert_eq!(
            conditions(&whens, &[]).as_deref(),
            Some("Applies to Windows with Steam or Linux")
        );
    }

    #[test]
    fn a_when_that_matches_anything_leaves_out_the_conditions() {
        let whens = [when(Some(Os::Windows), None), when(None, None)];
        assert_eq!(
            conditions(&whens, &[Tag::Config]).as_deref(),
            Some("Holds config")
        );
        assert_eq!(conditions(&whens, &[]), None);
    }

    #[test]
    fn a_rule_without_whens_or_tags_has_no_line() {
        assert_eq!(conditions(&[], &[]), None);
    }
}
