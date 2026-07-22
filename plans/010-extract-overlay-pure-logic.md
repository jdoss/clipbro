# Plan 010: Extract overlay's pure logic (hotkeys, filtering, cycling) into tested modules

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/overlay.rs src/main.rs`
> This plan EXPECTS prior drift from plans 003/005/006/008 (lint moves,
> async active-entry, paused field, fallible hotkey parse). The excerpts
> below describe d7a7c18 with those deltas noted. If `filtered_entries` or
> the cycle handlers differ beyond those deltas, STOP.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED (mechanical moves + signature changes in the repo's biggest, highest-churn file)
- **Depends on**: plans/008-validate-hotkey-config.md (moves the final, fallible ParsedHotkey). Land after 005/006 to avoid conflicts.
- **Category**: tech-debt
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

`src/overlay.rs` is 1600 lines — the largest, most-churned file (20 of the
last 50 commits) — mixing iced view code with pure logic. The pure parts
(hotkey parsing/matching, entry filtering, filter-cycle stepping) are
exactly the parts that change most and are unit-testable without a Wayland
display, but today they're buried in the UI module with only the hotkey
parser tested. This plan extracts them into `src/hotkey.rs` and
`src/filter.rs` with behavior pinned by tests written BEFORE the move.
This is deliberately NOT a redesign: signatures change minimally, view code
stays put, daemon.rs is untouched.

## Current state

- `src/main.rs:1-8` — module declarations (flat: `mod clipboard; mod config;
  mod daemon; mod db; mod dbus; mod entry; mod overlay; mod systemd;`).
  You will add `mod hotkey; mod filter;`.
- `src/overlay.rs:69-142` — `ParsedHotkey` struct + `parse` + `matches` +
  `matches_named` (after plan 008: `parse` returns `Result<Self, String>`
  and a `NAMED_KEYS` const exists). Its tests live at the bottom of
  overlay.rs (12 tests after plan 008).
- `src/overlay.rs:819-891` — `Overlay::filtered_entries(&self) -> Vec<&Entry>`:
  filters `self.entries` by `self.type_filter` (arms: `"Favorites"` →
  favorite, `"Text"`/`"Images"`/`"URLs"` → entry_type, anything else →
  Text entries whose highlight language equals the filter), then if
  `self.search_query` is non-empty, lowercases it, splits on whitespace,
  and keeps entries where EVERY term matches text content OR highlight
  language OR the literal type word (`"image"`/`"url"`).
- `src/overlay.rs:307-357` — `CycleTypeFilter` / `CycleTypeFilterReverse`
  handlers: step `self.type_filter` through `self.filter_cycle`
  (None → first; last → None when forward; None → last; first → None when
  reverse; unknown current → None).
- `HighlightedText { language: String, spans: Vec<(Color, String)> }`
  (`src/overlay.rs:144-147`) — note `Color` is an iced type; the filter
  module must NOT depend on it (see Step 3's signature).
- Callers of `filtered_entries`: `update()` (SelectByIndex,
  ToggleFocusedFavorite, DeleteEntry, DeleteEntryById, Dismiss, NavForward,
  NavBackward), `scroll_to_focused`, `select_focused_and_exit`, `view()`.
- Repo conventions: flat modules, inline `#[cfg(test)] mod tests`, no
  rustfmt — preserve the existing hand-wrapped style when moving code.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | all pass; test COUNT must not decrease across moves |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 (post plan 003) |
| Line count | `wc -l src/overlay.rs` | meaningfully below 1600 (expect ~1250-1350) |

## Scope

**In scope**:
- `src/hotkey.rs` (create), `src/filter.rs` (create)
- `src/overlay.rs` (remove moved code; call the new modules)
- `src/main.rs` (two `mod` lines)

**Out of scope**:
- `src/daemon.rs` — its seams (store pipeline vs process management) are
  riskier and deferred; do not touch.
- `entry_card()` and `view()` — view code stays in overlay.rs.
- Any behavior change whatsoever — this is a pure refactor; if a test needs
  its assertion changed (not just its import path), that's a STOP condition.
- Renaming `Message` variants or struct fields.

## Git workflow

- Branch: `improve/010-extract-overlay-logic`
- Commit per step (tests-first commit, then each extraction) so review can
  follow the behavior pinning.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Pin current filtering/cycling behavior with tests (before moving anything)

In `src/overlay.rs` `mod tests`, add tests against the CURRENT private
logic. For `filtered_entries` you can't easily build an `Overlay` (it owns a
`Database`), so write the tests against the functions you're ABOUT to
extract by first doing a minimal mechanical step: convert the body of
`filtered_entries` into a private free function in overlay.rs:

