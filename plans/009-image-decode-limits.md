# Plan 009: Bound image decoding so a crafted image can't exhaust daemon memory

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat d7a7c18..HEAD -- src/daemon.rs`
> Plans 001–003 touched daemon.rs (tests, trim call, item moves). The
> `resize_to_thumbnail` function should match the excerpt. On mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (adds limits far above any legitimate clipboard image)
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `d7a7c18`, 2026-06-11

## Why this matters

The daemon decodes untrusted image bytes in two flows: every `image/*`
clipboard copy (any application can offer crafted bytes on the clipboard),
and — when the opt-in `show_remote_thumbnails` is enabled — images fetched
from copied URLs. The HTTP fetch correctly caps the **download** at
`max_thumbnail_bytes` (verified: `ureq` reader `.limit(max_bytes)`,
`src/daemon.rs:750-756`), but nothing caps what those bytes **decode to**: a
few-KB PNG can declare enormous dimensions and cost hundreds of MB of
allocation work in `image::load_from_memory`. The `image` crate supports
explicit decode limits; this plan sets them. (Exact crate-default behavior
varies by version — the fix is cheap insurance either way.)

## Current state

- `src/daemon.rs:701-711`:

```rust
fn resize_to_thumbnail(
    data: &[u8],
) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let thumb = img.thumbnail(256, 256);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}
```

- Callers: `generate_thumbnail` (clipboard images, `src/daemon.rs:463-501`)
  and `maybe_fetch_thumbnail` (remote, lines 503-553) — both already run it
  inside `spawn_blocking`, and both treat `None` as "skip thumbnail", which
  is the correct failure mode for an over-limit image (the entry itself is
  still stored).
- Existing tests: `daemon::tests::resize_to_thumbnail_valid_png`
  (512×512 → thumbnail) and `resize_to_thumbnail_invalid_data`
  (`src/daemon.rs:833-862`).
- `image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }`
  (`Cargo.toml:38`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| API reference | `cargo doc -p image --no-deps` then open `target/doc/image/struct.Limits.html` | confirms field names |
| Typecheck | `cargo check --all-targets` | exit 0 |
| Targeted tests | `cargo test resize_to_thumbnail` | all pass (2 existing + 1 new) |

## Scope

**In scope**: `src/daemon.rs` (`resize_to_thumbnail` + one test)

**Out of scope**:
- `max_thumbnail_bytes` download cap — already correct.
- URL scheme/IP filtering for remote fetches — considered and rejected for
  now: `ureq` only speaks http/https (de facto scheme allowlist), the
  feature is off by default, and private-range filtering was judged
  over-engineering for an opt-in desktop feature. Do not add it here.
- The overlay's `iced_image::Handle` rendering path (decodes stored
  *thumbnails* we generated ourselves, ≤256×256).
- Making limits configurable — fixed constants only.

## Git workflow

- Branch: `improve/009-image-decode-limits`
- Single commit, e.g. `Bound image decode dimensions and allocation`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Confirm the image 0.25 Limits API

Run `cargo doc -p image --no-deps` and confirm in the generated docs (or the
crate source under `~/.cargo/registry/src/*/image-0.25*/src/`):
`image::Limits` with fields `max_image_width: Option<u32>`,
`max_image_height: Option<u32>`, `max_alloc: Option<u64>`, and
`image::ImageReader::limits(&mut self, limits)` (or builder equivalent).
If names differ, adapt mechanically; if the Limits API doesn't exist in the
locked version, STOP.

**Verify**: doc page shows the three fields

### Step 2: Replace `load_from_memory` with a limited reader

```rust
fn resize_to_thumbnail(
    data: &[u8],
) -> Option<Vec<u8>> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIM);
    limits.max_image_height = Some(MAX_DECODE_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);

    let mut reader = image::ImageReader::new(
        std::io::Cursor::new(data),
    )
    .with_guessed_format()
    .ok()?;
    reader.limits(limits);
    let img = reader.decode().ok()?;

    let thumb = img.thumbnail(256, 256);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}
