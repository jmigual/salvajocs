# salvajocs

Back up and restore game saves into a restic-format repository, using the embedded
[rustic](https://rustic.cli.rs) library. The repository can be a local folder or any backend
rustic supports. Save locations come from the
[ludusavi manifest](https://github.com/mtkennerly/ludusavi-manifest). A save backed up from a
Wine/Proton prefix on Linux restores on native Windows and the other way round. Saves from a
native Linux build can be mapped onto the Windows build too.

Design decisions: [docs/adrs/README.md](docs/adrs/README.md) (index; read it before opening an
ADR). Implementation plan: [docs/plan.md](docs/plan.md).
