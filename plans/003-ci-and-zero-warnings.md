# Plan 003: Add CI, fix all clippy warnings, drop the dead nucleo dependency, and make installs reproducible

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/ Cargo.toml README.md`
> Plans 001/002 legitimately changed `src/db.rs`, `src/daemon.rs`, and
> `README.md` before this plan. Re-run `cargo clippy --all-targets 2>&1 |
> grep -c warning` to get the live warning count before starting — the
> specific warnings below were counted at d7a7c18 and may have shifted
> slightly.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW (lint fixes are mechanical; CI is additive)
- **Depends on**: plans/001-fix-entry-id-collisions.md (CI gates on `cargo test`; the suite is flaky until 001 lands)
- **Category**: dx
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

Nothing runs the test suite automatically — the repo has no `.github/`
directory at all. The suite itself is fast and headless-friendly (102 tests,
~0.1s, verified to need no D-Bus, Wayland, or keyring: db tests use tempfile
databases with `encrypt=false`, daemon tests use a mockall clipboard), so CI
is cheap to add and the only real obstacle is compiling the libcosmic git
dependency. Additionally: `cargo clippy --all-targets` emits 26 warnings,
`nucleo` is declared in Cargo.toml but never imported anywhere (dead
dependency — overlay search is plain `contains`), and the README's install
command (`cargo install --git ...`) ignores Cargo.lock, so users build
against an unpinned floating libcosmic instead of the locked
`cca48bc2` revision.

## Current state

- No `.github/` directory.
- `Cargo.toml:15` — `nucleo = "0.5"` (confirmed unused: `rg nucleo src/`
  returns nothing).
- `Cargo.toml:40-43` — libcosmic git dependency, no `rev` pin; Cargo.lock
  pins it to `cca48bc2`:

```toml
[dependencies.libcosmic]
git = "https://github.com/pop-os/libcosmic"
default-features = false
features = ["tokio", "wayland", "winit"]
```

- `README.md:53` — `cargo install --git https://github.com/jdoss/clipbro`
  (no `--locked`).
- Clippy warnings at d7a7c18 (counts from `cargo clippy --all-targets`):
  - 20× `collapsible_if`
  - 3× `items_after_test_module` — code defined below `#[cfg(test)] mod tests`:
    `get_encryption_key`/`load_or_create_key` in `src/db.rs` (lines 579–628),
    `run()` in `src/daemon.rs` (lines 1317–1373), `run()` in
    `src/overlay.rs` (lines 1586–1600). Fix by moving the items ABOVE the
    test module in each file (do not move the test modules).
  - 1× `type_complexity` — `position_settings` return type in
    `src/overlay.rs:894-896`; fix with a module-level type alias, e.g.
    `type SurfaceGeometry = (layer_surface::Anchor, Option<(Option<u32>, Option<u32>)>);`
  - 1× `let_and_return`, 1× unnecessary `usize`→`usize` cast — locations via
    `cargo clippy` output; both are auto-fixable.
- Toolchain on the dev machine: `cargo 1.96.0` (edition 2024 requires a
  recent stable — use `stable` in CI, not a pinned old version).
- Repo style: NOT rustfmt-formatted (`cargo fmt --check` shows diffs).
  **Decision already made: do NOT add `cargo fmt --check` to CI and do not
  reformat the repo.** Record nothing further; this was considered and
  deferred deliberately.
- The crate links SQLCipher via rusqlite's `bundled-sqlcipher` feature,
  which needs OpenSSL development headers at build time.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 (after fixes) |
| Auto-fix | `cargo clippy --fix --all-targets --allow-dirty` | applies ~22 mechanical fixes |
| Tests | `cargo test` | all pass |
| Unused dep check | `rg -n "nucleo" src/` | no matches (before and after removal) |
| Workflow lint | `actionlint .github/workflows/ci.yml` | exit 0, no findings |
| Workflow audit | `zizmor .github/workflows/ci.yml` | no high-severity findings |

## Scope

**In scope**:
- `.github/workflows/ci.yml` (create)
- `Cargo.toml` (remove nucleo), `Cargo.lock` (regenerates via `cargo check`)
- `README.md` (install command)
- `src/db.rs`, `src/daemon.rs`, `src/overlay.rs`, and any other file clippy
  flags — lint fixes only, no behavior changes

**Out of scope**:
- Reformatting with rustfmt (deliberately deferred — see Current state).
- Pinning libcosmic to a `rev` in Cargo.toml. Decision: the lockfile plus
  `--locked` installs are sufficient, and the maintainer bumps deps
  regularly (see branch `deps/update-2026-06`); a rev pin would just be a
  second place to update. Do not add one.
- Release packaging, publishing, badges.

## Git workflow