```rust
fn apply_filters<'a>(
    entries: &'a [Entry],
    query: &str,
    type_filter: Option<&str>,
    language_for: &dyn Fn(i64) -> Option<String>,
) -> Vec<&'a Entry>
```

…and make `Overlay::filtered_entries` a one-liner calling it with
`&|id| self.highlights.get(&id).map(|h| h.language.clone())`.
Similarly extract the two cycle handlers' match logic into:

```rust
fn cycle_next(current: Option<&str>, cycle: &[String]) -> Option<String>
fn cycle_prev(current: Option<&str>, cycle: &[String]) -> Option<String>
```

…and have the `update()` arms call them (each arm becomes
`self.type_filter = cycle_next(self.type_filter.as_deref(), &self.filter_cycle); self.focused_index = 0;`).

Then write the tests (build entries exactly like `src/entry.rs` tests do —
`Entry { id, created_at: 0, entry_type: detect_entry_type(&contents), favorite, contents }`):

- `apply_filters`: no filter/no query returns all; `"Favorites"` keeps only
  favorites; `"Images"`/`"URLs"`/`"Text"` by type; language filter (e.g.
  `"Rust"`) keeps only Text entries whose `language_for` returns `"Rust"`;
  multi-term query is AND across terms; term matches language name
  (`"rust"`) and type word (`"image"`); query is case-insensitive; empty
  entries → empty.
- `cycle_next`: None→first, mid→next, last→None, empty cycle→None,
  unknown current→None.
- `cycle_prev`: None→last, mid→previous, first→None, empty→None.

**Verify**: `cargo test overlay::` → all pass; note the total test count

### Step 2: Move hotkey code to `src/hotkey.rs`

Create `src/hotkey.rs`; move `ParsedHotkey`, `NAMED_KEYS`, `parse`,
`matches`, `matches_named`, and ALL hotkey tests there verbatim (adjust
imports: it needs `cosmic::iced` keyboard types — import as overlay.rs does
at lines 4-19). Make the type and needed methods `pub(crate)`. In
overlay.rs: `use crate::hotkey::ParsedHotkey;` and delete the moved code.
Add `mod hotkey;` to `src/main.rs`.

**Verify**: `cargo test` → same total count as Step 1, all pass

### Step 3: Move filtering to `src/filter.rs`

Move `apply_filters`, `cycle_next`, `cycle_prev`, and their tests to
`src/filter.rs` (pub(crate) functions). The module imports
`crate::entry::{Entry, EntryType}` ONLY — it must not import anything from
`cosmic`/iced (that's the point of the `language_for` closure parameter).
Add `mod filter;` to main.rs; update overlay.rs call sites
(`crate::filter::apply_filters(...)` etc.).

**Verify**: `cargo test` → same total count, all pass
**Verify**: `rg -n "cosmic|iced" src/filter.rs` → no matches

### Step 4: Confirm the seams hold

- `wc -l src/overlay.rs src/hotkey.rs src/filter.rs` — overlay.rs should be
  roughly 1250–1350; if it barely moved, the extraction missed code.
- `cargo clippy --all-targets -- -D warnings` → exit 0.
- Update `CLAUDE.md`'s module map (plan 004) if it exists: add the two new
  modules, one line each.

**Verify**: all of the above

## Test plan

Step 1 IS the test plan: behavior is pinned by new unit tests before any
code moves, then Steps 2–3 must keep the total test count and pass rate
identical. Pattern files: `src/entry.rs` tests (entry construction),
existing hotkey tests (structure).

## Done criteria

- [ ] `cargo test` exits 0; test count ≥ Step-1 count (nothing lost in moves)
- [ ] `src/hotkey.rs` and `src/filter.rs` exist, each with inline tests
- [ ] `rg -n "ParsedHotkey" src/overlay.rs` → only the `use` and OnceLock/static usages, no definition
- [ ] `rg -n "cosmic|iced" src/filter.rs` → no matches
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

- Plan 008 is not DONE (you'd be moving the pre-validation ParsedHotkey and
  008 would then conflict).
- Pinning tests in Step 1 reveal the filtering behavior differs from the
  description in Current state (e.g. a plan 005/006 change altered it) —
  report the difference; do not encode it silently.
- Any existing test needs an assertion (not import) change to pass.
- The `language_for` closure approach fails the borrow checker in
  `filtered_entries` — do not restructure `Overlay` to work around it; STOP
  and report the exact error.

## Maintenance notes

- This creates the natural home for plan 011's outcome (full-history search
  would change `apply_filters` or add a db-query path beside it) — another
  reason filter logic now lives display-independent.
- daemon.rs extraction (store pipeline vs process management) was considered
  and deferred: higher risk, lower test leverage, and the mock-based tests
  already cover its logic. Revisit only if daemon churn stays high.
- Reviewer: diff Steps 2–3 commits with `--color-moved` to confirm pure moves.
