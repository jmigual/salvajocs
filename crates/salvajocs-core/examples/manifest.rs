//! Parses a manifest and prints each game's slug, Steam id and save path templates.

use salvajocs_core::{manifest, slug::slug};

const YAML: &str = r#"
Example Game:
  files:
    "<winAppData>/Example Game/saves":
      tags: [save]
  steam:
    id: 1234
"#;

fn main() -> Result<(), manifest::ManifestError> {
    let manifest = manifest::parse_yaml(YAML)?;
    for (title, game) in &manifest.games {
        let templates: Vec<_> = game.files.keys().collect();
        let steam_id = game.steam.as_ref().and_then(|steam| steam.id);
        println!(
            "{title} ({}): steam {steam_id:?}, {templates:?}",
            slug(title)
        );
    }
    Ok(())
}
