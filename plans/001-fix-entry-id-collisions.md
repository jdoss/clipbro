# Plan 001: Stop using timestamps as entry primary keys so same-millisecond copies are never lost

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/db.rs src/daemon.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW (schema migration on existing user databases — mitigated by a versioned, transactional migration)
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `d7a7c18`, 2026-06-11
- **Revised**: 2026-06-11 during execution — Step 2.1 corrected. The original
  premise ("SQLite's default is foreign_keys OFF") is false for this build:
  rusqlite's `bundled-sqlcipher` compiles with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`
  (libsqlite3-sys 0.38.1 `build.rs:126`), so an explicit
  `PRAGMA foreign_keys=OFF` before `migrate()` is required.

## Why this matters

`Database::insert()` uses the current millisecond timestamp as the entry's
PRIMARY KEY. Two different clipboard events stored in the same millisecond
collide: the second `INSERT` fails with a UNIQUE constraint violation and the
entry is **silently lost** (the daemon only logs "Failed to insert").
A second manifestation: in the image-supersedes-text flow, the daemon deletes
the text entry and then inserts the image — if both happen in the same
millisecond, the image **reuses the deleted entry's id**, resurrecting a
"deleted" id. This is exactly why `daemon::tests::store_image_supersedes_text`
is flaky (fails under parallel test load, passes in isolation), and why
nearly every db test contains a 5ms `sleep` workaround.

After this plan: ids are SQLite `AUTOINCREMENT` rowids (never reused, never
colliding), `created_at` remains a plain column for ordering, all sleeps are
removed from tests, and the suite is deterministic.

## Current state

- `src/db.rs` — all database access. Schema in `migrate()` (lines 45–70),
  the bug in `insert()` (lines 72–98), tests at lines 361–577.
- `src/daemon.rs` — the flaky test `store_image_supersedes_text`
  (lines 971–1015); the image-supersedes-text delete logic (lines 262–288).

`src/db.rs:45-70` (schema — note `id INTEGER PRIMARY KEY` is a rowid alias,
and the so-far-unused `favorite_position` column which must be preserved):

```rust
fn migrate(&self) -> Result<(), DbError> {
    self.conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY,
            created_at INTEGER NOT NULL,
            entry_type TEXT NOT NULL,
            favorite INTEGER NOT NULL DEFAULT 0,
            favorite_position INTEGER
        );

        CREATE TABLE IF NOT EXISTS contents (
            entry_id INTEGER NOT NULL,
            mime TEXT NOT NULL,
            content BLOB NOT NULL,
            PRIMARY KEY (entry_id, mime),
            FOREIGN KEY (entry_id)
                REFERENCES entries(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_entries_created
            ON entries(created_at);
        CREATE INDEX IF NOT EXISTS idx_entries_type
            ON entries(entry_type);",
    )?;
    Ok(())
}
```

`src/db.rs:84-88` (the bug — timestamp supplied as id):

```rust
self.conn.execute(
    "INSERT INTO entries (id, created_at, entry_type) VALUES (?, ?, ?)",
    params![now, now, entry_type.as_str()],
)?;
let id = now;
```

`src/db.rs:30-42` (`open()` — PRAGMAs run in this order; the migration you
add must run with `foreign_keys` OFF, see Step 2):

```rust
let conn = Connection::open(path)?;

if encrypt {
    let key = get_encryption_key()?;
    conn.pragma_update(None, "key", &key)?;
}

conn.execute_batch("PRAGMA journal_mode=WAL;")?;
conn.execute_batch("PRAGMA foreign_keys=ON;")?;

let db = Self { conn };
db.migrate()?;
Ok(db)
```

Repo conventions: errors via the `DbError` thiserror enum (`src/db.rs:7-15`);
tests are inline `#[cfg(test)] mod tests` using `tempfile` with
`Database::open(&path, false)` (see `test_db()` at `src/db.rs:366-372`).
The repo does NOT use rustfmt — match the surrounding hand-wrapped style.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| All tests | `cargo test` | all pass (102+ tests; currently `store_image_supersedes_text` is flaky — after this plan it must pass repeatedly) |
| One test, repeated | `for i in $(seq 10); do cargo test store_image_supersedes_text -q \|\| break; done` | 10 consecutive passes |
| Lint | `cargo clippy --all-targets` | no NEW warnings beyond the 26 pre-existing ones |

