# Plan 007: Create the database and log files with 0600 permissions

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/db.rs src/main.rs`
> Plan 001 rewrote parts of src/db.rs (migration); the `Connection::open`
> call in `open()` should still match. On mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (tightening permissions on files only this user's processes read)
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

Clipboard history is among the most sensitive data on a desktop (passwords,
tokens, private text routinely transit the clipboard). The SQLite database
and the log file are created with the process umask (typically 022 →
world-readable 0644):

- The **database** is encrypted by default, but `encrypt_db = false` is a
  documented, supported configuration — in that mode the entire plaintext
  history is world-readable on any multi-user host.
- The **log file** never contains entry content (verified), but it does
  record entry ids, mime types, byte sizes, and store timing — activity
  metadata worth protecting.

Setting 0600 at creation closes both. SQLite's WAL/journal side-files copy
the main database file's permissions, so fixing the db file before first
write fixes those too.

## Current state

- `src/db.rs:22-43` — `Database::open()`:

```rust
let conn = Connection::open(path)?;

if encrypt {
    let key = get_encryption_key()?;
    conn.pragma_update(None, "key", &key)?;
}

conn.execute_batch("PRAGMA journal_mode=WAL;")?;
```

  `Connection::open` creates the file if missing, mode 0644 & ~umask. The
  WAL/SHM files are created at the `journal_mode=WAL` pragma (first write) —
  AFTER the point where you'll chmod, so they inherit 0600.

- `src/main.rs:61-87` — `setup_logging()`:

```rust
let log_path = config::data_dir().join("clipbro.log");
let log_file = std::fs::File::create(&log_path).ok();
```

- `DbError` has an `Io(#[from] std::io::Error)` variant (`src/db.rs:13-14`)
  — permission errors can use `?` directly.
- This crate is Linux-only (Wayland/COSMIC), so unconditional
  `std::os::unix` usage is fine — `libc` is already a dependency and the
  code has no cross-platform shims.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | all pass |
| Targeted | `cargo test permissions` | new tests pass |

## Scope

**In scope**:
- `src/db.rs` (chmod after `Connection::open`, + test)
- `src/main.rs` (chmod after log `File::create`)

**Out of scope**:
- `config.toml` permissions (`src/config.rs::write_default_config`) — the
  config holds no secrets (a path and booleans); leaving it readable is
  fine and useful.
- The systemd unit file (must stay readable by systemd).
- Existing files on users' disks — see Maintenance notes for the
  fix-on-open behavior that covers them.
- Log content changes (already content-free).

## Git workflow

- Branch: `improve/007-private-file-permissions`
- Single commit, e.g. `Create database and log files with 0600 permissions`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Restrict the database file

In `Database::open()` (`src/db.rs`), immediately after
`let conn = Connection::open(path)?;` and BEFORE the journal_mode pragma,
add:

```rust
let perms = std::os::unix::fs::PermissionsExt::from_mode(0o600);
std::fs::set_permissions(path, perms)?;
```

(Use the idiomatic form: `use std::os::unix::fs::PermissionsExt;` scoped
inside the function, then
`std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;`.)

Placement matters: before WAL activation so `-wal`/`-shm` inherit the mode,
and unconditionally (also when the file already existed — this silently
fixes databases created by older versions on every daemon start).

**Verify**: `cargo check --all-targets` → exit 0

### Step 2: Restrict the log file

In `setup_logging()` (`src/main.rs`), after the
`let log_file = std::fs::File::create(&log_path).ok();` line, add:

```rust
if log_file.is_some() {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(
        &log_path,
        std::fs::Permissions::from_mode(0o600),
    );
}
```

The `let _ =` is acceptable here (logging setup must never abort the
program), but pair it with nothing — there is no logger yet at this point
in startup to report to.

**Verify**: `cargo check --all-targets` → exit 0

### Step 3: Tests

In `src/db.rs` tests (model after `test_db()` / `insert_and_get_roundtrip`):

1. `database_file_is_owner_only` — create a db via the existing `test_db()`
   pattern but keep the tempdir handle (don't `mem::forget` — bind the dir
   in the test body so the path stays valid), then:

```rust
use std::os::unix::fs::PermissionsExt;
let mode = std::fs::metadata(&path)
    .unwrap()
    .permissions()
    .mode();
assert_eq!(mode & 0o777, 0o600);
```

2. `open_tightens_existing_loose_db` — `std::fs::write(&path, b"")` then
   chmod it 0o644, then `Database::open(&path, false)`... note: an empty
   file IS a valid SQLite database-to-be, so open succeeds; assert mode is
   0600 afterward.

**Verify**: `cargo test database_file_is_owner_only open_tightens` → both pass

## Test plan

Step 3 covers creation mode and the retrofit path. The log-file chmod has no
unit seam (it lives in `setup_logging`, which installs a global subscriber
and can only run once per process); verify manually if a session is
available: `rm ~/.local/share/clipbro/clipbro.log; cargo run -- status;
stat -c %a ~/.local/share/clipbro/clipbro.log` → `600`.

## Done criteria

- [ ] `cargo test` exits 0, including the 2 new permission tests
- [ ] `rg -n "from_mode\(0o600\)" src/` → 2 matches (db.rs, main.rs)
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

- `Database::open` no longer matches the excerpt (plan 001 moved things
  more than expected) — re-locate the `Connection::open` call; if the
  open/pragma ordering described in Current state no longer holds, STOP.
- The permission test fails with mode 0644 — set_permissions is being called
  on the wrong path (e.g. relative vs absolute); don't loosen the assert.

## Maintenance notes

- The unconditional chmod in `open()` retrofits existing user databases on
  next daemon start — mention this in the commit message.
- If a `--db-path` on another filesystem (e.g. FAT) ever errors on
  set_permissions, that surfaces as a clear `DbError::Io` at startup — the
  fail-fast is intentional for the database (silent fallback would defeat
  the purpose); the log chmod is best-effort by design.
- Reviewer: confirm chmod-before-WAL ordering in Step 1.
