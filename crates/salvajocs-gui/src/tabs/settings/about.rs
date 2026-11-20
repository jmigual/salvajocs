//! The About section of the Settings tab: which build runs, for bug reports. Its values come from
//! `build.rs` and Cargo, and name no machine or path.

use eframe::egui::Ui;

use crate::theme;
use crate::widgets::form_grid;

#[cfg(test)]
#[path = "../../../build/vcs_info.rs"]
mod vcs_info;

/// The release notes at the tag of the running version.
const RELEASE_NOTES: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/blob/v",
    env!("CARGO_PKG_VERSION"),
    "/CHANGELOG.md"
);

/// The label and value of each line, in the order shown. A value `build.rs` could not find is
/// left out, except the commit, which reads "unknown".
fn rows() -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
        (
            "Commit",
            option_env!("SALVAJOCS_COMMIT")
                .unwrap_or("unknown")
                .to_owned(),
        ),
    ];
    if let Some(date) = option_env!("SALVAJOCS_COMMIT_DATE") {
        rows.push(("Commit date", date.to_owned()));
    }
    rows.push((
        "Target",
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    ));
    if let Some(rustc) = option_env!("SALVAJOCS_RUSTC") {
        rows.push(("Rust compiler", rustc.to_owned()));
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    rows.push(("Build profile", profile.to_owned()));
    if let Some(version) = option_env!("SALVAJOCS_RUSTIC_CORE") {
        rows.push(("Storage library", format!("rustic_core {version}")));
    }
    rows
}

/// The lines of `rows` as plain text, for a bug report.
fn report() -> String {
    rows()
        .into_iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Draws the section.
pub(super) fn ui(ui: &mut Ui) {
    ui.heading("About");
    theme::group(ui, |ui| {
        form_grid("about").show(ui, |ui| {
            for (label, value) in rows() {
                ui.label(label);
                ui.label(value);
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(report());
            }
            ui.hyperlink_to("Release notes", RELEASE_NOTES);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_has_a_line_per_row_starting_with_the_version() {
        let report = report();
        assert_eq!(report.lines().count(), rows().len());
        assert_eq!(
            report.lines().next(),
            Some(concat!("Version: ", env!("CARGO_PKG_VERSION")))
        );
    }
}
