# Plan 002: Enforce max_entries so the database stops growing without bound

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/db.rs src/daemon.rs README.md`
> Plan 001 intentionally changes `src/db.rs` before this plan — that diff is
> expected. Compare the excerpts below against live code; the `insert()` body
> will differ (rowid-based after 001) but `handle_store()` should match. On
> any other mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW (a DELETE that can only remove non-favorite rows beyond the cap)
- **Depends on**: plans/001-fix-entry-id-collisions.md (test determinism: rapid inserts in the new tests would collide under the old timestamp-id scheme)
- **Category**: bug
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

`max_entries` is the first option in the config file and the README documents
it as "Maximum number of clipboard entries to keep" — but **nothing enforces
it**. It is read in exactly one place, as a display cap
(`src/overlay.rs:189`: `config.max_entries.min(20)`). Every unique copy —
including full-size images plus their stored thumbnails — is kept forever,
so the database grows without bound. This is a documented feature that does
not exist. After this plan, the daemon trims the oldest non-favorite entries
beyond `max_entries` after every store.

## Current state

- `src/config.rs` — `max_entries: usize` field (line 8), default `100`
  (line 31), documented in `DEFAULT_CONFIG_TOML` (line 112).
- `src/db.rs` — has `insert`, `delete`, `clear` but no trim. `clear()`
  (lines 355–358) shows the favorite-exemption convention:

```rust
pub fn clear(&self) -> Result<(), DbError> {
    self.conn.execute("DELETE FROM entries WHERE favorite = 0", [])?;
    Ok(())
}
```

- `src/daemon.rs` — `handle_store()` inserts at lines 290–334. The insert
  block as written today:

```rust
let mut data = entry::MimeDataMap::new();
data.insert(mime.clone(), content.clone());
let db = self.db.clone();
let result = {
    let db = db.lock().await;
    db.insert(data)
};
match result {
    Ok(id) => {
        tracing::info!(
            "Inserted entry {id}"
        );
        ...
```

- `contents` rows are removed automatically via
  `FOREIGN KEY ... ON DELETE CASCADE` (`src/db.rs` schema) — deleting an
  entry deletes its contents/thumbnails. `PRAGMA foreign_keys=ON` is set in
  `Database::open`.
- `README.md:131-132` documents the config key.

Semantics decision (already made — implement as specified): **favorites are
exempt and do not count toward the cap**, consistent with `clear()` and the
README's "Favorites pin entries so they survive clears and deletions". The
cap applies to the number of non-favorite entries.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | all pass |
| Targeted | `cargo test trim` | new trim tests pass |

## Scope

**In scope**:
- `src/db.rs` (new `trim_to_max` method + tests)
- `src/daemon.rs` (call site in `handle_store` + one test)
- `README.md` (one-line clarification)

**Out of scope**:
- `src/overlay.rs` — the `min(20)` display cap is a separate concern (plan 011 investigates it).
- Any change to `clear()` or favorite semantics.
- Config validation (plan 008 covers config feedback).

## Git workflow

- Branch: `improve/002-enforce-max-entries`
- Commit style: imperative, ≤72 chars (match `git log`)
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add `Database::trim_to_max`

In `src/db.rs`, after `clear()`, add:

```rust
pub fn trim_to_max(&self, max: usize) -> Result<usize, DbError> {
    let removed = self.conn.execute(
        "DELETE FROM entries
         WHERE favorite = 0
         AND id NOT IN (
             SELECT id FROM entries
             WHERE favorite = 0
             ORDER BY created_at DESC
             LIMIT ?
         )",
        params![max as i64],
    )?;
    Ok(removed)
}
```

Match the file's existing narrow hand-wrapped style (the repo does not use
rustfmt).

**Verify**: `cargo check --all-targets` → exit 0

### Step 2: Call it after every successful store

In `src/daemon.rs` `handle_store()`, inside the `Ok(id)` arm of the insert
`match` (after the `tracing::info!("Inserted entry {id}")` line and before
the thumbnail logic), add:

```rust
let trim_result = {
    let db = db.lock().await;
    db.trim_to_max(self.config.max_entries)
};
match trim_result {
    Ok(n) if n > 0 => {
        tracing::debug!(
            "Trimmed {n} old entries \
             (max_entries = {})",
            self.config.max_entries
        );
    }
    Ok(_) => {}
    Err(e) => {
        tracing::error!(
            "Failed to trim entries: {e}"
        );
    }
}
```

Note `db` (the `Arc<Mutex<Database>>` clone) is already in scope from the
insert block; the lock was released when the insert's block ended, so
re-locking here is correct and cheap.

**Verify**: `cargo test daemon::` → all pass

### Step 3: Tests

In `src/db.rs` tests (model after `clear_removes_non_favorites_only`,
line 539):

1. `trim_keeps_newest_non_favorites` — insert 5 entries (distinct text, no
   sleeps — plan 001 made ids collision-free), `trim_to_max(3)`, assert
   `list_entries(10)` has exactly 3 and they are the 3 most recent texts.
2. `trim_exempts_favorites` — insert 4 entries, favorite the OLDEST,
   `trim_to_max(2)`, assert the favorite survived and exactly 2 non-favorites
   (the newest two) remain → total 3.
3. `trim_noop_under_limit` — insert 2, `trim_to_max(10)`, returns `Ok(0)`,
   both remain.

In `src/daemon.rs` tests (model after `store_text_inserts_entry`, line 867):

4. `store_trims_beyond_max_entries` — build a daemon whose config has
   `max_entries: 3` (see `test_config()` at line 788; construct
   `Config { max_entries: 3, sync_selections: true, ..Config::default() }`
   inline like `store_no_sync_when_disabled` does at line 1017), store 5
   distinct texts via `handle_action(store_action(...))`, assert
   `db.list_entries(10)` has exactly 3 and the oldest two texts are gone.
   Caveat: `handle_store` has a 1-second same-hash window and other dedup
   gates, but those only skip *identical* content — 5 distinct strings pass
   through. Do NOT reuse content strings across stores in this test.

**Verify**: `cargo test trim && cargo test store_trims` → all 4 new tests pass

### Step 4: Clarify the README

In `README.md`, change the config-example comment (line ~131):

```toml
# Maximum number of clipboard entries to keep (favorites are exempt
# and do not count toward the limit)
max_entries = 100
```

Make the same edit to `DEFAULT_CONFIG_TOML` in `src/config.rs` (line ~111) so
`clipbro init` writes the same wording. The `write_default_config_roundtrip`
test parses this constant — confirm it still passes.

**Verify**: `cargo test config::` → all pass

## Test plan

Covered in Step 3 — happy path (1), the favorite-exemption edge (2), the
no-op edge (3), and the end-to-end daemon behavior (4). Pattern files:
`src/db.rs` tests module and `src/daemon.rs` tests module.

## Done criteria

- [ ] `cargo check --all-targets` exits 0
- [ ] `cargo test` exits 0
- [ ] 4 new tests exist and pass (`trim_keeps_newest_non_favorites`, `trim_exempts_favorites`, `trim_noop_under_limit`, `store_trims_beyond_max_entries`)
- [ ] `rg -n "trim_to_max" src/daemon.rs` shows exactly one call site, inside `handle_store`
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

- Plan 001 is not yet DONE in `plans/README.md` (the rapid-insert test in Step 3 will flake without it).
- `handle_store()` no longer matches the excerpt (insert block moved/refactored).
- The trim DELETE removes a favorite in test 2 — indicates the SQL subquery was altered; revert to the exact statement in Step 1.

## Maintenance notes

- If a bulk-import or restore feature is ever added, it must call `trim_to_max` once at the end, not per row.
- Reviewer: confirm the trim runs AFTER insert (so the just-stored entry counts toward the cap and the cap is exact), and that `n > 0` logging is debug-level (it fires on every store once the db is full).
- Deferred: a one-time vacuum/trim at daemon startup for users whose dbs already grew past the cap — the first store after upgrade trims them anyway; `VACUUM` to reclaim disk was judged out of scope (note it to the operator if a user reports a huge db file).
