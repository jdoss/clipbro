# Plan 006: Stop creating a D-Bus connection on every overlay render frame

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/overlay.rs src/dbus.rs`
> Plans 003/005 legitimately touched overlay.rs. Compare the excerpts below
> against live code; if `view()` no longer calls `dbus::query_paused()`,
> this plan is already done — mark it REJECTED (superseded) in the index.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (small merge overlap with plan 005 in `Overlay::new`/`update` — execute sequentially, either order)
- **Category**: perf
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

`Overlay::view()` calls `dbus::query_paused()` on **every render frame**.
That function creates a brand-new `zbus::blocking::Connection::session()`
and performs a synchronous property Get — two D-Bus roundtrips plus a
connection handshake, on the UI thread, per frame (every keystroke,
navigation, hover repaint). The pause state changes at most when the user
presses the pause hotkey. After this plan the overlay queries once at
startup, keeps the state in the struct, and flips it locally when the user
toggles.

## Current state

- `src/overlay.rs:688` — in `view()`:

```rust
let paused = dbus::query_paused();
```

  followed by `if paused { ... }` building the amber PAUSED badge
  (lines 725–754). `view()` takes `&self`.

- `src/dbus.rs:24-57` — `query_paused()`: blocking session connection +
  `org.freedesktop.DBus.Properties.Get` for `Paused`; returns `false` on any
  error.

- `src/overlay.rs:151-166` — the `Overlay` struct (you will add a field).
  Current fields end with `is_dark: bool, db: Database`.

- `src/overlay.rs:430-447` — the toggle handler in `update()`:

```rust
Message::TogglePause => {
    return Task::perform(
        async move {
            let action =
                dbus::PopupAction::TogglePause;
            if let Err(e) =
                dbus::send_action(action)
                    .await
            {
                tracing::error!(
                    "Failed to send \
                     pause toggle: {e}"
                );
            }
        },
        |_| Message::PauseSent,
    );
}
```

- `Message::PauseSent` exists and is currently a no-op arm (`src/overlay.rs:429`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | all pass |
| Pattern gone | `rg -n "query_paused" src/overlay.rs` | exactly 1 match, inside `new()` |

## Scope

**In scope**: `src/overlay.rs`

**Out of scope**:
- `src/dbus.rs` — `query_paused()` stays as-is (still used once at init, and
  its error-→-false behavior is fine for a badge).
- The daemon's pause logic (`src/daemon.rs`) and the `Paused` D-Bus property.
- Live-updating the badge if pause is toggled externally (`clipbro pause`
  CLI) while the overlay is open — explicitly accepted staleness; see
  Maintenance notes.

## Git workflow

- Branch: `improve/006-cache-paused-state`
- Single commit, e.g. `Cache pause state instead of querying D-Bus per frame`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the field and initialize it once

In the `Overlay` struct add `paused: bool,`. In `new()`, in the struct
literal (around `src/overlay.rs:250-264`), add `paused: dbus::query_paused(),`
— one blocking query during process init is acceptable (it replaces hundreds
per second during use).

**Verify**: `cargo check --all-targets` → exit 0

### Step 2: Use the cached value in view()

Replace `let paused = dbus::query_paused();` (line ~688) with
`let paused = self.paused;`.

**Verify**: `rg -n "query_paused" src/overlay.rs` → 1 match (in `new()`)

### Step 3: Flip locally on toggle

In the `Message::TogglePause` arm, add `self.paused = !self.paused;` as the
first statement (before the `return Task::perform(...)`). The optimistic
flip makes the badge respond instantly; the D-Bus action still goes to the
daemon, and `PauseSent` remains a no-op acknowledgment.

**Verify**: `cargo test` → all pass

## Test plan

The pause rendering path is iced `view()` code with no seam for headless
unit tests; behavior-neutrality is covered by the full suite plus, if a
COSMIC session is available (optional): open the overlay, press Ctrl+P —
PAUSED badge appears immediately; press again — disappears; `clipbro pause`
from a terminal still toggles the daemon (verify with `clipbro status` /
re-opening the overlay).

## Done criteria

- [ ] `cargo test` exits 0
- [ ] `rg -n "query_paused" src/overlay.rs` → exactly 1 match, in `new()`
- [ ] `rg -n "self.paused" src/overlay.rs` → ≥2 matches (view read + toggle flip)
- [ ] `git status` clean outside `src/overlay.rs`
- [ ] `plans/README.md` status row updated

## STOP conditions

- `view()` no longer matches the excerpt (already fixed or refactored) —
  mark superseded in the index and report.
- Adding the field breaks an exhaustive struct construction somewhere other
  than `new()` (there shouldn't be one — `Overlay` is only constructed in
  `new()`; if another constructor exists, the codebase drifted).

## Maintenance notes

- Accepted limitation: if the user runs `clipbro pause` from a terminal
  while the overlay is open, the badge won't update until the overlay is
  reopened (overlay processes are short-lived, so the window is seconds).
  If this ever matters, subscribe to the zbus `PropertiesChanged` signal
  instead — that's the follow-up, not a partial fix here.
- Plan 010 (extraction refactor) touches `update()` — land this first so the
  extraction moves the final version.
