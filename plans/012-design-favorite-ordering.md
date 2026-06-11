# Plan 012: Design — manual ordering for favorites (use or remove the dead favorite_position column)

> **Executor instructions**: This is a DESIGN plan. The deliverable is a
> design document (`plans/spikes/favorite-ordering.md`) answering the listed
> questions with concrete proposals, plus a step-outline for the build plan.
> Write no feature code. If anything in the "STOP conditions" section
> occurs, stop and report.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/db.rs src/overlay.rs`
> Expect drift from plans 001–010. Re-verify the load-bearing fact:
> `rg -n "favorite_position" src/` → only schema DDL in src/db.rs, no reads
> or writes.

## Status

- **Priority**: P3
- **Effort**: S (design doc; the build is a separate M plan)
- **Risk**: LOW
- **Depends on**: plans/001-fix-entry-id-collisions.md (the schema/migration machinery it adds — `PRAGMA user_version` — is what a column change would ride on)
- **Category**: direction
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

The `entries` table has carried a `favorite_position INTEGER` column since
the favorites feature landed, but nothing reads or writes it — favorites
sort by `created_at DESC` like everything else (`ORDER BY favorite DESC,
created_at DESC` in `list_entries_light`). The maintainer's intent (per
project notes) was manual ordering of favorites; it was never built. Dead
schema misleads readers and violates the repo's no-phantom-features rule.
This plan produces the design that either builds the feature or deletes the
column — a decision, not a lingering TODO.

## Current state

- `src/db.rs` schema — `favorite_position INTEGER` (line 52 at d7a7c18;
  preserved verbatim by plan 001's migration).
- Ordering queries: `list_entries` / `list_entries_light`
  (`ORDER BY favorite DESC, created_at DESC`).
- Favorites UX today: `toggle_favorite` flips the flag
  (`src/db.rs:347-353`); the overlay writes it directly on its own db
  connection (`src/overlay.rs:459-474` — note: the overlay and daemon both
  hold connections to the same SQLite db in WAL mode; direct overlay writes
  are the established pattern). Gold border + star; favorites float to the
  front; `clear()` and Delete protect them. Tab-cycle has a "Favorites"
  filter. Ctrl+1..9 selects by displayed index.
- Hotkey landscape (for picking reorder bindings): existing bindings are
  Enter/Escape/Tab/Shift+Tab/arrows/Ctrl+1..9/Ctrl+F/Ctrl+P/Delete and
  configurable strings via `[hotkeys]` (`src/config.rs`, plan 008 added
  validation). Arrow keys navigate; Ctrl+arrows are FREE today.
- SQLite version: bundled via rusqlite 0.40 (`bundled-sqlcipher`) — recent
  enough for `NULLS LAST` (3.30+).

## Design questions the document must answer

1. **Build it or drop it?** Recommendation to start from: build — the
   maintainer recorded intent, the column exists, and favorites are the
   feature's power-user surface. But the doc must give the drop-it cost too
   (a 2-line migration via the plan-001 `user_version` mechanism) so the
   maintainer can choose with eyes open.
2. **Ordering semantics**: proposed — `ORDER BY favorite DESC,
   favorite_position ASC NULLS LAST, created_at DESC`. New favorites get
   `max(favorite_position)+1` (append at end of favorites). Unfavoriting
   clears the position to NULL. Mixed NULL/non-NULL favorites (pre-feature
   favorites) sort after positioned ones, by recency — is that acceptable,
   or should toggling backfill positions for all favorites?
3. **Reorder UX**: proposed — Ctrl+ArrowLeft/Right (horizontal layout) and
   Ctrl+ArrowUp/Down (vertical) move the focused favorite one slot; only
   meaningful inside the Favorites filter or among the favorite block.
   Alternatives to weigh: drag-and-drop (iced layer-shell drag support is
   immature — likely reject, verify), or `[hotkeys]` config entries
   (`move_favorite_left/right`) for consistency with plan-008 validation.
4. **Swap mechanics**: positions swap with the neighbor (`UPDATE` two rows
   in one transaction) vs. renumber-all (simpler invariants, more writes —
   at ≤100 favorites, renumber-all is fine; recommend whichever makes the
   db API smallest).
5. **Process split**: reorder writes happen from the overlay's direct db
   connection (matching `toggle_favorite`) — confirm no daemon
   participation is needed and that a daemon-side `list_entries` ordering
   change can't race the overlay's in-memory `entries` order visibly.
6. **Interaction audit**: Ctrl+1..9 index badges renumber after reorder
   (they index displayed order — fine?); `trim_to_max` (plan 002) exempts
   favorites (no interaction); search results ignore manual order or honor
   it within the favorites block?

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Confirm column is still dead | `rg -n "favorite_position" src/` | DDL only |
| Confirm free keybindings | `rg -n "ArrowLeft\|ArrowRight" src/overlay.rs` | only the Nav handlers (no Ctrl+arrow use) |
| SQLite NULLS LAST support | `rg -n "bundled-sqlcipher" Cargo.toml` | present (bundled ≥3.40) |

## Scope

**In scope**: `plans/spikes/favorite-ordering.md` (create)

**Out of scope**: ALL source changes; the build plan itself (outline only).

## Git workflow

- Branch: none needed (single doc commit on `improve/012-favorite-ordering-design`)
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Verify the facts table

Run the three commands above; re-read the cited code regions; correct any
drift in the design doc's "current state" section rather than here.

### Step 2: Write the design doc

`plans/spikes/favorite-ordering.md` with sections: Decision (build/drop +
rationale), Semantics (question 2 answered with the exact ORDER BY and
assignment rules), UX (question 3 with the chosen keys and their
`[hotkeys]` names), DB API (new `Database` methods with signatures, e.g.
`move_favorite(&self, id: i64, direction: i8) -> Result<(), DbError>` or the
renumbering alternative), Migration (none if building — column exists; the
2-line drop migration if not), Interactions (question 6, each with a
one-line resolution), Open questions for the maintainer (anything genuinely
unresolvable from the codebase), and Build-plan outline (numbered steps with
test names, sized S/M).

### Step 3: Sanity-check the ORDER BY against real SQLite

One throwaway check (no repo changes): `sqlite3 :memory:` — create a toy
table, insert favorites with NULL and non-NULL positions, run the proposed
`ORDER BY favorite DESC, favorite_position ASC NULLS LAST, created_at DESC`,
confirm the ordering matches the doc's claim. Paste the session into the doc
as evidence.

**Verify**: the doc exists; every design question 1–6 has a concrete answer
or is explicitly in "Open questions" with a recommendation.

## Test plan

Not applicable (design doc). The build-plan outline inside the doc must name
its tests (e.g. `move_favorite_swaps_neighbors`,
`unfavorite_clears_position`, `null_positions_sort_after_positioned`).

## Done criteria

- [ ] `plans/spikes/favorite-ordering.md` exists, answers questions 1–6, includes the sqlite3 ordering evidence and a build-plan outline
- [ ] No source files modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

- `favorite_position` is no longer in the schema or has gained read/write
  sites since d7a7c18 — the premise changed; report.
- Plan 001 is not DONE — the migration mechanism the doc references doesn't
  exist yet; the doc can still be written, but flag it.

## Maintenance notes

- If the maintainer chooses "drop the column", do it in the same release as
  some other schema migration to avoid a user-visible rebuild for nothing.
- The chosen reorder hotkeys must go through plan 008's validation path and
  the README's supported-keys list.
