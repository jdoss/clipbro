# Plan 013: Stop the overlay and CLI commands from truncating the daemon's log

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat 4cddd6e..HEAD -- src/main.rs`
> If `setup_logging()` changed since this plan was written, compare the
> "Current state" excerpt against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (single-line change to how one log file is opened)
- **Depends on**: none
- **Category**: bug / DX
- **Planned at**: commit `4cddd6e`, 2026-07-21 (found while live-debugging a clipboard report)

## Why this matters

`setup_logging()` opens the shared log with `std::fs::File::create`, which
**truncates**. It is called by three process roles that all write to the same
`data_dir/clipbro.log`:

- the daemon (`main.rs` `None` arm),
- the overlay (`clipbro overlay`, spawned on every Alt+Z),
- every CLI action (`clipbro toggle|show|hide|clear|pause`).

So **every overlay open and every CLI action wipes the daemon's running log.**
Observed live: the daemon had been up 21h, but its log held only 25 lines
starting at the last overlay spawn — the entire repro history for an active
debugging session was destroyed by a single Alt+Z. This makes the daemon
effectively un-debuggable in exactly the situations you most need the log.

After this plan: all roles **append** to one shared timeline; nothing
truncates a running daemon's history.

## Current state

`src/main.rs:61-87` (`setup_logging`), the offending line at 68:

```rust
fn setup_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,clipbro=info"));

    let stderr_layer = fmt::layer().with_target(true);

    let log_path = config::data_dir().join("clipbro.log");
    let log_file = std::fs::File::create(&log_path).ok();   // <-- truncates
    let file_layer = log_file.map(|f| {
        fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
    });
    ...
}
```

Callers (all pass no args today): `Command::Overlay` arm (`main.rs:115`), the
`Some(command)` CLI arm (`main.rs:120`), the daemon `None` arm (`main.rs:149`).
`Init`/`Install`/`Start`/`Stop`/`Restart`/`Status`/`Store` do **not** call it.

`config::data_dir()` resolves to `~/.local/share/clipbro` (`src/config.rs:91`).

Repo conventions: hand-wrapped formatting (no rustfmt); errors/logging via
`tracing`.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Lint | `cargo clippy --all-targets` | no NEW warnings beyond the pre-existing baseline |
| Tests | `cargo test` | unchanged pass count |
| Manual proof | see Step 2 | daemon log survives an overlay open |

Note: this build needs `RUSTC_BOOTSTRAP=1` (see repo memory / build quirk).

## Scope

**In scope**: `src/main.rs` (`setup_logging` only).
**Out of scope**: log rotation/size-capping (see Maintenance notes), the
`data_dir`/path logic, and any other logging behavior.

## Steps

### Step 1: Open the log in append mode instead of truncating

Replace the `File::create` line with an append-open:

```rust
let log_file = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)
    .ok();
```

Rationale for append (not "daemon truncates on start"): append never loses
history, including a crash log that predates a systemd restart — the exact
thing you want to read after a crash. Growth is the tradeoff and is deferred
(Maintenance notes).

**Verify**: `cargo check --all-targets` → exit 0.

### Step 2: Prove the truncation is gone (manual)

The function initializes global `tracing` state and writes to `data_dir`, so
it is not unit-testable in the repo's style. Verify by hand:

```sh
LOG=~/.local/share/clipbro/clipbro.log
wc -l "$LOG"                 # note the current line count
clipbro overlay & sleep 1; kill %1 2>/dev/null   # or just press Alt+Z twice
wc -l "$LOG"                 # must be >= the previous count, never reset to a
                             # few lines
```

**Expected**: the line count does not drop after an overlay open. Before this
fix it collapses to the overlay's first few lines.

## Done criteria

- [ ] `cargo check --all-targets` exits 0
- [ ] `cargo clippy --all-targets` adds no new warnings
- [ ] `cargo test` pass count unchanged
- [ ] `rg -n "File::create" src/main.rs` → no match (the log open no longer truncates)
- [ ] Manual Step 2 shows the log surviving an overlay open
- [ ] `plans/README.md` status row updated

## STOP conditions

- `setup_logging` at the cited lines doesn't match the excerpt (drift).
- You discover a caller that *relies* on the log being truncated (search:
  `rg -n "setup_logging" src/`) — there is none today; if one appears, stop.

## Maintenance notes

- **Deferred: log growth / rotation.** Append means the file grows without
  bound. A giant single whitespace line was seen in the wild (likely a panic
  backtrace or an overlay data dump), so growth is not purely theoretical. A
  follow-up could add size-capped rotation, or scope the file layer to the
  daemon only (overlay/CLI → stderr, which systemd already routes to
  journald). Both are larger than this bug fix and intentionally out of scope.
- Alternative considered and rejected for now: `setup_logging(truncate: bool)`
  with the daemon truncating on startup. Cleaner per-boot logs, but it throws
  away pre-crash history on a systemd restart — worse for debugging, which is
  the whole point of the file.
