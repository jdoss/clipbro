# Plan 011: Spike — make search cover the full clipboard history, not just the 20 newest entries

> **Executor instructions**: This is a SPIKE/design plan, not a build plan.
> The deliverable is a written recommendation with measurements
> (`plans/spikes/full-history-search.md`) plus throwaway prototype branches —
> NOT merged feature code. Follow the steps, record numbers, and stop at the
> decision point. If anything in the "STOP conditions" section occurs, stop
> and report.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/overlay.rs src/db.rs src/entry.rs`
> Expect drift from plans 001–010. The load-bearing fact to re-verify:
> `rg -n "min(20)" src/overlay.rs` still shows the display cap.

## Status

- **Priority**: P2
- **Effort**: M (spike itself: ~half a day; the follow-up build plan is separate)
- **Risk**: LOW (spike produces a document and measurements, no merged code)
- **Depends on**: plans/005-overlay-open-latency.md (its LazyLock fix changes the measurements this spike takes); plan 010 helpful but optional
- **Category**: direction
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

The database keeps `max_entries` entries (default 100, enforced by plan
002), but the overlay loads only `config.max_entries.min(20)`
(`src/overlay.rs:189`) and search runs in-memory over those loaded entries
(`filtered_entries`, `src/overlay.rs:819-891`). So the README's headline
"Multi-term search across content, language, and type" silently cannot find
anything older than the 20 newest entries — arguably the #1 reason to have
a clipboard history at all ("I copied it earlier today"). The 20-cap exists
for a reason: per-entry syntax highlighting at open was expensive
(mitigated by plan 005) and thumbnails are pre-built. This spike measures
what it actually costs to search everything and recommends the cheapest
design that doesn't regress open latency.

## Current state

- `src/overlay.rs:189-192`:

```rust
let display_limit = config.max_entries.min(20);
let entries = db
    .list_entries_light(display_limit)
    .unwrap_or_default();
```

- `list_entries_light` (`src/db.rs:203-245`) loads id/created_at/type/favorite
  + non-image contents (text only; `load_contents_filtered` skips
  `image/%` mimes) — so a "light" entry is small (text bytes only).
- Highlights are built for ALL loaded text entries at open
  (`build_highlights`, `src/overlay.rs:926-961`), truncated to 500 chars
  each. Thumbnails (`build_handles`) only for entries with stored thumbnail
  blobs.
- Search semantics to preserve (`src/overlay.rs:855-890`): multi-term AND;
  a term matches lowercased text content, highlight language name, or the
  type words "image"/"url".
- `nucleo` (fuzzy matcher) was REMOVED as an unused dependency by plan 003 —
  if fuzzy matching is part of the recommendation, it must be re-justified
  as a new dependency per the maintainer's rules ("justify new
  dependencies"); plain substring search needs nothing.

## Candidate designs to evaluate (and the working hypothesis)

- **A. Load everything light, search in memory** — change the load to
  `max_entries`, keep `filtered_entries` as-is, but only build
  highlights/handles for the first 20 (or visible) entries; highlight the
  rest lazily or show them un-highlighted. 100 light text entries is ~a few
  hundred KB; the hypothesis is this is the right answer at the current
  scale.
- **B. Two-tier: 20 at open, query the db on first keystroke** — keep open
  cost identical; on first non-empty query, load all light entries once and
  cache. Slightly more code, zero open-latency risk.
- **C. SQL-side search** (`WHERE content LIKE`) — scales past in-memory but
  fights the multi-term/language/type semantics (language lives only in the
  overlay's highlight map, not the db) and adds UTF-8/BLOB CAST complexity.
  Expected verdict: rejected at current scale; record why.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Seed a 100-entry history | `for i in $(seq 100); do echo "spike entry $i $(head -c 8 /dev/urandom \| xxd -p)" \| wl-copy; sleep 0.15; done` (in a running session with the daemon active) | db at max_entries |
| Time overlay init | `RUST_LOG=clipbro=debug cargo run -- overlay` with temporary `tracing::debug!` timers (see Step 2) | timings in stderr |
| Tests still green on prototype branches | `cargo test` | all pass |

## Scope

**In scope** (spike artifacts only):
- `plans/spikes/full-history-search.md` (create — the deliverable)
- Throwaway branches `spike/011-option-a`, `spike/011-option-b` (never merged)

**Out of scope**:
- Merging any prototype to master.
- Changing search semantics (fuzzy matching, ranking) — that's a separate
  product decision; note it under open questions only.
- README changes (happen in the follow-up build plan).

## Steps

### Step 1: Establish the baseline

On a seeded 100-entry db (see Commands): instrument `Overlay::new()` with
`std::time::Instant` spans around: `Database::open`, `list_entries_light`,
`build_handles`, `build_highlights`, total `new()`. Log via
`tracing::debug!`. Record timings with `display_limit = 20` (current).

**Verify**: a table of 5 numbers in the spike doc.

### Step 2: Measure option A

On branch `spike/011-option-a`: set `display_limit = config.max_entries`,
limit `build_highlights` to the first 20 entries (one-line change: iterate
`entries.iter().take(20)`), rerun the measurement. Then also measure the
worst case: highlights for all 100.

**Verify**: same table, two more columns (A-lazy, A-full). Key question
answered: does A-lazy keep total `new()` within ~10ms of baseline?

### Step 3: Sketch option B's cost only if A-lazy regresses

If A-lazy regressed open latency >25ms, prototype B (db load on first
keystroke; cache in a `Option<Vec<Entry>>` field) and measure the keystroke
stall. Otherwise skip — record "not needed at current scale".

### Step 4: Check the UX seams

On the A-lazy branch, manually exercise: search for an old entry (term only
in entry #95) → found? un-highlighted old entries readable? Ctrl+1..9 still
selects what's visible? scroll behavior with 100 cards (does
`scroll_to_focused`'s ratio math hold)? Record observations — these
constraints feed the build plan.

### Step 5: Write the recommendation

`plans/spikes/full-history-search.md` containing: the measurement tables,
the chosen option with rationale, rejected options with one-line reasons,
the lazy-highlight strategy (on-scroll? on-idle Task? never for >500-char
entries?), open questions for the maintainer (fuzzy search? raise
max_entries beyond 100 someday? README wording), and a step-outline for the
follow-up build plan. Delete the instrumentation; leave the spike branches
unmerged (named so they're findable).

**Verify**: the doc exists and answers: which option, what it costs at open,
what it costs per keystroke, what the build plan's steps are.

## Test plan

Not applicable (spike) — prototype branches must still pass `cargo test`,
since measurements on a broken build are meaningless.

## Done criteria

- [ ] `plans/spikes/full-history-search.md` exists with baseline + option measurements
- [ ] A recommendation is stated (one option, with numbers)
- [ ] Spike branches exist locally and are NOT merged; master untouched
- [ ] `plans/README.md` status row updated (DONE = recommendation written)

## STOP conditions

- No live COSMIC/Wayland session is available — the measurements are the
  deliverable; without a session, report that this spike needs the operator
  to run the seed+measure steps.
- Plan 005 not landed — measurements would be dominated by the syntect
  reload cost and mislead the decision.
- The seeded db can't reach 100 entries (plan 002's trim semantics
  interfering in an unexpected way) — that's a bug report, not a spike
  result.

## Maintenance notes

- The follow-up build plan should be written (by the advisor or maintainer)
  from the spike doc's step-outline — don't let the executor improvise the
  build from the spike.
- If the recommendation is A-lazy, plan 010's `apply_filters` is where the
  search continues to live; no schema work needed.
