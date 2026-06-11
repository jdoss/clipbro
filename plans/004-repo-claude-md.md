# Plan 004: Write a repo CLAUDE.md so agents and contributors can build, test, and navigate clipbro correctly

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/ Cargo.toml`
> If plans 001–003 landed, the facts below still hold (they were written to
> account for those plans). Verify each command in the drafted file actually
> works before committing it — that IS the verification for this plan.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (documentation only)
- **Depends on**: plans/003-ci-and-zero-warnings.md (soft — so the documented lint gate matches reality; if 003 hasn't landed, omit the `-D warnings` claim)
- **Category**: dx
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

The repo has no CLAUDE.md/AGENTS.md. Every future agent session re-derives
the same facts: how to build, that libcosmic is a slow git dependency with
system-package prerequisites, that the test suite is headless-safe, that the
repo deliberately does not use rustfmt, and which modules own what. Plans
005–012 in this directory will be executed by agents that benefit
immediately. This is a single small file with outsized leverage.

## Current state

- Repo root contains README.md (user-facing), no agent/contributor docs.
- Verified facts to encode (all confirmed at d7a7c18):
  - `cargo check --all-targets` ~7s warm; first cold build of libcosmic is
    many minutes.
  - `cargo test` → 102 tests in ~0.1s, fully headless: no D-Bus, Wayland,
    or keyring needed (db tests use `tempfile` + `encrypt=false`; daemon
    tests use `MockClipboardService` from `mockall`).
  - Runtime (not build/test) requirements: Wayland + COSMIC, `wl-clipboard`,
    a secret-service provider for the default `encrypt_db = true`.
  - Style: NOT rustfmt-formatted — `cargo fmt` must never be run repo-wide;
    match surrounding hand-wrapped style.
  - Module map: see the file body below.
  - Process architecture: one daemon process (spawns `wl-paste --watch`
    subprocesses that pipe into `clipbro store`, which forwards over D-Bus);
    the overlay is a separate short-lived process (`clipbro overlay`) spawned
    per toggle; IPC via session D-Bus name `io.github.jdoss.clipbro`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Verify build claim | `cargo check --all-targets` | exit 0 |
| Verify test claim | `cargo test` | all pass, no hangs (headless) |
| Verify lint claim | `cargo clippy --all-targets` | exit 0 (with `-D warnings` if 003 landed) |

## Scope

**In scope**: `CLAUDE.md` (create, repo root)

**Out of scope**: README.md, any source file, `.claude/` settings.

## Git workflow

- Branch: `improve/004-claude-md`
- Single commit, e.g. `Add CLAUDE.md with build, test, and architecture notes`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Write CLAUDE.md

Create `CLAUDE.md` at the repo root with exactly this content (adjust only
where a later plan changed reality — e.g. drop the `-D warnings` mention if
plan 003 is not DONE):

```markdown
# clipbro — agent/contributor notes

Clipboard manager daemon for the COSMIC desktop (Wayland). Rust, edition 2024.

## Build, test, lint

- `cargo check --all-targets` — fast typecheck (~7s warm). The first cold
  build compiles the libcosmic git dependency and takes many minutes.
- `cargo test` — full suite, ~0.1s, fully headless (no Wayland/D-Bus/keyring
  needed). This is the verification gate for every change.
- `cargo clippy --all-targets -- -D warnings` — must stay clean.
- Do NOT run `cargo fmt` repo-wide: the codebase predates rustfmt and is
  deliberately not formatted with it. Match the surrounding style.

Build-time system deps (for the libcosmic/Wayland/SQLCipher stack):
`pkg-config`, OpenSSL headers (`libssl-dev`/`openssl-devel`),
`libwayland-dev`, `libxkbcommon-dev`.
Runtime deps: COSMIC/Wayland session, `wl-clipboard` (`wl-copy`/`wl-paste`),
and a secret-service provider (GNOME Keyring/KDE Wallet/oo7) unless
`encrypt_db = false`.

