# Plan 014: SPIKE — images served by clipbro never reach the X11/XWayland clipboard

> **Executor instructions**: This is a SPIKE (investigate + decide), not an
> implementation plan. Do the investigation, record findings inline, and
> produce a decision + a follow-up implementation plan (or a "won't fix +
> upstream report" note). Do **not** add an X11 code path before the decision
> is made. Update `plans/README.md` when done.

## Status

- **Priority**: P2
- **Effort**: M (investigation)
- **Risk**: n/a (no production change in this plan)
- **Depends on**: none
- **Category**: bug / compatibility
- **Planned at**: commit `4cddd6e`, 2026-07-21 (observed while live-debugging)

## Why this matters

clipbro serves selections only on **Wayland** (it shells out to `wl-copy`) and
trusts the compositor to bridge to X11 for XWayland apps. During a live repro
on COSMIC:

- The Wayland CLIPBOARD held `image/png` continuously (verified with
  `wl-paste`), served by clipbro's `wl-copy`.
- The **X11 CLIPBOARD advertised text targets only** the entire time
  (`xclip -selection clipboard -t TARGETS -o` → `UTF8_STRING COMPOUND_TEXT
  STRING text/plain TEXT TARGETS MULTIPLE TIMESTAMP …`, no `image/*`), and
  `xclip -t image/png -o` returned empty.

Net effect: **any XWayland (X11) app cannot paste an image that clipbro put on
the clipboard.** Text works; images don't cross the bridge.

**Honesty about scope:** the original report (paste into Slack) is *not* fixed
by this — Slack runs `--ozone-platform=wayland` and reads the Wayland
clipboard. This plan is about the broad population of X11-only apps (many
still exist), not that report. Set priority accordingly.

## Open question (root cause unconfirmed)

Three candidate causes; the spike must distinguish them:

1. **cosmic-comp does not bridge image selections Wayland→X11 at all** (a
   compositor limitation/bug). → Not clipbro's bug; report upstream + document.
2. **cosmic-comp bridges text but chokes on an image-*only* offer** (clipbro's
   `wl-copy --type image/png` advertises a single non-text MIME; the source
   app's original offer also carried `text/html`, `text/x-moz-url`, and
   `chromium/*` types). → clipbro could offer a richer/compatible set.
3. **Something about how `wl-copy` publishes the offer** (INCR/large-data
   handling across the bridge). → clipbro could change how it serves.

## Investigation steps (record results inline in this file)

### Step 1: Does *any* Wayland→X11 image bridge work on this compositor?

Copy an image from a **native Wayland** app that owns the selection directly
(e.g. a screenshot tool's "copy to clipboard", or
`wl-copy --type image/png < some.png` run by hand), then:

```sh
xclip -selection clipboard -t TARGETS -o     # look for image/png
xclip -selection clipboard -t image/png -o | wc -c
```

- Image target present + bytes flow → bridging works in principle; clipbro's
  serve format is the problem (cause #2/#3). Proceed to Step 2.
- Still text-only/empty → **cosmic-comp limitation (cause #1)**; skip to the
  decision, option (B).

### Step 2: Does a richer offer bridge?

If Step 1 implicated the offer, try serving the image with an added text
fallback and/or the original rich types, and re-check `xclip`. Identify the
minimal offer that makes `image/png` appear on X11.

### Step 3: Environment facts

Record: cosmic-comp / XWayland versions (`cosmic-comp --version` if available,
`Xwayland -version`), and search their issue trackers for
"clipboard image XWayland" before deciding it's clipbro's job.

## Spike results (2026-07-21, cosmic-comp 1.3.0 `dec1ee8`, Xwayland 24.1.13)

Ran on the live session:

- **Control (text via `wl-copy`)**: set fresh text on the Wayland clipboard;
  `xclip -sel clipboard -o` returned **stale, unrelated text** (a prior
  Path-of-Exile copy), not the new text. Even *text* set via `wl-copy` did not
  reach X11.
- **Test (image via `wl-copy`)**: Wayland `--list-types` showed `image/png`
  (clipbro's serve works), but the X11 `TARGETS` were unchanged text-only atoms
  and `xclip -t image/png -o` returned **0 bytes**, twice, after a 3s settle.
- The X11 `TARGETS` set was **byte-identical on every probe all session**
  (`UTF8_STRING COMPOUND_TEXT STRING text/plain TEXT TARGETS MULTIPLE TIMESTAMP
  #16 #1 #7`): the X11 clipboard is pinned to a stale owner and never updates
  from Wayland.

**Root cause: not clipbro.** This is candidate #1 — cosmic-comp/XWayland is not
bridging externally-set Wayland selections to X11 (text *or* image) in this
session. clipbro correctly owns the Wayland selection; the Wayland→X11 mirror
is the compositor's responsibility and isn't happening. Open nuance: general
cosmic-comp bug vs. a session-specific stuck X11 owner — a clean repro after a
compositor restart distinguishes them; either way it is outside clipbro.

## Decision: (B) won't-fix in clipbro — upstream + document

- Confirm with a clean repro whether it's general or a stuck-owner artifact,
  then file/point to a cosmic-comp issue for Wayland→X11 clipboard bridging.
- Add a README limitation note: under COSMIC, X11-only apps may not receive
  clipboard content clipbro serves; Wayland apps are unaffected.
- Option (A) — clipbro owning the X11 selection itself (in-process x11rb/xcb
  owner with TARGETS/TIMESTAMP/INCR) — is **deferred**, not adopted: a
  heavyweight workaround (new runtime X11 dependency, ownership-ping-pong risk
  with the compositor) for a compositor-side bug that would not help the app in
  the original report (Slack is Wayland). Revisit only if the upstream bug
  persists and X11-app support becomes a priority.

## Original decision options (for reference)

Pick one:

- **(A) clipbro owns the X11 selection too.** When serving an image, also
  publish it on the X11 CLIPBOARD. Design constraints to spell out in the
  follow-up plan:
  - Mechanism: a persistent X11 selection owner. `xclip`/`xsel` shell-outs
    have the *same* fire-and-forget lifetime issues as `wl-copy` and a fresh
    X11 dependency at runtime; a proper in-process owner (`x11rb`/xcb) handles
    `TARGETS`/`TIMESTAMP`/INCR for large images but is real work and a new
    dependency (justify it per repo policy).
  - Must not fight the compositor's own bridge (avoid ownership ping-pong).
  - Gate behind a config flag only if it can't be made always-safe.
- **(B) Won't fix in clipbro.** File an upstream cosmic-comp/XWayland issue,
  and document the limitation in the README ("X11-only apps may not receive
  image pastes under COSMIC").

## Done criteria

- [ ] Steps 1-3 executed and results recorded in this file
- [ ] Root cause identified among the three candidates
- [ ] Decision (A) or (B) recorded with rationale
- [ ] If (A): a follow-up implementation plan (015+) written with the chosen
      mechanism, dependency justification, and test approach
- [ ] If (B): upstream issue link + README note drafted
- [ ] `plans/README.md` status row updated

## STOP conditions

- Do not add `xclip`/`x11rb`/xcb code or a new dependency in this plan.
- If Step 1 shows bridging works for native Wayland apps but clipbro still
  can't make it work with any offer, stop and report — that points at a
  subtler `wl-copy`/compositor interaction worth a focused look before coding.
