# ADR-0030: Custom games from the repository need acceptance on each PC

**Status:** Proposed
**Date:** 2026-11-22

## Context

A custom game decides what a restore may write: its file templates, saved pairs and registry
keys are the declared roots and keys of ADR-0005, and its Steam id picks the Proton prefix.
Since 0.8.0 (ADR-0028, ADR-0029) a read merges the repository's custom games into this PC's
copy: a new game is appended and, when the copy mirrors the repository, a changed one replaces
the local one. Anyone who can write to the repository can therefore save a custom game that
declares `<xdgConfig>/autostart` or
`HKEY_CURRENT_USER/Software/Microsoft/Windows/CurrentVersion/Run`, or edit one the user already
has, and back up a snapshot of that game holding a payload. The
next restore of the game on any PC writes it. The cheapest attack edits a game the user already
uses.

The user decided "Ask on each PC": a custom game that comes from the repository, written there
by another PC, is not used on this PC until the user accepts it on this PC.

## Decision

- **Quarantine at the merge.** `custom_games::merge` never adds or replaces a game of
  `config.custom_games` from the repository. A repository copy that is new here, or that differs
  from the local copy when the copy mirrors the repository, goes to `Merged::offered` (by name,
  without a Wine prefix). So every game in `config.custom_games` was edited on this PC, kept from
  before this version, or accepted on this PC with its exact shared content. Every reader of
  `config.custom_games` (scan, restore, `game_info`, defaults, `apply_choices`) is unchanged and
  sees only those.
- **Acceptance is bound to content.** Accepting puts the offered copy in place, with this PC's
  `wine_prefix`, and adds no unsent entry. The accepted copy is the record: the next read finds
  it equal to the repository's (`shared(game) == stored`, as `merge` already compares) and offers
  nothing. Any later change on another PC is offered again. This PC's own edits
  (`edit_custom_game`, `apply_mapping_choices`) never need acceptance.
- **The offers live in memory** (`CustomGameJobs::offered`), rebuilt by every read, as
  `CustomGameJobs::unreadable` is, and forgotten when Settings switches to another repository.
  A restore needs an open repository, so a read has started. No `config.yaml` field is added.
  Marking a name unsent (an edit here, a mapping choice, a decline) drops its offer, since an
  unsent name is never offered; otherwise a declined copy would come back once Back up clears
  the entry.
- **An answer applies to the copy drawn.** The tab passes the offer it drew to
  `accept_custom_game` or `keep_this_pcs_custom_game`, which do nothing when a read that ended
  since replaced it; the next frame draws the new copy.
- **The Custom games tab** lists the offers whose name has no unsent entry under **From other
  PCs, not used here**, each with every line that matters (`game_lines`: rows, registry keys,
  old-style `entry.files` templates, install folders, pairs, skips and the Steam id); a changed
  game shows this PC's version beside the repository's, like the Back up conflict dialog. A read
  that offers a game the last read did not, or another copy of one, adds one notice.
  - **Use on this PC** accepts (`State::accept_custom_game`).
  - **Keep this PC's version**, or **Remove from the repository** for a game new here, declines
    through ADR-0029 (`State::keep_this_pcs_custom_game`): an unsent entry based on the offered
    copy, so the next Back up writes this PC's copy, or a tombstone when this PC lacks the game.
    No "declined" state is stored. Without an answer, the offer stays listed.
- **The Restore tab** says in the plan of a game, alone or in a restore of several, when an
  offer of its custom game waits for review, since its files would otherwise show as skipped
  (`UndeclaredRoot`) with no hint. The core restore is unchanged. The read runs in the
  background, so a restore planned before it ends shows no note.
- **Tombstones still drop the local game without asking**: a removal only narrows what a restore
  writes, and anyone who can write to the repository can already delete snapshots.
- **"Keep the repository's" in the Back up conflict dialog is an acceptance**: the user saw both
  versions, which `game_lines` now shows in full, and chose one.
- **Shape rules are defence in depth only.** A file template that is a whole base or a glob with
  no literal name below one (`crossbuild::too_wide`) declares nothing and `validate_file` refuses
  it; a registry key that is `registry::protected_key` declares nothing and the form refuses it
  (ADR-0005). No shape rule stops an absolute row (`/home/u/.config/autostart`), `<home>/.*`,
  `<xdgConfig>/a*` or a precise file such as `<home>/.bashrc`: acceptance is the control.
- **Migration:** the custom games a PC has when it first runs this version stay in use. No
  provenance was ever recorded, so the app cannot tell a game made here from a mirrored one, and
  a forced review would ask about the user's own games too.

### Amends ADR-0028 and ADR-0029

- *Merge:* "the repository's games the copy lacks are appended" and the mirror's "takes the
  repository's shared fields" become "offered"; a local game stays as this PC has it.
- *Keep the repository's* in the conflict dialog counts as an acceptance.

## Alternatives considered

- **Acceptance per name:** the cheapest attack edits a game already accepted.
- **A hash of the accepted content in `config.yaml`:** needs a canonical serialization and cannot
  show what changed; the accepted copy already is the record.
- **Filtering at each consumer** (scan, restore, defaults): many sites, and a new one would miss
  it; the merge is the one writer from the repository.
- **Filtering in `File::stored`:** the wrong layer, and lossy: a filtered game would be rewritten
  without its rows.
- **Re-asking once on upgrade:** needs a `custom_games_offered` field and asks about the user's
  own games, as no provenance exists.
- **A persisted "declined" list:** declining through an unsent entry reuses ADR-0029 and tells
  the other PCs.
- **"Accept all":** defeats the review.

## Consequences

- A game planted in the repository before this version, and merged then, stays in use.
- salvajocs 0.9.1 and older still merge the repository's games without asking; a downgrade does
  so again at its next read.
- Another PC's change of a game is not backed up from this PC until accepted here: Back up sends
  only unsent entries.
- The shape rules do not stop precise or absolute paths; only acceptance does.
- Offers are not shown offline.
- A saved pair with a bare side (`{linux: "<home>", windows: "<winDocuments>/x"}`) still maps a
  declared Windows root onto the whole Linux home in a cross-build restore; the pair-side rule
  only stops that side from declaring a root. The user chose to warn only: the mapping dialog
  and the preview name such a pair (ADR-0011).
