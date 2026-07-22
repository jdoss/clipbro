# Plan 008: Make hotkey and config mistakes loud instead of silent

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/overlay.rs src/config.rs README.md`
> Plans 003/005/006 touch overlay.rs elsewhere; the `ParsedHotkey` block
> (lines ~69-142 at d7a7c18) should be structurally intact. On mismatch in
> that block, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (invalid configs currently misbehave silently; after this they misbehave identically but say so)
- **Depends on**: none. Plan 010 moves this code afterward — land 008 first.
- **Category**: bug
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

A user who writes `pause = "ctr+p"` (typo), `toggle_favorite = "meta+f"`
(unsupported modifier), or `delete_entry = "f1"` (unsupported named key)
gets a hotkey that simply never fires — no error, no warning anywhere
visible. Mechanism: `ParsedHotkey::parse` ignores unknown modifiers
(`_ => {}`) and stores any final token as `key_char`; multi-character
`key_char`s that aren't in the named-key list can never match a keyboard
event. Similarly, one syntax error in `config.toml` silently discards the
**entire** config (all settings revert to defaults) with only a log-file
warning. After this plan, invalid hotkeys produce a clear warning naming the
bad binding and the supported values (and fall back to that binding's
default), and a broken config file warns on stderr where systemd's journal
captures it. The README documents which keys are supported.

## Current state

- `src/overlay.rs:77-101` — `ParsedHotkey::parse` (the silent `_ => {}` at
  line 96):

```rust
fn parse(s: &str) -> Self {
    let lower = s.to_lowercase();
    let parts: Vec<&str> =
        lower.split('+').map(|p| p.trim()).collect();
    let mut hotkey = Self {
        ctrl: false,
        alt: false,
        shift: false,
        key_char: String::new(),
    };
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            hotkey.key_char = part.to_string();
        } else {
            match *part {
                "ctrl" => hotkey.ctrl = true,
                "alt" => hotkey.alt = true,
                "shift" => hotkey.shift = true,
                _ => {}
            }
        }
    }
    hotkey
}
```

- `src/overlay.rs:125-141` — `matches_named`: the complete supported
  named-key list is `delete, insert, home, end, pageup, pagedown, tab`;
  anything else multi-char returns false forever.
- `src/overlay.rs:213-225` — the three parse call sites in `new()`
  (`FAVORITE_HOTKEY.set(ParsedHotkey::parse(&config.hotkeys.toggle_favorite))` etc.).
- `src/config.rs:56-74` — `Config::load()`; the invalid-TOML branch:

```rust
Err(e) => {
    tracing::warn!(
        "Invalid config at {}: {e}, \
         using defaults",
        path.display()
    );
    Self::default()
}
```

- `src/config.rs:45-53` — `Hotkeys::default()`:
  `toggle_favorite: "ctrl+f"`, `delete_entry: "delete"`, `pause: "ctrl+p"`.
- Existing hotkey tests: `src/overlay.rs:1518-1584` (7 tests, happy paths
  only). README hotkey docs: `README.md:186-188`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Typecheck | `cargo check --all-targets` | exit 0 |
| Tests | `cargo test overlay::` | all pass (7 updated + ~5 new) |
| Full | `cargo test` | all pass |

## Scope

**In scope**:
- `src/overlay.rs` (`ParsedHotkey::parse` → fallible; call sites; tests)
- `src/config.rs` (stderr echo on invalid TOML)
- `README.md` (supported-keys paragraph)

**Out of scope**:
- Adding NEW named keys (F1-F12, space, …) — that's a feature, not
  validation; note it as a follow-up only.
- Validating other config fields (position strings, byte sizes).
- Moving `ParsedHotkey` to its own module (plan 010).

## Git workflow

- Branch: `improve/008-validate-hotkeys`
- Two commits: code+tests, then README.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Make `parse` fallible

Change the signature to `fn parse(s: &str) -> Result<Self, String>` and
enforce:

- empty input or empty final token (e.g. `"ctrl+"`) →
  `Err("empty key in hotkey '<s>'")`
- a non-final token not in `ctrl|alt|shift` →
  `Err("unknown modifier '<part>' in hotkey '<s>' (supported: ctrl, alt, shift)")`
- a final token longer than one char that is NOT one of
  `delete|insert|home|end|pageup|pagedown|tab` →
  `Err("unsupported key '<part>' in hotkey '<s>' (supported named keys: delete, insert, home, end, pageup, pagedown, tab, or any single character)")`

Keep the named-key list in ONE place: add
`const NAMED_KEYS: &[&str] = &["delete", "insert", "home", "end", "pageup", "pagedown", "tab"];`
and use it both in `parse`'s validation and (via the existing match) keep
`matches_named` as-is — but add a comment on `matches_named` pointing at
`NAMED_KEYS` so they don't drift.

**Verify**: `cargo check --all-targets` → errors only at the 3 call sites (expected, fixed next)

### Step 2: Handle errors at the call sites

In `new()` (`src/overlay.rs:213-225`), replace each
`OnceLock.set(ParsedHotkey::parse(&config.hotkeys.X))` with the pattern:

```rust
let favorite_hotkey = ParsedHotkey::parse(
    &config.hotkeys.toggle_favorite,
)
.unwrap_or_else(|e| {
    tracing::warn!(
        "Invalid toggle_favorite hotkey: {e}; \
         using default"
    );
    ParsedHotkey::parse(
        &crate::config::Hotkeys::default()
            .toggle_favorite,
    )
    .expect("default hotkey is valid")
});
let _ = FAVORITE_HOTKEY.set(favorite_hotkey);
```

Same for `DELETE_HOTKEY`/`delete_entry` and `PAUSE_HOTKEY`/`pause`. The
warning goes through tracing, which the overlay process writes to the log
file and stderr (captured by the journal when daemon-spawned).

**Verify**: `cargo check --all-targets` → exit 0

### Step 3: Echo config-load failures to stderr

In `Config::load()`'s invalid-TOML branch (`src/config.rs:62-69`), add after
the `tracing::warn!`:

```rust
eprintln!(
    "clipbro: invalid config at {}: {e} \
     — using defaults",
    path.display()
);
```

Rationale: `Config::load()` runs in the daemon, overlay, and `clipbro init`;
in the CLI cases tracing may not be initialized yet, and stderr reaches both
the terminal and the systemd journal.

**Verify**: `cargo test config::` → all pass

### Step 4: Update the tests

The 7 existing tests (`src/overlay.rs:1522-1584`) now need `.unwrap()` on
`parse(...)`. Then add new tests in the same module:

1. `parse_rejects_unknown_modifier` — `parse("meta+f")` → Err containing `"unknown modifier 'meta'"`
2. `parse_rejects_typo_modifier` — `parse("ctr+p")` → Err (the README's own typo example)
3. `parse_rejects_unsupported_named_key` — `parse("f1")` → Err containing `"unsupported key 'f1'"`
4. `parse_rejects_empty_key` — `parse("ctrl+")` → Err containing `"empty key"`
5. `parse_accepts_all_named_keys` — loop over the `NAMED_KEYS` const; each parses Ok with the right `key_char`

**Verify**: `cargo test overlay::` → all pass (12 hotkey tests)

### Step 5: Document supported keys in the README

In the Hotkeys section (`README.md:186-188`), extend the paragraph:

> Values are modifier+key strings like `"ctrl+f"`, `"alt+d"`, or `"delete"`.
> Supported modifiers: `ctrl`, `alt`, `shift`. The key is any single
> character, or one of: `delete`, `insert`, `home`, `end`, `pageup`,
> `pagedown`, `tab`. Invalid bindings are reported in the log and fall back
> to that hotkey's default.

**Verify**: `rg -n "pagedown" README.md` → 1 match

## Test plan

Step 4 enumerates the cases: each rejection rule has a test that triggers
it, plus the exhaustive named-key acceptance loop. Pattern: the existing
hotkey tests in the same module. The fallback-to-default path in Step 2 is
exercised implicitly by `expect("default hotkey is valid")` +
`parse_accepts_all_named_keys` covering the default values' shapes
(`ctrl+f`, `delete`, `ctrl+p`).

## Done criteria

- [ ] `cargo test` exits 0; 5 new rejection/acceptance tests pass
- [ ] `rg -n "_ => \{\}" src/overlay.rs` → no match inside `ParsedHotkey::parse` (the silent-ignore arm is gone)
- [ ] `rg -n "NAMED_KEYS" src/overlay.rs` → const defined and used in `parse`
- [ ] README documents the named-key list
- [ ] `git status` clean outside the in-scope list
- [ ] `plans/README.md` status row updated

## STOP conditions

- `ParsedHotkey` has moved out of `src/overlay.rs` (plan 010 ran first) —
  apply the same changes in its new location IF the code is otherwise
  identical; on any structural difference, STOP.
- Making `parse` fallible breaks a caller this plan didn't list — there are
  exactly 3 production call sites at d7a7c18; a 4th means drift.

## Maintenance notes

- Follow-up explicitly deferred: supporting more named keys (F1–F12,
  `space`, `esc`). When added, extend `NAMED_KEYS` + `matches_named` + the
  README list together — the comment added in Step 1 marks the pairing.
- Plan 010 moves this code into `src/hotkey.rs`; the tests move with it.
- Reviewer: confirm the fallback uses the per-binding default (not a
  hardcoded string), so changing `Hotkeys::default()` stays one-place.
