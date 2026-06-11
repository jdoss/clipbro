# Plan 005: Cut overlay open latency — load syntax/theme sets once, stop blocking first paint on wl-paste

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/entry.rs src/overlay.rs`
> Plan 003 may have moved `run()` above the test module in overlay.rs and
> collapsed some `if`s — that's expected. Compare the excerpts below against
> live code; on any other structural mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW (LazyLock statics; one init step made async)
- **Depends on**: none (executes cleanly before or after 003)
- **Category**: perf
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

The overlay is a **fresh process spawned on every toggle** (the daemon
spawns `clipbro overlay` and kills it on hide/select — `src/daemon.rs:431`),
so everything in `Overlay::new()` happens before first paint, every single
time the user opens it. Two avoidable costs dominate:

1. `entry::highlight_text()` calls `two_face::syntax::extra_newlines()` and
   `ThemeSet::load_defaults()` **on every call** — deserializing the full
   bundled syntax/theme sets per entry. With up to 20 entries highlighted at
   open, the sets are rebuilt up to 20×.
2. `Overlay::new()` synchronously runs `wl-paste` to detect the active
   clipboard entry (`detect_active_entry`, a blocking
   `std::process::Command::output()` — a Wayland roundtrip before first
   frame).

After this plan the sets load once per process (≈20× less of that work per
open) and the active-entry detection happens asynchronously after the
surface is up, with the badge filling in a few ms later.

## Current state

- `src/entry.rs:501-553` — `highlight_text`; the per-call loads:

```rust
pub fn highlight_text(
    text: &str,
    is_dark: bool,
) -> (String, Vec<([u8; 4], String)>) {
    let ss =
        two_face::syntax::extra_newlines();
    let ts =
        syntect::highlighting::ThemeSet::load_defaults();
```

- `src/entry.rs:555-557` — the repo's existing lazy-static pattern (model
  the fix on this):

```rust
static URL_PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^https?://\S+$").unwrap()
});
```

- `src/entry.rs:98-105` — `detect_syntax_name` (marked `#[allow(dead_code)]`)
  also calls `extra_newlines()`; switch it to the static too.
- `src/overlay.rs:194-195` — the blocking call in `new()`:

```rust
let active_entry_id =
    detect_active_entry(&entries);
```

- `src/overlay.rs:1348-1369` — `detect_active_entry` today: runs
  `wl-paste --no-newline` via `std::process::Command::output()`, then
  compares stdout bytes to each entry's `text_content()`.
- `src/overlay.rs:44-67` — the `Message` enum (you will add a variant).
- `src/overlay.rs:266-287` — `new()` currently returns
  `(overlay, init_task)` where `init_task = get_layer_surface(...)`.
- `build_highlights` (`src/overlay.rs:926-961`) calls `highlight_text` per
  text entry — no change needed there; it benefits automatically.
- The iced executor is tokio (libcosmic is built with the `tokio` feature —
  `Cargo.toml:40-43`), so `tokio::process` works inside `Task::perform`
  futures.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | all pass |
| Targeted | `cargo test entry::` and `cargo test overlay::` | all pass |
| Pattern gone | `rg -n "extra_newlines\(\)\|load_defaults\(\)" src/` | matches only inside the two LazyLock initializers |

## Scope

**In scope**:
- `src/entry.rs` (LazyLock statics; `highlight_text`, `detect_syntax_name`)
- `src/overlay.rs` (async active-entry detection; new Message variant; make
  the matching logic a pure, tested function)

**Out of scope**:
- `Database::open`/keyring timing in `new()` — measured as a one-time
  D-Bus roundtrip, deliberately deferred (see Maintenance notes).
- `build_handles` / thumbnail decoding.
- `dbus::query_paused()` in `view()` — that is plan 006; don't fix it here
  even though you'll see it.
- Any change to highlighting output or theme choice.

## Git workflow

- Branch: `improve/005-overlay-open-latency`
- Two commits: (1) entry.rs statics, (2) overlay async detection.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Cache the syntax and theme sets in entry.rs

Add module-level statics next to `URL_PATTERN` (match its style):

```rust
static SYNTAX_SET: std::sync::LazyLock<syntect::parsing::SyntaxSet> =
    std::sync::LazyLock::new(two_face::syntax::extra_newlines);

static THEME_SET: std::sync::LazyLock<syntect::highlighting::ThemeSet> =
    std::sync::LazyLock::new(syntect::highlighting::ThemeSet::load_defaults);
```

(If `two_face::syntax::extra_newlines`'s return type is a two_face re-export
rather than `syntect::parsing::SyntaxSet`, use the type the compiler names —
check `cargo doc -p two-face --no-deps` or the compile error.)