```

With module-level constants near the other `const`s at the top of
`src/daemon.rs` (lines 14-33):

```rust
/// Decode guards for untrusted clipboard/remote
/// images: 16384px per side (~268 MP) and 512 MiB
/// of decoder allocations — far beyond any real
/// screenshot, small enough to keep a crafted
/// image from exhausting daemon memory.
const MAX_DECODE_DIM: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
```

**Verify**: `cargo test resize_to_thumbnail` → both existing tests pass

### Step 3: Add the bomb regression test

In `src/daemon.rs` tests (next to `resize_to_thumbnail_invalid_data`), add a
hand-crafted PNG header declaring 100000×100000 — a valid signature + IHDR
so the decoder reads the dimensions and must reject on limits rather than on
parse failure:

```rust
#[test]
fn resize_to_thumbnail_rejects_huge_dimensions() {
    // PNG signature + IHDR declaring 100000x100000.
    let mut data: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47,
        0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D,
        b'I', b'H', b'D', b'R',
    ];
    data.extend_from_slice(
        &100_000u32.to_be_bytes(),
    );
    data.extend_from_slice(
        &100_000u32.to_be_bytes(),
    );
    // bit depth 8, color type 6 (RGBA),
    // compression 0, filter 0, interlace 0
    data.extend_from_slice(&[8, 6, 0, 0, 0]);
    // CRC: wrong is fine — limits must reject
    // before/regardless of CRC validation, but use
    // the real CRC if the decoder checks it first:
    // 0x7A 0x6A 0xE1 0xC4 is NOT asserted; see note.
    data.extend_from_slice(&[0, 0, 0, 0]);

    assert!(
        resize_to_thumbnail(&data).is_none()
    );
}
```

Note for the executor: the assertion is only that the function returns
`None` (it must not allocate gigabytes or panic). If the decoder rejects on
the bad CRC before checking limits, the test still passes — to make the test
specifically exercise the limit path, compute the correct IHDR CRC32 (over
`IHDR` + the 13 data bytes) with a tiny local crc32 helper in the test, or
verify the limit path manually by temporarily printing the decode error.
Prefer the correct-CRC version if it takes <15 minutes; otherwise ship the
simple version with a comment.

**Verify**: `cargo test resize_to_thumbnail` → 3 tests pass, completing in
well under a second (if this test takes seconds or OOMs, the limits are not
being applied — STOP)

## Test plan

Step 3 is the regression test (huge declared dimensions → `None`, fast).
Existing `resize_to_thumbnail_valid_png` (512×512 well under limits) and
`resize_to_thumbnail_invalid_data` (garbage) keep covering the happy and
malformed paths.

## Done criteria

- [ ] `cargo test` exits 0; the new bomb test passes in <1s
- [ ] `rg -n "load_from_memory" src/daemon.rs` → no matches
- [ ] `rg -n "MAX_DECODE_DIM" src/daemon.rs` → const + 1 use
- [ ] `git status` clean outside `src/daemon.rs`
- [ ] `plans/README.md` status row updated

## STOP conditions

- The locked `image` version lacks the `Limits`/`ImageReader::limits` API
  (Step 1).
- The bomb test OOMs or takes seconds — limits aren't reaching the decoder;
  do not "fix" by shrinking the test image.
- `resize_to_thumbnail_valid_png` starts failing — the limits are too tight
  or the reader path changed behavior for normal images.

## Maintenance notes

- If formats are ever added to the `image` features (gif/webp are sniffed in
  `src/main.rs::detect_mime` but NOT decodable today — only png/jpeg
  features are on), the same limits automatically apply; no per-format work.
- Reviewer: confirm `None` still means "store the entry without a thumbnail"
  in both callers, not "drop the entry".
