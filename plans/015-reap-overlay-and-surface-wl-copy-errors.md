# Plan 015: Reap a self-exited overlay promptly and surface wl-copy failures

> **Executor instructions**: Follow step by step; verify each step. Honor STOP
> conditions. Update `plans/README.md` when done.
>
> **Drift check (run first)**: `git diff --stat 4cddd6e..HEAD -- src/daemon.rs src/clipboard.rs`

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: hygiene / error-handling
- **Planned at**: commit `4cddd6e`, 2026-07-21

## Why this matters

Two small, real issues found while debugging (the larger "wl-copy zombie
accumulation" claim was **not** confirmed — the detached `wl-copy` are the
required, superseded-on-next-copy selection servers, and tokio reaps the
foreground children; see `plans/README.md` rejected findings):

1. **A self-exited overlay isn't reaped until the next D-Bus action.** When the
   user closes the overlay itself (Esc), the daemon still holds the `Child` in
   `overlay_child` and only reaps it the next time `overlay_running()` /
   `kill_overlay()` runs — i.e. on the next Toggle/Show/Hide. Observed a
   `[clipbro] <defunct>` zombie **11.5 minutes** old, ppid = daemon, because no
   action had occurred since. Harmless but untidy, and it also means
   `dbus::set_visible(false)` lags (the tray/state thinks the overlay is still
   up).
2. **`wl_copy()` swallows the child's exit status.** If `wl-copy` fails
   (missing binary, spawn error mid-run, non-zero exit), clipbro logs nothing
   and proceeds as if the clipboard were set. That violates the repo's
   "never swallow errors silently" rule and would make a real serve failure
   invisible.

## Current state

`src/daemon.rs` run loop (`run()`, ~lines 1353-1369): the `tokio::select!` has
a `watcher_check.tick()` arm calling `daemon.check_watchers()` every 5s. There
is no periodic overlay reap.

`src/daemon.rs::overlay_running()` (lines 388-410) already `try_wait()`s and,
on exit, clears `overlay_child`, records `last_overlay_exit`, and calls
`dbus::set_visible(false)`. It just isn't called unless an action arrives.

`src/clipboard.rs::wl_copy()` (lines 70-94):

```rust
async fn wl_copy(args: &[&str], data: &[u8]) {
    let mut child = match tokio::process::Command::new("wl-copy") ... spawn() {
        Ok(child) => child,
        Err(e) => { tracing::error!("wl-copy spawn failed: {e}"); return; }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(data).await {
            tracing::error!("wl-copy stdin write failed: {e}");
            let _ = child.kill().await;
        }
    }
    // returns here; child dropped without wait — exit status never checked
}
```

Note: `wl-copy` forks a background server and its **foreground** process exits
quickly after reading stdin, so awaiting the foreground does not block on the
selection's lifetime.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test` | unchanged pass count |
| Lint | `cargo clippy --all-targets` | no new warnings |

Build needs `RUSTC_BOOTSTRAP=1`.

## Scope

**In scope**: `src/daemon.rs` (`run()` loop reap), `src/clipboard.rs`
(`wl_copy` exit check).
**Out of scope**: changing the wl-copy-per-selection serving model, tracking
selection owners, or the X11 path (plan 014).

## Steps

### Step 1: Reap a self-exited overlay in the periodic tick

In `run()`'s `tokio::select!`, in the `watcher_check.tick()` arm, call the
existing reaper alongside `check_watchers()`:

```rust
_ = watcher_check.tick() => {
    daemon.check_watchers();
    daemon.overlay_running();   // reaps a self-exited overlay, updates visibility
}
```

`overlay_running()` returns `bool` — discard it (or `let _ =`). This bounds the
zombie/visibility lag to one `WATCHER_CHECK_INTERVAL` (5s) instead of "until
the next user action."

**Verify**: `cargo check --all-targets` → 0. Manually: open the overlay, press
Esc, wait ~6s, `ps -eo stat,cmd | rg '[c]lipbro'` shows no `Z`/`<defunct>`.

### Step 2: Wait on wl-copy's foreground and log a non-zero exit

At the end of `wl_copy()`, after the stdin block, await the child and log
failure:

```rust
match child.wait().await {
    Ok(status) if !status.success() => {
        tracing::error!("wl-copy exited with {status}");
    }
    Ok(_) => {}
    Err(e) => tracing::error!("wl-copy wait failed: {e}"),
}
```

Keep the existing spawn-error and stdin-write-error `tracing::error!`s. Do not
change call sites in `copy_to_clipboard` / `sync_to_selection`.

**Verify**: `cargo test` (the daemon/clipboard mock tests don't exercise the
real `WaylandClipboard`, so they stay green). Manually confirm a normal copy
still works and logs nothing new; temporarily rename `wl-copy` on `PATH` and
confirm the error is now logged.

## Done criteria

- [ ] `cargo check --all-targets` exits 0
- [ ] `cargo clippy --all-targets` adds no new warnings
- [ ] `cargo test` pass count unchanged
- [ ] `overlay_running()` (or equivalent reap) is called from the periodic tick
- [ ] `wl_copy()` awaits the child and logs a non-zero/failed exit
- [ ] Manual: no lingering `<defunct>` overlay after Esc + 6s; wl-copy failure is logged
- [ ] `plans/README.md` status row updated

## STOP conditions

- If `child.wait().await` in Step 2 ever blocks a normal copy (it must not —
  the foreground wl-copy exits after forking), stop and report; do not paper
  over it with a timeout without understanding why.
- If `overlay_running()`'s signature/side effects differ from the "Current
  state" description (drift), stop.
