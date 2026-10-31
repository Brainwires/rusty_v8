# Brainwires rusty_v8 (denext security line)

This fork builds rusty_v8 for the Brainwires Deno fork (denext's runtime). It exists to ship V8
security fixes that upstream rusty_v8 has not released yet. It is not published to crates.io.

## `v156.0.0-denext.1`: V8 15.6.75.32 (phase 2)

Branch `v8-15.6-denext`, cut from `denext/v152.2.0-v8.15.2.124.45` (upstream `v152.2.0` plus
phase 0). V8 is Chrome **Extended Stable 156**'s branch: Chromium Dash's Extended channel ships
156.0.8078.80 (2026-10-29) with V8 15.6.75.27; this line takes `branch-heads/15.6` at its head on
2026-10-30, **15.6.75.32**, which adds five more `[M156]` security merges that the next Chrome 156
update ships.

| | `v152.2.1-denext.1` | this line |
|---|---|---|
| V8 | 15.2.124.45 | **15.6.75.32** (`5d6b2c77e4`) |
| `v8` submodule | denoland/v8 `15.2-lkgr-denoland` | chromium.googlesource.com/v8/v8 at the release commit |
| `build` submodule | denoland/chromium_build | chromium.googlesource.com/chromium/src/build at V8's DEPS pin (`bc0661cf80`) |
| embedder patches | commits of those two repositories | `denext/patches/{v8,build}/*.patch`, applied by `tools/denext_patches.py` |
| crate version | `152.2.1+denext.1` | `156.0.0+denext.1` |

### Source

- **The patches.** denoland keeps rusty_v8's embedder patches as commits on denoland/v8
  (`<ver>-lkgr-denoland`, 4 patches) and denoland/chromium_build (8 patches). Neither has a 15.6
  branch, so the submodules now point at Chromium's own repositories and the patches live here as
  `git format-patch` files: `python3 tools/denext_patches.py` applies them (idempotent; `--check`
  verifies). The 4 V8 patches apply cleanly to 15.6. Of the 8 build patches, "Disable TARGET check
  in rust/run_bindgen.py" was ported by hand (upstream reformatted the block); the others apply
  cleanly. `release-denext.yml` runs the script after checkout.
- **The other submodules** moved to V8 15.6's `DEPS`: `buildtools`, `tools/clang`, `tools/win`,
  icu, abseil-cpp, libc++, libc++abi, libunwind, llvm-libc, fast_float, partition_alloc and
  `third_party/rust`. jinja2, markupsafe, fp16, simdutf, highway and dragonbox did not move.
- **metagen off.** V8 15.6 generates its instance types with `metagen`, which needs a separately
  downloaded libclang package (`third_party/llvm-libclang`, a GCS entry in `DEPS`), `gn` and a
  bare `python` on `PATH`. `build.rs` passes `v8_use_metagen_instance_types=false`, the
  configuration of V8's own "no metagen" builders: Torque emits the instance types instead (it runs
  either way), and the metagen action leaves the build graph. `GN_ARGS` can override it.

### API changes (V8 15.2 → 15.6)

C++ (`src/binding.cc`):

- `SetNativeDataProperty` takes only `AccessorNameSetterCallbackV2`
  (`PropertyCallbackInfo<Boolean>`), for both `ObjectTemplate` and `Object`.
- `Object::GetPrototypeV2` / `SetPrototypeV2` are `GetPrototype` / `SetPrototype` again.
- `PropertyCallbackInfo::HolderV2` is `Holder` again.
- `CppHeap::Terminate` is gone (a `CppHeap` tears down when destroyed);
  `v8__CppHeap__Terminate` is a no-op, so `cppgc::Heap::terminate` stays.
- `Isolate::GetContinuationPreservedEmbedderData` returns `Local<Data>`; the binding casts it back
  to `Value` (only values are ever stored, and the unset default is `undefined`).
- `MicrotaskQueue` is always garbage collected: the queue handle always holds a
  `cppgc::Persistent`.

