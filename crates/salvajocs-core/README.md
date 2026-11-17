# salvajocs-core

The library behind [salvajocs](https://github.com/jmigual/salvajocs), a game-save backup app for
Linux and Windows. It holds everything that is not drawing:

- `manifest`: parse the Ludusavi game manifest and merge custom games into it.
- `scan`, `prefix`, `layout`: find save files on this machine, including Wine/Proton prefixes.
- `registry`: read and write Windows registry keys, natively or in a Wine prefix.
- `backup`, `restore`, `storage`: back up into a restic-format repository (via `rustic_core`) and
  restore from it.
- `crossbuild`: map a save between Linux and Windows roots, for cross-OS restore.

```rust
use salvajocs_core::{manifest, slug::slug};

let yaml = "Example Game:\n  files:\n    \"<winAppData>/Example Game\": {tags: [save]}\n";
let manifest = manifest::parse_yaml(yaml)?;
for title in manifest.games.keys() {
    println!("{title} -> {}", slug(title));
}
# Ok::<(), manifest::ManifestError>(())
```

A runnable version is in [`examples/manifest.rs`](examples/manifest.rs). API reference:
[docs.rs/salvajocs-core](https://docs.rs/salvajocs-core).

The API is 0.x and can change between minor versions.

Source, issues and the user guide: <https://github.com/jmigual/salvajocs>. Licensed under MIT.
