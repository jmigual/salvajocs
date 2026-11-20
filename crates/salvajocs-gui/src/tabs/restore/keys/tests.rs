use std::path::PathBuf;

use eframe::egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use salvajocs_core::layout::{Context, WinFolders};
use salvajocs_core::registry::{RegData, RegValue};
use salvajocs_core::selection::GameSettings;

use super::super::ticked_plan;
use super::*;
use crate::theme;

const REG: &str = r"HKEY_CURRENT_USER\Software\Co\Reg";
const OTHER: &str = r"HKEY_CURRENT_USER\Software\Co\Other";

/// `Reg` holding `Slot1` and `Slot2`, which holds `Deep`, and `Other`, each with a value.
fn export() -> RegistryExport {
    let mut export = RegistryExport::default();
    for key in [REG, OTHER].into_iter().map(String::from).chain(
        ["Slot1", "Slot2", r"Slot2\Deep"]
            .into_iter()
            .map(|child| format!(r"{REG}\{child}")),
    ) {
        let value = RegValue::new("v".into(), RegData::Dword(1));
        export.keys.insert(key, vec![value]);
    }
    export
}

fn plan() -> RestorePlan {
    let prefix = PathBuf::from("pfx");
    let folders = WinFolders::for_prefix(&prefix, "me");
    let registry = Some((export(), Context::WinePrefix { prefix, folders }));
    RestorePlan::new(Vec::new(), registry, Vec::new(), false, Vec::new())
}

/// The tree of [`plan`], drawn enabled, with `Reg` open.
fn opened() -> Harness<'static, KeyTree> {
    let mut harness = theme::harness(
        |ui, tree: &mut KeyTree| tree.ui(ui, true),
        KeyTree::of(&plan()),
    );
    harness.step();
    click(&mut harness, &format!("Open {REG}"));
    harness
}

fn click(harness: &mut Harness<'static, KeyTree>, label: &str) {
    harness.get_by_label(label).click();
    harness.step();
    harness.step();
}

fn toggled(harness: &Harness<'static, KeyTree>, label: &str) -> Option<Toggled> {
    harness.get_by_label(label).accesskit_node().toggled()
}

#[test]
fn a_folder_ticks_and_unticks_its_whole_subtree() {
    let mut harness = opened();

    click(&mut harness, REG);

    assert_eq!(harness.state().ticked_count(), 1, "only Other");
    for label in ["Slot1", "Slot2", &format!("Values of {REG}")] {
        assert_eq!(toggled(&harness, label), Some(Toggled::False), "{label}");
    }
    assert_eq!(toggled(&harness, OTHER), Some(Toggled::True));
    click(&mut harness, REG);
    assert_eq!(harness.state().ticked_count(), 5);
}

#[test]
fn the_values_row_ticks_only_its_own_key() {
    let mut harness = opened();

    click(&mut harness, &format!("Values of {REG}"));

    let tree = harness.state();
    assert_eq!(tree.ticked_count(), 4);
    let (write, kept) = tree.select(&export());
    assert_eq!(kept, [REG]);
    assert!(!write.keys.contains_key(REG));
    assert_eq!(write.keys.len(), 4);
    assert_eq!(toggled(&harness, "Slot1"), Some(Toggled::True));
}

#[test]
fn a_folder_counts_its_ticked_keys() {
    let mut harness = opened();
    assert!(harness.query_by_label("4 of 4 keys").is_some());

    click(&mut harness, "Slot1");

    assert!(harness.query_by_label("3 of 4 keys").is_some());
    assert_eq!(toggled(&harness, REG), Some(Toggled::Mixed));
    assert!(harness.state().any_ticked());
}

#[test]
fn the_ticked_plan_writes_and_keeps_what_select_gives() {
    let plan = plan();
    let mut tree = KeyTree::of(&plan);
    let slot2 = (tree.nodes.iter())
        .position(|node| node.key.ends_with("Slot2"))
        .unwrap();
    tree.ticked[slot2] = false;
    tree.count();

    let ticked = ticked_plan(&plan, &[], &tree, &GameSettings::default());

    let (write, kept) = export().select(&tree.nodes, &tree.ticked);
    assert_eq!(kept, [format!(r"{REG}\Slot2")]);
    assert_eq!(ticked.registry.map(|(export, _)| export), Some(write));
    assert_eq!(ticked.registry_kept, kept);

    // Nothing written: no registry, so nothing kept either.
    tree.ticked.fill(false);
    tree.count();
    let ticked = ticked_plan(&plan, &[], &tree, &GameSettings::default());
    assert_eq!(ticked.registry, None);
    assert!(ticked.registry_kept.is_empty());
}