Rust (public API):

- **Breaking:** `AccessorNameSetterCallback` (`ObjectTemplate::set_accessor_with_setter`,
  `Object::set_accessor_with_setter`, `AccessorConfiguration::setter`) callbacks take
  `ReturnValue<Boolean>` instead of `ReturnValue<()>`. `rv.set_bool(false)` reports a failed set
  (a `TypeError` in strict mode); leaving it unset reports success.

Behaviour:

- `SyntheticModule` evaluation steps must return a Promise (V8 `CHECK`s it since 15.3).

### Build fixes

- **ICU headers.** V8 15.6 no longer forwards ICU's public config to its dependents, so
  `binding.cc`'s `#include "unicode/locid.h"` found a *system* ICU (Ubuntu's ICU 74) and the
  tests failed to link (`icu_74::Locale::getDefault()` undefined). `BUILD.gn` adds
  `//third_party/icu:icu_config` to `rusty_v8_config`.
- **`binding.cc` compile errors**, found by the build rather than predicted: besides the API list
  above, the untagged `Context::{Get,Set}AlignedPointer…EmbedderData` are gone; the binding passes
  `v8::kEmbedderDataTypeTagDefault`, which is what the untagged forms used, and the Rust API stays
  untagged.

### bindgen

The phase-0 spike's open blocker. Two separate causes:

1. **`reference to unresolved using declaration` in libc++** is not the libclang version (it
   reproduces with libclang 21 and 23 alike). bindgen (clang-sys) also runs a `clang` executable
   to learn the default include paths and takes the first one on `PATH` unless `CLANG_PATH` names
   one, so a libclang 23 parse got another Clang's builtin headers. `build.rs` now sets
   `CLANG_PATH` to the `clang` beside `LIBCLANG_PATH` (`<llvm>/bin/clang[.exe]`) when it is unset.
2. **bindgen 0.72 panics `Not an item: ItemId(..)`** in `compute_bitfield_units`, on libc++'s new
   `std::aligned_union` (`__type_traits/aligned_union.h`: a recursive variadic `union __union`).
   Nothing bindgen reads uses it (it is deprecated, and only `<type_traits>` includes it), so
   `build.rs` predefines its include guard (`-D_LIBCPP___TYPE_TRAITS_ALIGNED_UNION_H`) for the
   bindgen parse only; the C++ build is unaffected.

With both, libclang **23.1.4** generates `src_binding.rs`. Its items are the same as 15.2's apart
from nested classes losing their enclosing-class prefix in the generated names (for example
`v8_String_WriteFlags` is now `WriteFlags`), which no Rust code refers to (it uses the `v8__*`
aliases of `binding.hpp`). `release-denext.yml` pins libclang 23 on every host: apt.llvm.org's
`llvm-toolchain-jammy-23` on Linux, Homebrew's `llvm` (checked to be 23) on macOS, and LLVM's own
`clang+llvm-23.1.4-x86_64-pc-windows-msvc.tar.xz` (SHA-256 pinned) on Windows, instead of
Chromium's libclang package.

### Verified (biscuits, Linux x86_64)

`V8_FROM_SOURCE=1 cargo build --release --lib` (2385 ninja steps, about 30 min at nice 10;
`librusty_v8.a` 204 MB) and `cargo test --release`: every suite passes (384 tests, 14 ignored),
including the updated `synthetic_module` and the new `object_set_accessor_setter_reports_failure`
(a V2 setter's `rv.set_bool(false)` throws a `TypeError` in strict mode only).

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

Each target runs rusty_v8's own test suite where it can execute. Each release (`v<crate>-denext.<n>`)
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

Patch-level bumps on `branch-heads/15.6` are a `v8` submodule move plus a release (the patches
apply unchanged within a branch). The next major is Extended Stable 160 (V8 16.0, about
mid-December 2026): move the submodules to its `DEPS`, re-export the patches (`git format-patch`
from a rebased checkout) and follow the binding changes.