## Scope

**In scope** (the only files you should modify):
- `src/db.rs`
- `src/daemon.rs` (test module only — removing sleeps; production code in it must not change)

**Out of scope** (do NOT touch, even though they look related):
- `src/entry.rs` — `Entry.id` stays `i64`; no type changes.
- `src/overlay.rs` — reads ids opaquely; no change needed.
- The `favorite_position` column — keep it exactly as-is (plan 012 decides its fate).
- Dedup logic in `find_duplicate()` / `handle_store()` — unchanged.

## Git workflow

- Branch: `improve/001-entry-id-collisions` (repo uses `topic/slug` branches, e.g. `deps/update-2026-06`)
- Commit style: imperative, ≤72 chars, like `Fix image capture from Chromium-based apps` in `git log`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add a versioned migration to an AUTOINCREMENT schema

In `src/db.rs`, rewrite `migrate()` to be versioned via `PRAGMA user_version`:

- Read `user_version` (`self.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))`).
- **Version 0, fresh database** (no `entries` table — check
  `SELECT name FROM sqlite_master WHERE type='table' AND name='entries'`):
  create the new schema directly and set `user_version = 1`. New schema is
  identical to the old one except `id INTEGER PRIMARY KEY AUTOINCREMENT`.
- **Version 0, existing database** (entries table exists): run the rebuild in
  Step 2, then set `user_version = 1`.
- **Version 1**: do nothing.

`CREATE INDEX IF NOT EXISTS` statements stay for both paths.

**Verify**: `cargo check --all-targets` → exit 0

### Step 2: Implement the table rebuild for existing databases

SQLite cannot `ALTER TABLE` to add AUTOINCREMENT; rebuild the table. Order is
load-bearing because `contents.entry_id` has `ON DELETE CASCADE` — dropping
`entries` with foreign keys ON would cascade-delete all contents:

1. In `open()`, run the migration with foreign keys explicitly OFF, then
   re-enable them. Order: `PRAGMA journal_mode=WAL;` →
   `PRAGMA foreign_keys=OFF;` → `db.migrate()` → `PRAGMA foreign_keys=ON;`.
   The explicit OFF is load-bearing: this project's rusqlite uses
   `bundled-sqlcipher`, and libsqlite3-sys compiles the bundled library with
   `-DSQLITE_DEFAULT_FOREIGN_KEYS=1` (`build.rs:126` in libsqlite3-sys
   0.38.1), so foreign keys are ON by default — relying on stock SQLite's
   OFF default silently cascade-deletes all `contents` rows at the rebuild's
   `DROP TABLE entries`. `PRAGMA foreign_keys` cannot change inside a
   transaction, so the OFF must run before the rebuild's BEGIN.
2. The rebuild, inside one transaction (`execute_batch` with explicit
   BEGIN/COMMIT is fine):

```sql
BEGIN;
CREATE TABLE entries_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    entry_type TEXT NOT NULL,
    favorite INTEGER NOT NULL DEFAULT 0,
    favorite_position INTEGER
);
INSERT INTO entries_new (id, created_at, entry_type, favorite, favorite_position)
    SELECT id, created_at, entry_type, favorite, favorite_position FROM entries;
DROP TABLE entries;
ALTER TABLE entries_new RENAME TO entries;
CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at);
CREATE INDEX IF NOT EXISTS idx_entries_type ON entries(entry_type);
PRAGMA user_version = 1;
COMMIT;
```

Existing ids (large millisecond timestamps) are preserved; AUTOINCREMENT
continues from `max(id)+1`, so ordering-by-id stays monotonic. `contents`
rows are untouched and still reference the same ids.

**Verify**: `cargo test db::` → all db tests pass (they create fresh dbs and
now exercise the version-0-fresh path)

### Step 3: Let SQLite assign ids in `insert()`

Replace the excerpt shown in "Current state" (`src/db.rs:84-88`) with:

```rust
self.conn.execute(
    "INSERT INTO entries (created_at, entry_type) VALUES (?, ?)",
    params![now, entry_type.as_str()],
)?;
let id = self.conn.last_insert_rowid();
```

`now` is still computed at the top of `insert()` and used for `created_at`
and the duplicate-touch UPDATE — leave those alone.

