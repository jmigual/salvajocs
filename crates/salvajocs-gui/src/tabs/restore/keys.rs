//! The registry keys of a restore plan as a tree with a checkbox on each key (ADR-0026).

use std::iter;
use std::ops::Range;

use eframe::egui::collapsing_header::CollapsingState;
use eframe::egui::{Checkbox, Role, Ui, WidgetInfo};
use salvajocs_core::registry::{KeyNode, RegistryExport};
use salvajocs_core::restore::RestorePlan;

use crate::tabs::count;

/// The keys of a plan's registry export with a tick on each, all ticked at first. Unlike
/// `FileTree`, which draws a selection the Backup tab saves, it holds its ticks: they belong to
/// one plan and are never saved.
pub(super) struct KeyTree {
    nodes: Vec<KeyNode>,
    ticked: Vec<bool>,
    /// `before[i]` is how many of `nodes[..i]` are ticked, so a subtree's count is a difference.
    before: Vec<usize>,
}

/// What a click in a [`KeyTree`] ticks or unticks: a range of its nodes.
type Toggle = Option<(Range<usize>, bool)>;

impl KeyTree {
    /// The tree of `plan`'s registry export, everything ticked; empty without one. It compares
    /// every key, so it is built once per plan.
    pub(super) fn of(plan: &RestorePlan) -> KeyTree {
        let nodes = (plan.registry.as_ref()).map_or_else(Vec::new, |(export, _)| export.tree());
        let mut tree = KeyTree {
            ticked: vec![true; nodes.len()],
            nodes,
            before: Vec::new(),
        };
        tree.count();
        tree
    }

    fn count(&mut self) {
        let mut ticked = 0;
        self.before = iter::once(0)
            .chain(self.ticked.iter().map(|&tick| {
                ticked += usize::from(tick);
                ticked
            }))
            .collect();
    }

    /// How many keys are ticked.
    pub(super) fn ticked_count(&self) -> usize {
        self.before[self.nodes.len()]
    }

    /// Whether any key is ticked.
    pub(super) fn any_ticked(&self) -> bool {
        self.ticked_count() > 0
    }

    /// What a restore of `export`, the export this tree was built from, writes and keeps with
    /// these ticks, as `RegistryExport::select` gives them.
    pub(super) fn select(&self, export: &RegistryExport) -> (RegistryExport, Vec<String>) {
        export.select(&self.nodes, &self.ticked)
    }

    /// Draws a checkbox per key: a key with subkeys is a folder, closed at first, whose
    /// "(values)" row ticks the key alone. `enabled` false greys the checkboxes. The caller
    /// scopes the ids (`ui.push_id`) when it draws several trees.
    pub(super) fn ui(&mut self, ui: &mut Ui, enabled: bool) {
        let mut toggle = None;
        self.children(ui, 0..self.nodes.len(), enabled, &mut toggle);
        if let Some((range, ticked)) = toggle {
            self.ticked[range].fill(ticked);
            self.count();
        }
    }

    /// The nodes of `range` that have no parent in it, each with its subtree.
    fn children(&self, ui: &mut Ui, range: Range<usize>, enabled: bool, toggle: &mut Toggle) {
        let mut at = range.start;
        while at < range.end {
            self.node(ui, at, enabled, toggle);
            at = self.nodes[at].end;
        }
    }

    fn node(&self, ui: &mut Ui, at: usize, enabled: bool, toggle: &mut Toggle) {
        let node = &self.nodes[at];
        let label = if node.depth == 0 {
            &node.key
        } else {
            node.key.rsplit('\\').next().unwrap_or_default()
        };
        if node.end == at + 1 {
            let mut checked = self.ticked[at];
            if (ui.add_enabled(enabled, Checkbox::new(&mut checked, label))).changed() {
                *toggle = Some((at..at + 1, checked));
            }
            return;
        }
        let (ticked, all) = (self.before[node.end] - self.before[at], node.end - at);
        let id = ui.make_persistent_id(&node.key);
        let state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
        let verb = if state.is_open() { "Close" } else { "Open" };
        let (arrow, ..) = state
            .show_header(ui, |ui| {
                // A mixed folder is drawn as unchecked, so a click on it ticks everything.
                let mut checked = ticked == all;
                let mixed = ticked != 0 && !checked;
                let checkbox = Checkbox::new(&mut checked, label).indeterminate(mixed);
                if ui.add_enabled(enabled, checkbox).changed() {
                    *toggle = Some((at..node.end, checked));
                }
                ui.weak(format!("{ticked} of {}", count(all, "key")));
            })
            .body(|ui| {
                let mut checked = self.ticked[at];
                let response = ui.add_enabled(enabled, Checkbox::new(&mut checked, "(values)"));
                if response.changed() {
                    *toggle = Some((at..at + 1, checked));
                }
                // The full key: every open folder has a "(values)" row.
                let name = format!("Values of {}", node.key);
                response
                    .widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, checked, &name));
                self.children(ui, at + 1..node.end, enabled, toggle);
            });
        // The arrow is a bare interaction, so name it for screen readers.
        arrow.widget_info(|| {
            WidgetInfo::labeled(
                Role::DisclosureTriangle,
                ui.is_enabled(),
                format!("{verb} {label}"),
            )
        });
    }
}

#[cfg(test)]
mod tests;