In `highlight_text`, replace the two `let ss = ...; let ts = ...;` bindings
with `let ss = &*SYNTAX_SET; let ts = &*THEME_SET;` and keep the rest of the
body unchanged. In `detect_syntax_name`, replace its `extra_newlines()` call
with `&*SYNTAX_SET`.

**Verify**: `cargo test entry::` → all pass (the two highlight tests prove
output is unchanged)
**Verify**: `rg -n "extra_newlines()" src/` → exactly 1 match (the static initializer); `rg -n "load_defaults" src/` → exactly 1 match

### Step 2: Make active-entry detection pure + async

1. In `src/overlay.rs`, split `detect_active_entry` into:

```rust
fn match_active_entry(
    entries: &[Entry],
    clip: &[u8],
) -> Option<i64> {
    for entry in entries {
        if let Some(t) = entry.text_content() {
            if t.as_bytes() == clip {
                return Some(entry.id);
            }
        }
    }
    None
}
```

2. Add a Message variant: `ActiveClipboard(Option<Vec<u8>>)`.
3. In `new()`: set `active_entry_id: None` in the struct literal, and change
   the returned task to a batch of the layer-surface task plus the fetch:

```rust
let fetch_task = Task::perform(
    async {
        let out = tokio::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            .await
            .ok()?;
        if !out.status.success()
            || out.stdout.is_empty()
        {
            return None;
        }
        Some(out.stdout)
    },
    Message::ActiveClipboard,
);

(overlay, Task::batch([init_task, fetch_task]))
```

4. In `update()`, handle the new message:

```rust
Message::ActiveClipboard(clip) => {
    if let Some(clip) = clip {
        self.active_entry_id =
            match_active_entry(
                &self.entries, &clip,
            );
    }
}
```

5. Delete the old `detect_active_entry` function entirely (no dead code).

**Verify**: `cargo check --all-targets` → exit 0; `rg -n "detect_active_entry" src/` → no matches

### Step 3: Test the pure matcher

In `src/overlay.rs` `mod tests` (alongside the ParsedHotkey tests,
line 1518+), add tests for `match_active_entry`. Build entries the way
`src/entry.rs` tests do (`Entry { id, created_at: 0, entry_type, favorite:
false, contents }` with a `text/plain;charset=utf-8` map):

1. matches the entry whose text equals the clip bytes exactly
2. returns None when no entry matches
3. returns None for an image-only entry even if bytes are equal (text_content is None)
4. empty entries slice → None

**Verify**: `cargo test overlay::` → all pass including 4 new tests

### Step 4: Smoke-check behavior didn't change

Full suite + a build of the binary:

**Verify**: `cargo test` → all pass; `cargo build` → exit 0

If you have a live COSMIC session available (optional, not required):
`cargo run -- toggle` twice and confirm the overlay still marks the current
clipboard entry with the 📋 badge shortly after opening.

## Test plan

Step 1 relies on the two existing `highlight_text` tests
(`entry::tests::highlight_text_returns_language_and_spans`,
`highlight_text_plain_text`) to prove identical behavior. Step 3 adds 4 unit
tests for the newly-pure matcher — these are NEW coverage that the old
subprocess-coupled function couldn't have.

## Done criteria

- [ ] `cargo test` exits 0
- [ ] `rg -n "extra_newlines\(\)" src/` → 1 match (static init only)
- [ ] `rg -n "load_defaults" src/` → 1 match (static init only)
- [ ] `rg -n "std::process::Command" src/overlay.rs` → only the `xdg-open` call in `Message::OpenUrl` remains
- [ ] 4 new `match_active_entry` tests pass
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

- The compiler reports `tokio::process` cannot run in the Task future (would
  mean the iced executor is not tokio in this build config) — report; the
  fallback design (spawn_blocking + std::process) needs an advisor decision.
- `highlight_text` tests fail after Step 1 (the statics must be
  behavior-neutral; if they aren't, something else changed).
- `Task::batch` doesn't exist in this iced version under that name — find
  the equivalent (`Task::batch` exists in iced 0.14/libcosmic; if renamed,
  check how `src/overlay.rs` composes tasks elsewhere) — if no equivalent in
  10 minutes of looking, STOP.

## Maintenance notes

- The remaining `new()` costs are `Database::open` (one keyring D-Bus
  roundtrip when `encrypt_db = true`, plus a throwaway tokio runtime built in
  `get_encryption_key` — `src/db.rs:579-589`) and `build_handles` thumbnail
  decode. If open latency is still noticeable after this plan, those are the
  next targets — they need a loading-state UI, which is why they were
  deferred.
- Plan 011 (full-history search) will raise the entry count the overlay
  loads; the LazyLock change here is a prerequisite for that to stay fast.
  If 011 lands, `build_highlights` should become lazy/incremental.
- Reviewer: confirm the 📋 active badge still appears (now ~one frame later),
  and that `match_active_entry` compares raw bytes (not trimmed/lossy
  strings) exactly like the old code did.