- Branch: `improve/003-ci-zero-warnings`
- One commit per logical unit: clippy fixes / nucleo removal / README / workflow.
- Do NOT push or open a PR unless the operator instructed it (note: CI can only fully run remotely; see Done criteria).

## Steps

### Step 1: Remove the dead nucleo dependency

Confirm `rg -n "nucleo" src/` → no matches. Delete the `nucleo = "0.5"` line
from `Cargo.toml`, then run `cargo check --all-targets` to update Cargo.lock.

**Verify**: `cargo check --all-targets` → exit 0; `rg nucleo Cargo.toml` → no match

### Step 2: Fix the mechanical clippy warnings

Run `cargo clippy --fix --all-targets --allow-dirty`, then review the diff
(`git diff`) — expect collapsed `if` nesting and removed casts only. The
repo's hand-wrapped style means `--fix` output may look denser than
surrounding code; that's acceptable for these lines.

**Verify**: `cargo clippy --all-targets 2>&1 | grep -c "^warning"` → only the 3 `items_after_test_module` + 1 `type_complexity` (and possibly `let_and_return`) remain

### Step 3: Fix the structural clippy warnings by hand

1. `src/db.rs`: move `get_encryption_key()` and `load_or_create_key()`
   (currently lines 579–628, AFTER `mod tests`) to just above
   `#[cfg(test)] mod tests`.
2. `src/daemon.rs`: move `pub async fn run(...)` (lines 1317–1373) above the
   test module.
3. `src/overlay.rs`: move `pub fn run()` (lines 1586–1600) above the test
   module.
4. `src/overlay.rs`: add the type alias for `position_settings`'s return
   type (see Current state) and use it in the signature.

**Verify**: `cargo clippy --all-targets -- -D warnings` → exit 0
**Verify**: `cargo test` → all pass (moves must not change behavior)

### Step 4: Make the README install reproducible

`README.md:53`: change to

```sh
cargo install --locked --git https://github.com/jdoss/clipbro
```

**Verify**: `rg -n "install --locked --git" README.md` → 1 match

### Step 5: Create the CI workflow

Create `.github/workflows/ci.yml`. Before writing it, verify the current
major versions of `actions/checkout`, `dtolnay/rust-toolchain`, and
`Swatinem/rust-cache` (web search or `gh api repos/<owner>/<repo>/releases/latest`)
— do not trust the versions memorized below if newer majors exist.

```yaml
name: CI

on:
  push:
    branches: [master]
  pull_request:

permissions:
  contents: read

jobs:
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - name: Install system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends \
            pkg-config libssl-dev libwayland-dev libxkbcommon-dev
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - name: Clippy
        run: cargo clippy --all-targets --locked -- -D warnings
      - name: Tests
        run: cargo test --locked
```

Notes for the executor:
- `libssl-dev` is for rusqlite's `bundled-sqlcipher`; `libxkbcommon-dev` for
  the xkbcommon crate; `libwayland-dev`+`pkg-config` for the Wayland stack.
  If the CI build later fails on a missing native library, add that one
  package — after two unknown failures, STOP and report rather than guessing.
- First uncached compile of libcosmic takes a long time — hence
  `timeout-minutes: 45` and rust-cache.

**Verify**: `actionlint .github/workflows/ci.yml` → exit 0
**Verify**: `zizmor .github/workflows/ci.yml` → no high-severity findings

## Test plan

No new unit tests — this plan's "tests" are the gates themselves:
clippy clean with `-D warnings`, full suite green, workflow lints clean.
Behavior-neutrality of Step 2/3 is proven by the unchanged test suite.

## Done criteria

- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo test` exits 0
- [ ] `rg nucleo Cargo.toml src/` → no matches
- [ ] `README.md` install command includes `--locked`
- [ ] `actionlint` and `zizmor` pass on the workflow
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated (mark DONE-pending-remote-run: the workflow's first real execution happens after the operator pushes)

## STOP conditions

- `cargo clippy --fix` produces a diff that changes anything other than `if`
  collapsing / cast removal / let-return simplification — revert and report.
- After Step 3 the test count drops (a moved item accidentally landed inside
  `#[cfg(test)]`).
- More than two iterations of "add a missing apt package" would be needed to
  make a local container/CI build proceed.
- `rg nucleo src/` matches anything (a use appeared since d7a7c18 — drop
  Step 1 and report).

## Maintenance notes

- When plans 005–010 land, CI gates them automatically — this is why this
  plan runs early.
- Reviewer: check the moved functions in Step 3 are byte-identical (pure
  moves), and that the workflow has `permissions: contents: read` (least
  privilege).
- Deferred deliberately: rustfmt adoption (whole-repo churn; maintainer's
  call), cargo-audit/cargo-deny in CI (worth adding later; needs a decision
  on alert noise), release builds.
