# Brainwires rusty_v8 (denext security line)

This fork builds rusty_v8 for the Brainwires Deno fork (denext's runtime). It exists to ship V8
security fixes that upstream rusty_v8 has not released yet. It is not published to crates.io.

## `v156.0.0-denext.4`: V8 15.6.75.38

A patch-level move on `branch-heads/15.6`, which was at its head on 2026-11-03: the `v8` submodule
moves from 15.6.75.33 (`292c5548f4`) to 15.6.75.38 (`96c8e217cf`). The five merges between them:

- `32bb3bf0bb` "[M156] [modules] Do not cache star exports after linking" (crbug 576122160,
  574038247).
- `0ae4806f48` "[M156] [fastapi] Don't catch uncatchable exceptions after deopt" (crbug 578797801,
  577627207).
- `0113334b37` "[debug][wasm] Defer debugger break in optimized Wasm frames" (crbug 577352626,
  579732376).
- `a78a00bf57` "[M156] [scopes] Fix private name resolution in catch scopes and heritage eval"
  (crbug 576947634).
- `0ac55dd376` "[M156] [scopes] Restore synthetic eval heritage skip bit from inner scope" (crbug
  576947634, 572988453, 573962535).

V8's `DEPS` and public headers did not change between them (only `include/v8-version.h`), and none
of the merges touches a file the `denext/patches/v8` patches change, so no other submodule, patch or
binding moved. The read-only heap guard of `denext.3` is unchanged. The crate is
`156.0.0+denext.4`.

## `v156.0.0-denext.3`: V8 15.6.75.33, read-only heap guard

V8 is unchanged (15.6.75.33, `292c5548f4`); the crate is `156.0.0+denext.3`. The binding's guard
on V8's shared read-only heap (`snapshot::read_only_heap_guard`) now covers the two cases the
`denext.2` guard missed, both described under [Snapshot creators and concurrent
isolates](#snapshot-creators-and-concurrent-isolates): a snapshot creator that joins an isolate
group V8 has emptied and set up anew, and isolates of snapshots with different read-only images
alive at the same time (a deterministic abort in `Deserializer::ReadReadOnlyHeapRef`). Creating and
disposing an isolate now takes the guard's mutex (about 46 ns per create and dispose,
uncontended). A process whose isolates all come from one snapshot never waits. The release notes
now carry a summary of the release's changes (`denext/release-notes/<tag>.md`).

## `v156.0.0-denext.2`: V8 15.6.75.33

A patch-level move on `branch-heads/15.6`: the `v8` submodule moves from 15.6.75.32 to 15.6.75.33
(`292c5548f4`), which adds the security merge `e1c709282e` "[M156] [builtins] Track Promise.any
reject closure state in the errors array". V8's `DEPS` and public API did not change between them,
so no other submodule, patch or binding moved. The crate is `156.0.0+denext.2`. The release notes
now state the V8 version, the Windows SDK bootstrapper and the conda-forge LLVM packages.

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
- **JavaScript in interrupt callbacks.** V8 15.6 builds with
  `v8_disallow_js_in_api_interrupts_is_checked` on (except for Node), which runs every
  `Isolate::RequestInterrupt` callback under `DisallowJavascriptExecution` and aborts the
  process if one runs JavaScript ("Invoke in DisallowJavascriptExecutionScope"). Embedders
  dispatch inspector messages from interrupts (the Deno fork's inspector does), and the fork's
  `--trace-sigint` captured its stack there (`specs::node::trace_sigint::*_busy` aborted), so
  `build.rs` passes `v8_disallow_js_in_api_interrupts_is_checked=false`, V8's configuration for
  Node (test `request_interrupt_callback_runs_javascript`). `GN_ARGS` can override it.
- **Live edit is gone** from V8: `Debugger.setScriptSource` always fails ("setScriptSource
  functionality no longer available") and `--inspector-live-edit` is no longer a flag. Not a
  binding change, but embedders that hot-replace modules must restart or reload instead.

### Build fixes

- **ICU headers.** V8 15.6 no longer forwards ICU's public config to its dependents, so
  `binding.cc`'s `#include "unicode/locid.h"` found a *system* ICU (Ubuntu's ICU 74) and the
  tests failed to link (`icu_74::Locale::getDefault()` undefined). `BUILD.gn` adds
  `//third_party/icu:icu_config` to `rusty_v8_config`.
- **`binding.cc` compile errors**, found by the build rather than predicted: besides the API list
  above, the untagged `Context::{Get,Set}AlignedPointer…EmbedderData` are gone; the binding passes
  `v8::kEmbedderDataTypeTagDefault`, which is what the untagged forms used, and the Rust API stays
  untagged.
- **Out-of-range embedder data reads.** V8 15.2's untagged `GetAlignedPointerFromEmbedderData`
  was an inline read with no bounds check; the checked getter 15.6 leaves aborts the process
  (`Fatal error in v8::Context::GetAlignedPointerFromEmbedderData(): Index too large`) for a slot
  past the context's embedder data. The Deno fork's `node:vm` code reads such a slot on its main
  context to tell vm contexts apart (its `specs::node::vm_wasm_streaming` aborted).
  `Context::get_aligned_pointer_from_embedder_data` now returns null for a slot past the
  context's embedder data (test `context_aligned_pointer_out_of_range_is_null`), which is how an
  unset slot already reads.

### Snapshot creators and concurrent isolates

`tests/slots.rs` aborted intermittently under a plain `cargo test` (8 of 60 runs on 15.2, 2 of 60
on 15.6, Linux x86_64) with `vector.h:415: libc++ Hardening assertion __n < size() failed:
vector[] index out of bounds` in `Deserializer::ReadReadOnlyHeapRef`, or V8's `Check failed:
IsFreeSpaceOrFiller(filler)`, or a SIGSEGV. Its tests run on parallel threads of one process, and
one of them (`clear_all_context_slots`) makes a snapshot. Skipping that test: 0 of 60.

The cause is in V8's design, not in concurrent deserialization: every isolate of a process shares
one read-only heap (the isolate group's), set up by the first isolate created. When that is a
`SnapshotCreator`, V8 leaves the heap writable (`--extensible-ro-snapshot`, on by default) until
`CreateBlob` (or the creator's destructor) promotes objects into it and seals it, which detaches
its pages into the shared space and **deletes** the space the creator started with
(`ReadOnlyHeap::InitFromIsolate` → `Heap::ReplaceReadOnlySpace`). An isolate created in that
window attaches to the creator's writable space: on another thread it reads the page vector while
the creator appends to it, or allocates into it (the creator's own `ReadOnlySerializer` then
crashed walking the pages), and on any thread it keeps `Heap::read_only_space_` pointing at the
space V8 deletes (a use-after-free). V8's comment on that path expects it only in `mksnapshot`.

The binding now enforces the window (`snapshot::read_only_heap_guard`): while a snapshot creator
is the isolate group's only isolate and has not created its blob, `Isolate::new` and another
`snapshot_creator` wait for it on other threads and panic on the creator's own thread (where
waiting would never end). Snapshots are unchanged (the creator still extends read-only space).
Test `test_snapshot_creator_concurrent_isolates` (its own binary, so the creator is the first
isolate) failed 9 of 20 runs without the guard and 0 of 60 with it; `tests/slots.rs` 0 of 100.

**The window reopens** (fixed in `denext.3`). The guard took the heap to be sealed for
good once a regular isolate had existed. V8 frees it when the group's last isolate is disposed
(`IsolateGroup::RemoveIsolate`), and the next isolate sets up a new one, so a snapshot creator
created after the process ran out of isolates had a writable heap and no guard, and regular isolates
on other threads attached to it: the guard only helped when a creator was the very first isolate.
Whether this explains the aborts of `deno_core`'s lib tests on parallel threads
(`vector.h:419: libc++ Hardening assertion __n < size() failed`, 3 of 33 runs) is not established.

The guard now counts the group's isolates. Whether a creator joins an empty group is only known
while no isolate is being created or disposed, so a creator is constructed alone, while regular
isolates and disposals may still overlap each other; creating or disposing an isolate takes a mutex
where it used to take one atomic load. Beside a live regular isolate the heap is sealed, and a
creator holds nothing up. Test `test_snapshot_creator_after_last_isolate` empties the group before
each creator.

**Isolates of another snapshot** (fixed in `denext.3`). The shared read-only heap is set up from the
snapshot of the isolate that joins the empty group, and V8 attaches every later isolate to it
whatever its own snapshot holds. A snapshot whose creator extended the heap has a read-only image
of its own, and its startup data refers to read-only objects by page and offset in that image. An
isolate made from it while the group holds V8's built-in image reads past the heap's pages:
`vector.h:419: libc++ Hardening assertion __n < size() failed` in
`Deserializer::ReadReadOnlyHeapRef`, deterministically and on one thread (default isolate alive,
then `Isolate::new` from such a blob: 5 of 5 aborts on 15.6.75.33, the same on 15.2.124.45 at
`vector.h:415`). The reverse order resolves references to the wrong objects without a crash. V8
checks this only in debug builds (`ReadOnlyArtifacts::VerifyChecksum`: "make sure it's always the
same snapshot"). This, and the reopened window above, are what `deno_core`'s lib tests hit on
parallel threads: its snapshot tests load the snapshots they make while other tests hold built-in
isolates.

The guard now also tracks the group's read-only image, by the read-only checksum in the snapshot
blob's header (`kReadOnlySnapshotChecksumOffset`, byte 12; V8's built-in snapshot is its own
image, never equal to a blob's). An isolate of another image waits until the group is empty and
then sets the heap up from its own; isolates of the group's image are held back for up to 5 s
meanwhile so the group drains, unless their thread already owns an isolate. Where the wait could
never end (the thread owns an isolate of the other image) or outlasts 120 s, creation panics with
the cause. A process whose isolates all come from one snapshot, as Deno's do (`CLI_SNAPSHOT`),
never waits. A creator that joins a sealed heap cannot extend it, so the guard records the blob it
makes as that heap's image (rusty_v8's `test_api` makes such snapshots beside live isolates).
Test `test_snapshot_read_only_image`: 5 of 5 aborts on the released guard, 20 of 20 passes here;
the whole suite passes (Linux x86_64, prebuilt 15.6.75.33 archive; `test_api` 5 of 5).

**Review before release.** Three changes. (1) The guard removes an isolate from its owners before
V8 disposes it: it used to remove the entry by address afterwards, so when V8 created another
isolate at the same address meanwhile, the entry of the new isolate could go instead, and the
disposing thread then looked as if it still owned an isolate (and would panic on its next isolate
of another image). (2) Once the group has drained, an isolate that waited to set up another image
goes first: isolates of the drained image are held back from an empty group too, within their 5 s,
where any thread that took the mutex first could set the old image up again and send the waiting
one into another drain, up to its 120 s panic. (3) The bookkeeping is one `Guard` value, so the
unit tests in `src/snapshot.rs` drive it with stand-in isolates and short timeouts, and owners are
kept in a map instead of a list. The unit tests cover both fixes (each fails without its fix), a
single image never waiting under 8 threads, the hold-back running out, and both panics.

Waits that can still last: an isolate waits without a limit while another thread's snapshot creator
holds the heap open, and a creator waits without a limit for creations and disposals in flight (one
that V8 runs embedder code in, such as a backing store's deleter, must not wait for a thread that is
creating a creator). A thread that waits for the group to drain while it holds a lock that the other
image's disposal needs waits for its 120 s, then panics. An isolate used on another thread than the
one that created it (through a `Locker`) is counted for its creating thread. None of these happens in
Deno, whose runtime creates every isolate from `CLI_SNAPSHOT` on the thread that runs it.

Cost, `Isolate::new` + drop on Apple M-series (aarch64-apple-darwin, release, one isolate kept
alive): about 150 us per isolate on one thread and 49 us per isolate on 8 threads, with and without
the guard (the runs differ by less than their noise, 2 %); the guard alone, measured with stand-in
isolates, costs 46 ns per create and dispose on one thread and 193 ns on 8 contending threads.

Deno is not exposed: `JsRuntimeForSnapshot` and `create_runtime_snapshot` are reached only from
`build.rs` processes and tests, not from the runtime, `deno compile` or desktop packaging (which
embed the prebuilt `CLI_SNAPSHOT`), and its workers are created from a sealed heap (9,600 concurrent
`new Worker` on runtime 3.0.0-denext.6: no failure).

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
`llvm-toolchain-jammy-23` on Linux, Homebrew's `llvm` (checked to be 23) on arm64 macOS, conda-forge's
osx-64 packages of 23.1.4 (each SHA-256 pinned) on Intel macOS, and LLVM's own
`clang+llvm-23.1.4-x86_64-pc-windows-msvc.tar.xz` (SHA-256 pinned) on Windows, instead of
Chromium's libclang package. Intel macOS takes conda-forge because LLVM's 23.1.x releases carry no
x86_64 macOS tarball and Homebrew's `llvm` has no Intel bottle past 22.

### Windows SDK

V8 15.6's `build` pins `SDK_VERSION = 10.0.28000.0` in `toolchain/win/setup_toolchain.py`, passed to
`vcvarsall`, and `gn gen` fails if that SDK's directories are missing. GitHub's `windows-2022` image
stops at 26100 ([runner-images#14198](https://github.com/actions/runner-images/issues/14198)), so
`release-denext.yml` installs 10.0.28000 first, with the installer pinned by the SHA-256 in
`microsoft/winget-pkgs`, and its Authenticode signature checked. Remove the step once the image
ships this SDK.

### Verified (biscuits, Linux x86_64)

`V8_FROM_SOURCE=1 cargo build --release --lib` (2385 ninja steps, about 30 min at nice 10;
`librusty_v8.a` 204 MB) and `cargo test --release`: every suite passes (386 tests, 14 ignored),
including the updated `synthetic_module`, the new `object_set_accessor_setter_reports_failure`
(a V2 setter's `rv.set_bool(false)` throws a `TypeError` in strict mode only),
`context_aligned_pointer_out_of_range_is_null` and `request_interrupt_callback_runs_javascript`.

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
apply unchanged within a branch). `.github/workflows/patch-bump-denext.yml` (dispatch only, never
scheduled; `tools/denext_patch_bump.py`, upstream's `tools/auto_update_v8.ts` pointed at the
pinned branch) makes the move: it reads the branch head, refuses one whose `DEPS` moved or that
`denext/patches/v8` no longer applies to, bumps the crate's `+denext.<n>`, the release workflow's
default tag and the README, writes the release notes and a section here listing the merges, and
opens a pull request. It reports how long the pin has lacked the oldest merge it is missing (the
Deno fork's engine staleness gate fails a runtime tag past 14 days). After the merge, dispatch
`release-denext.yml` with the new tag. `--dry-run` (the `dry_run` input) writes the bump without
a branch or pull request. The next major is Extended Stable 160 (V8 16.0, about
mid-December 2026): move the submodules to its `DEPS`, re-export the patches (`git format-patch`
from a rebased checkout) and follow the binding changes.
