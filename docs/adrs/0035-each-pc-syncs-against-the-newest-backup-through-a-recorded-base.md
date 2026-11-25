# ADR-0035: Each PC syncs against the newest backup through a recorded base

**Status:** Accepted (the user's OK, 2026-11-25)
**Date:** 2026-11-25

## Context

The user wants two things from salvajocs: "Backup games" (every version stays one to go back to
until the Old backups rules remove it) and "Sync PCs" (each game has one history, ordered by
time and shared by every PC; its newest backup is "the saves").

Until 0.10.0 a game makes no snapshot when (A) rustic's `skip_if_unchanged` finds the staged
tree equal to this host's latest snapshot (ADR-0010), or (B) its staged hash list equals the
newest backup's from any host (ADR-0018, mapped by ADR-0021). Rule (A) keeps a per-host line of
history. The reported case: PC A backs up A1, JOAN-XPS-9730 backs up other saves J1, and A's
saves do not change. The preview counts the differences from J1 but shows a green check
("unchanged since this PC's last backup"), Back up skips by (A) and reports "up to date with
this PC's last backup". A is in fact behind: J1 is newer, nothing offers it, and A goes on as a
parallel branch.

## Decision

- **Three things, compared by content** with the existing rules (ADR-0018, mapped for the other
  build by ADR-0021; no new comparison):
  - **L**: this PC's saves now (the scan in the preview, the staged hash list at Back up);
  - **N**: the game's newest backup from any PC, not `pre-restore` (`changes::newest`);
  - **B**: this PC's **base**, the backup it last synced with: what it last backed up, last
    found equal to the newest, or last restored.
- **States** (`sync::State`):

  | State | Condition | Back up does |
  | --- | --- | --- |
  | No backup yet | no N | backs up; base := it |
  | Up to date | L = N | nothing; base := N |
  | Ahead | L ≠ N, N = B | backs up (the new newest); base := it |
  | Behind | L = B, N ≠ B | nothing, unless Back up anyway; base unchanged; the preview offers the restore |
  | Changed on both | L ≠ N, L ≠ B, N ≠ B | asks |
  | Unknown base | no B, L ≠ N | asks, as changed on both |

  Behind never backs up by itself (only by Back up anyway, below): storing L would make older
  saves the newest and put every other PC behind them, a silent revert. Changed on both and
  unknown base ask, since a silent backup is "last writer wins" and a silent skip leaves this
  PC's changes in no backup.
- **Equalities.** L = N is rule (B) at Back up, and in the preview counts all zero with no
  `skip_blocker`; a newest without a verified list is never equal. N = B is the same snapshot
  id, else the base's hash list against N (`Baseline::same_content`). L = B is the base's hash
  list against L, else, when the base has no list, B's snapshot read through the `Mapper` and
  compared as N; a base without a list whose snapshot is gone is not equal. A newest that Back
  up cannot read counts as one without a list, and a base snapshot it cannot read as gone: Back
  up cannot confirm an equality then, so it asks (or stores, when ahead); nothing is lost. The
  preview, which reads neither the registry nor `salvajocs.json`, finds L = B only when neither
  side has registry keys and the scan would stage the same `salvajocs.json`, so it never
  predicts behind for a game Back up asks about.
- **One pure function decides**: `sync::state(saves, newest, base, base_backup)` in
  `core::sync`, called by the preview (`Saves::Scanned`) and by `backup_games`
  (`Saves::Staged`), with the same rules. A scan does not read the registry or
  `salvajocs.json`, so the two can differ; Back up's decision then wins and its results line
  says so, never silently. The preview never predicts behind for a game Back up asks about, but
  against a newest of the same build it can read up to date when the registry or
  `salvajocs.json` differs and Back up decides otherwise. It reads nothing from the
  repository; `Base::to_read` names the base snapshot a caller must read for it, only when
  L ≠ N, so an up-to-date game reads nothing more.
- **The base record** is local, per PC: `<config_dir>/sync-base.json`, keyed by the
  repository's `AppConfig::secret_scope()`, then by game slug:

  ```json
  { "format": 1,
    "repositories": { "<secret_scope>": {
      "<slug>": { "snapshot": "<64 hex>", "saves": { …hashes.json object… } } } } }
  ```

  `saves` is the staged `hashes.json` list when a Back up set the base, so a base snapshot
  forgotten by the rules still compares; it is absent when a restore set it. `sync::load` reads
  a file that is missing, cannot be read, does not parse or is of a newer format as empty (the
  worst case is an unknown base, which asks). `sync::record` rewrites it atomically
  (`fsutil::write_atomic`) from a strict read: a missing file starts empty and one that does not
  parse is replaced, but one that cannot be read (an antivirus lock, a permission) or is of a
  newer format fails the record and is left as it was, since a rewrite would drop every other
  game's base. The GUI jobs call it while they hold the session's storage lock.
- **Set by:** a Back up that stored a snapshot → `{ new id, staged }`; a Back up that found
  L = N → `{ N, staged }`; behind or "don't back up" → unchanged; a restore of S that wrote every
  planned write (ticks allowed) → `{ S, none }`; a restore with a failed write or a failed
  safety backup → unchanged; the preview never writes. A ticked restore sets the base to S on
  purpose: the unticked files are this PC's choice, so the next Back up stores the mix.
  A restore of a `pre-restore` safety backup (an undo) is a restore like any other: it records
  the safety backup as the base, so the game reads behind (the newest regular backup is not the
  base) and Back up asks only after the user changes the saves (2026-11-25, approved by the
  user). An earlier rule cleared the record on an undo so the next Back up asked; it is
  replaced.
- **Seed** (upgrade, or a lost file): with no record, the base is this PC's own newest regular
  snapshot of the game, by hostname, without a list. It is used, never written, so the reported
  case reads "behind" on the first preview after the upgrade. A seed never makes a game ahead:
  when it is the newest and L ≠ N, the state is unknown base, so Back up asks. PCs can share a
  hostname (every Steam Deck is `steamdeck`), and their seed would then be another PC's backup,
  read as this PC's own; storing L silently would bury that PC's saves (user decision,
  2026-11-25: nothing is lost silently).
- **Rustic's `skip_if_unchanged` is off for regular backups** and kept for the `pre-restore`
  safety backup. The state decides; rustic only stores (it still reads unchanged files from the
  parent). With it on, a game the state says to store would be skipped whenever the staging
  tree equals this host's last snapshot, and reported up to date.
- **User decisions (2026-11-25):**
  - Changed on both: Back up asks in its dialog (ADR-0029's), one block per game, with
    **Back up this PC's** (it becomes the newest; the other PC's stays in the history) and
    **Don't back up** (preselected; restore the newest later). The answers go to the job in
    memory only, and core decides again from the fresh scan.
  - `pre-restore` safety backups keep their own retention list, same rules, newest kept
    (ADR-0034). A restore always stores this PC's files that way first (ADR-0005).
  - The base is stored with its hash list in `sync-base.json`.
  - ~~Restoring a `pre-restore` safety backup clears the game's base, so the next Back up asks.~~
    Replaced the same day, with the user's approval: it records the safety backup as the base
    like any restore, which reads behind, and Back up asks only after the saves change.
  - **Back up anyway** (2026-11-25, approved by the user for behind, changed-on-both and
    unknown-base games): a button under such a game on the Backup tab backs up only that game,
    now, with no question: this PC's saves become the newest, and the other PC's stay in the
    history. The consent is in memory for that backup only, as in the dialog.
- **Restore tab:** the default snapshot stays the newest regular one; a game whose recorded base
  (no seed) is not its newest gets a marker.

### Amends ADR-0010, ADR-0018 and ADR-0029

- *ADR-0010:* `skip_if_unchanged` only for the `pre-restore` safety backup.
- *ADR-0018:* the skip rule and the preview's lines become the states above; `Stored` and the
  preview's `Next` follow them; "Baseline = this machine's last backup" is still rejected as the
  comparison, and this PC's last sync is the base instead.
- *ADR-0029:* the Back up dialog also asks about each game changed on both.
- ADR-0021's mapping is reused unchanged; ADR-0019, ADR-0020 and ADR-0023 are unchanged except
  that a restore records the base.

## Alternatives considered

- **Keep rule (A) everywhere:** a smaller change, but a game to store is skipped silently when
  the staging tree equals this host's last snapshot.
- **Changed on both backs up silently** (last writer wins): the other PC is then offered these
  saves as newer with no warning.
- **A "conflict copy"** that is never the newest (a new tag): a second line of history and a
  repository format change, which is what the user wants gone.
- **Base without its hash list** (only the id): a PC whose base snapshot the rules removed can
  no longer tell behind from changed on both, and asks.
- **Base in `config.yaml`:** hash lists are bulky machine data, and `State::save_config` is the
  only config writer, on the GUI thread; the base is written by the backup and restore jobs.
  **In the cache dir:** clearing caches must not reset sync state. **In the repository:** only
  Back up writes there (ADR-0029), and the base is per PC.

## Consequences

- No repository format change; existing per-host snapshots become one list per game by time.
- A renamed PC without a record has no seed: a game whose saves differ from the newest reads
  "unknown base" and asks.
- A PC whose clock runs far ahead makes the "newest" backup; the others read behind against it
  (as ADR-0015's clock-skew consequence).
- A PC still on 0.10.0 or older backs up by rules (A) and (B) and can make a behind PC's saves
  the newest; every PC should be upgraded.
- The preview reads one more snapshot listing and list per game whose base has no list and is
  not the newest, the cost it paid for the "parent" before.
- Each upgraded PC asks once about every game it changed since its own last backup, which it
  made the newest: its seed is that backup, which reads unknown base, not ahead.
- `secret_scope()` also holds non-secret option values of the repository, so editing one of
  them keys every base anew: each game falls back to its seed, which asks rather than stores.
- A game with registry keys, or whose `salvajocs.json` would change (a new prefix or root), never
  reads behind in the preview; it reads changed on both there, and Back up decides from the
  staged list.
- When some paths of a game could not be read, the preview cannot predict its `salvajocs.json`,
  so the dialog may ask about it while Back up still decides behind from the staged list; the
  results line must then say why the user's choice was not applied.