**Verify**: `cargo test db::` → all pass

### Step 4: Add regression tests

In the `tests` module of `src/db.rs`, add (model after the existing tests in
the same module, e.g. `insert_different_text_different_ids` at line 452):

1. `rapid_inserts_get_distinct_ids` — insert 50 entries with distinct text
   (`format!("entry {i}")`) in a tight loop with NO sleep; collect ids into a
   `std::collections::HashSet`; assert `set.len() == 50`.
2. `deleted_id_is_never_reused` — insert A then B (no sleep); `db.delete(b)`;
   insert C with different text; assert `c_id != b_id` and `c_id > b_id`.
3. `migration_preserves_existing_rows` — build a database with the OLD schema
   by hand: open a raw `rusqlite::Connection` on a temp path, `execute_batch`
   the old-schema DDL (the exact "Current state" schema excerpt above, without
   AUTOINCREMENT), insert one row into `entries` (id = 1749000000000) and one
   into `contents` for it, close the connection. Then `Database::open(&path,
   false)` and assert `get_entry(1749000000000)` returns the entry with its
   content intact, and that `PRAGMA user_version` is 1.

**Verify**: `cargo test db::` → all pass, including the 3 new tests

### Step 5: Remove the sleep workarounds

Delete every `std::thread::sleep(std::time::Duration::from_millis(5))` in
`src/db.rs` tests (6 occurrences: lines 409, 420, 433, 445, 457, 467, 506,
544 region) and in `src/daemon.rs` tests (1 occurrence at lines 1100-1102 in
`clear_removes_non_favorites`). One nuance: `touch_updates_timestamp`
(`src/db.rs:496-517`) asserts `after > before` on `created_at` — that one
legitimately needs time to advance, so KEEP its sleep (or change the
assertion to `>=` and drop it; prefer keeping the sleep and a one-line
comment `// created_at has millisecond resolution`).

**Verify**: `cargo test` → all pass

### Step 6: Prove the flake is gone

**Verify**: `for i in $(seq 10); do cargo test store_image_supersedes_text -q || break; done`
→ 10 consecutive passes, and `cargo test` (full parallel suite) → 0 failures
run 3 times in a row.

## Test plan

Covered by Steps 4–6. The structural pattern is the existing
`src/db.rs` tests module (tempfile db, `encrypt=false`). The migration test
in Step 4.3 is the only test that hand-builds a legacy schema — keep it
self-contained in the test body.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `cargo check --all-targets` exits 0
- [ ] `cargo test` exits 0, three consecutive runs
- [ ] `rg -n "INSERT INTO entries \(id" src/db.rs` → only the migration-test's legacy-schema setup matches (or nothing)
- [ ] `rg -n "from_millis\(5\)" src/db.rs src/daemon.rs` → at most the one kept in `touch_updates_timestamp`
- [ ] New tests `rapid_inserts_get_distinct_ids`, `deleted_id_is_never_reused`, `migration_preserves_existing_rows` exist and pass
- [ ] `git status` shows no modified files outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The schema or `insert()` at the cited lines doesn't match the excerpts.
- `PRAGMA user_version` is already non-zero in a fresh `Database::open` test.
- The migration test (Step 4.3) fails because `contents` rows were cascade-deleted — that means migration is running with foreign keys ON; re-check the explicit `PRAGMA foreign_keys=OFF` in Step 2.1, and if it still fails, stop.
- You find any production code (not tests) that relies on `entry.id` equaling `entry.created_at` — search first: `rg -n "\.id" src/ | rg -i "created|time|now"`.

## Maintenance notes

- Plan 002 (max_entries trimming) and plan 012 (favorite ordering) both touch this schema/area — execute this plan first; they were written assuming AUTOINCREMENT ids.
- Reviewer should scrutinize: the explicit `PRAGMA foreign_keys=OFF` before `migrate()` in `open()` (and ON after), and that `user_version` is set inside the same transaction as the rebuild.
- Plans 002 and 012, if they do table rebuilds, are subject to the same compiled-in `SQLITE_DEFAULT_FOREIGN_KEYS=1` default — same explicit-OFF mechanism applies.
- Deferred: backup-before-migrate (copy db file aside). The transaction makes the rebuild atomic; a file backup was judged unnecessary for a clipboard history db, but flag it to the operator if they want belt-and-suspenders.