## Architecture (two processes + helpers)

- **Daemon** (`clipbro`, no subcommand): owns the database and clipboard
  watching. Spawns `wl-paste --watch clipbro store ...` subprocesses (text,
  primary, image); they pipe clipboard payloads back into short-lived
  `clipbro store` helper processes that forward over D-Bus.
- **Overlay** (`clipbro overlay`): separate iced/libcosmic layer-shell
  process, spawned fresh by the daemon on every toggle and killed on
  hide/select. Anything done in `Overlay::new()` delays first paint.
- IPC: session D-Bus, name `io.github.jdoss.clipbro` (src/dbus.rs).

## Module map

- `src/main.rs` — clap CLI, logging setup, `clipbro store` ingestion helper
  (image magic-byte sniffing for Chromium's prefixed payloads).
- `src/daemon.rs` — event loop, store pipeline (pause → debounce → echo/dedup
  windows → insert → sync), watcher respawn, overlay process lifecycle,
  thumbnails.
- `src/overlay.rs` — iced UI: search, filters, cards, hotkeys, selection.
- `src/entry.rs` — content-type detection (regex URL + heuristics +
  tree-sitter scoring), syntect highlighting, Entry accessors.
- `src/db.rs` — rusqlite + SQLCipher (key from oo7 keyring), schema +
  migrations, CRUD.
- `src/config.rs` — TOML config at `~/.config/clipbro/config.toml`.
- `src/dbus.rs` — zbus service + client helpers. `src/clipboard.rs` —
  wl-copy wrapper behind the mockable `ClipboardService` trait.
  `src/systemd.rs` — user-service install/start/stop via systemd D-Bus API.

## Testing conventions

- Tests are inline `#[cfg(test)] mod tests` per file (no tests/ dir).
- DB tests: `tempfile` dir + `Database::open(&path, false)` — never the
  real keyring.
- Daemon tests: `MockClipboardService` (mockall) + real tempfile db; assert
  on resulting DB state, not mock call sequences.
- Keep tests headless: nothing in the suite may require Wayland, a session
  bus name, or a keyring.

## Gotchas

- Clipboard contents are sensitive — never log entry content/text, only ids,
  mime types, and sizes.
- The overlay process exits via `iced::exit()`; the daemon reaps it and
  enforces a 200ms respawn gap (compositor teardown race — see
  `OVERLAY_RESPAWN_GAP` in src/daemon.rs).
- `plans/` contains advisor-written implementation plans with their own
  index and status table.
```

**Verify**: file exists; every command claimed in it has been run in this
session with the stated result (`cargo check --all-targets`, `cargo test`,
`cargo clippy --all-targets`).

### Step 2: Cross-check facts against the live tree

- `rg -n "OVERLAY_RESPAWN_GAP" src/daemon.rs` → found (else fix the file).
- `rg -n "io.github.jdoss.clipbro" src/dbus.rs` → found.
- Test count in `cargo test` output ≥ 102 — update the number in the file to
  the live count.

**Verify**: all three checks pass and the file's numbers match reality

## Test plan

Not applicable (docs). The verification is that each documented command was
executed and behaved as documented.

## Done criteria

- [ ] `CLAUDE.md` exists at repo root
- [ ] Every command in it was run during execution with the documented result
- [ ] `git status` shows only `CLAUDE.md` (and the plans index) modified
- [ ] `plans/README.md` status row updated

## STOP conditions

- A documented claim fails verification (e.g. `cargo test` is not headless
  on this machine) — report the discrepancy instead of writing it down.
- A `CLAUDE.md` already exists at the repo root (it didn't at d7a7c18).

## Maintenance notes

- Update the module map if plan 010 extracts `src/hotkey.rs`/`src/filter.rs`.
- Reviewer: confirm no aspirational claims — the file must describe what IS,
  per the maintainer's no-phantom-features rule.
