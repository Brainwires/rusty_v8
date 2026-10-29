# Brainwires rusty_v8 (denext security line)

This fork builds rusty_v8 for the Brainwires Deno fork (denext's runtime). It exists to ship V8
security fixes that upstream rusty_v8 has not released yet. It is not published to crates.io.

## `v152.2.1-denext.1`: V8 15.2.124.45 (phase 0)

Branch `denext/v152.2.0-v8.15.2.124.45`, cut from upstream tag `v152.2.0`.

| | upstream `v152.2.0` | this line |
|---|---|---|
| V8 | 15.2.124.1 | **15.2.124.45** |
| `v8` submodule | denoland/v8 `c4ca1eccb9` | denoland/v8 `7d033dd9d2` (`15.2-lkgr-denoland`, 2026-10-29) |
| crate version | `152.2.0` | `152.2.1+denext.1` |

What changed, and nothing else:

- **The `v8` submodule** moved to denoland/v8 `7d033dd9d2`. That is V8's `branch-heads/15.2` at
  15.2.124.45 (2026-10-14, the last 15.2 release) with denoland's 4 embedder patches already
  rebased on top. It carries the 44 merges Google made to 15.2 after 15.2.124.1, among them the
  fixes for two V8 bugs exploited in the wild: **CVE-2026-85046** (type confusion, fixed in
  15.2.124.21) and **CVE-2026-87491** (OOB write, crbug 543557673).
- **No other submodule moved.** V8's `DEPS` is identical between the two commits except for a
  `-vpython-spec` argument in one gclient hook, so `tools/update_deps.py` has nothing to update and
  the `build` (denoland/chromium_build) pin still contains Chromium's revision. The V8 public
  headers differ only in `include/v8-version.h`, so the Rust and C++ bindings are unchanged.
- **The crate version** is `152.2.1+denext.1`. It is a patch above upstream so it satisfies every
  `^152.2.0` requirement (the Deno fork's `libs/deno_v8` asks for `152.2.0`) when supplied through
  `[patch.crates-io]`. The `+denext.1` build metadata names the fork's build and is ignored by
  Cargo's version matching. A pre-release (`152.2.0-denext.1`) would not do: it sorts *below*
  `152.2.0` and does not satisfy `^152.2.0`.

## Release assets

`.github/workflows/release-denext.yml` (dispatch only, never scheduled) builds the release profile
with default features from source on GitHub-hosted runners for the six targets the Deno fork
ships:

- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`

Each target runs rusty_v8's own test suite where it can execute. The release `v152.2.1-denext.1`
carries, per target, `librusty_v8_release_<target>.a.gz` (`rusty_v8_release_<target>.lib.gz` on
Windows) and `src_binding_release_<target>.rs`, plus `SHA256SUMS` and a build-provenance
attestation (`gh attestation verify <file> -R Brainwires/rusty_v8`).

## How the Deno fork consumes it

- The crate comes from the `crate` branch of this repo by git rev, through `[patch.crates-io]`.
  That branch holds only the `cargo package` output (no submodules), so Cargo never clones V8.
- The prebuilt library and binding come from this repo's release, checked against a SHA-256
  manifest in the Deno fork, then handed to `build.rs` through `RUSTY_V8_ARCHIVE`,
  `RUSTY_V8_ARCHIVE_SHA256` and `RUSTY_V8_SRC_BINDING_PATH`. Nothing is fetched from denoland's
  releases.

## Next

Phase 2 of the plan moves this line to the current Chrome Extended Stable V8 (15.6), which needs
`binding.cc` and Rust API changes. Patch-level bumps on a branch are a submodule move plus a
release, as here.
