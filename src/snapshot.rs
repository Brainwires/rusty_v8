use crate::Context;
use crate::Data;
use crate::Local;
use crate::OwnedIsolate;
use crate::external_references::ExternalReference;
use crate::isolate::RealIsolate;
use crate::isolate_create_params::raw;
use crate::support::char;
use crate::support::int;

use std::borrow::Cow;
use std::mem::MaybeUninit;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::thread::ThreadId;

unsafe extern "C" {
  fn v8__SnapshotCreator__CONSTRUCT(
    buf: *mut MaybeUninit<SnapshotCreator>,
    params: *const raw::CreateParams,
  );
  fn v8__SnapshotCreator__DESTRUCT(this: *mut SnapshotCreator);
  fn v8__SnapshotCreator__GetIsolate(
    this: *const SnapshotCreator,
  ) -> *mut RealIsolate;
  fn v8__SnapshotCreator__CreateBlob(
    this: *mut SnapshotCreator,
    function_code_handling: FunctionCodeHandling,
  ) -> RawStartupData;
  fn v8__SnapshotCreator__SetDefaultContext(
    this: *mut SnapshotCreator,
    context: *const Context,
  );
  fn v8__SnapshotCreator__AddContext(
    this: *mut SnapshotCreator,
    context: *const Context,
  ) -> usize;
  fn v8__SnapshotCreator__AddData_to_isolate(
    this: *mut SnapshotCreator,
    data: *const Data,
  ) -> usize;
  fn v8__SnapshotCreator__AddData_to_context(
    this: *mut SnapshotCreator,
    context: *const Context,
    data: *const Data,
  ) -> usize;
  fn v8__StartupData__CanBeRehashed(this: *const RawStartupData) -> bool;
  fn v8__StartupData__IsValid(this: *const RawStartupData) -> bool;
  fn v8__StartupData__data__DELETE(this: *const char);
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawStartupData {
  pub(crate) data: *const char,
  pub(crate) raw_size: int,
}

#[derive(Debug, Clone)]
pub struct StartupData(StartupDataInner);

#[derive(Debug)]
enum StartupDataInner {
  Cpp(RawStartupData),
  Cow(Cow<'static, [u8]>),
}

impl StartupData {
  /// Whether the data created can be rehashed and and the hash seed can be
  /// recomputed when deserialized.
  /// Only valid for StartupData returned by SnapshotCreator::CreateBlob().
  pub fn can_be_rehashed(&self) -> bool {
    let tmp = match &self.0 {
      StartupDataInner::Cpp(t) => *t,
      StartupDataInner::Cow(c) => RawStartupData {
        data: c.as_ptr() as _,
        raw_size: c.len() as _,
      },
    };
    unsafe { v8__StartupData__CanBeRehashed(&tmp) }
  }

  /// Allows embedders to verify whether the data is valid for the current
  /// V8 instance.
  pub fn is_valid(&self) -> bool {
    let tmp = match &self.0 {
      StartupDataInner::Cpp(t) => *t,
      StartupDataInner::Cow(c) => RawStartupData {
        data: c.as_ptr() as _,
        raw_size: c.len() as _,
      },
    };
    unsafe { v8__StartupData__IsValid(&tmp) }
  }
}

impl std::ops::Deref for StartupData {
  type Target = [u8];

  fn deref(&self) -> &Self::Target {
    match &self.0 {
      StartupDataInner::Cpp(t) => unsafe {
        std::slice::from_raw_parts(t.data as _, t.raw_size as _)
      },
      StartupDataInner::Cow(c) => c,
    }
  }
}

impl<T> From<T> for StartupData
where
  T: Into<Cow<'static, [u8]>>,
{
  fn from(value: T) -> Self {
    Self(StartupDataInner::Cow(value.into()))
  }
}

impl Drop for StartupData {
  fn drop(&mut self) {
    if let StartupDataInner::Cpp(raw) = self.0 {
      unsafe {
        v8__StartupData__data__DELETE(raw.data);
      }
    }
  }
}

impl Clone for StartupDataInner {
  fn clone(&self) -> Self {
    match self {
      // Cpp -> Cow to maintain unique ownership of the underlying pointer
      Self::Cpp(r) => Self::Cow(
        unsafe { std::slice::from_raw_parts(r.data as _, r.raw_size as _) }
          .to_vec()
          .into(),
      ),
      Self::Cow(c) => Self::Cow(c.clone()),
    }
  }
}

#[repr(C)]
#[derive(Debug)]
pub enum FunctionCodeHandling {
  Clear,
  Keep,
}

/// Keeps every isolate on a read-only heap it can use.
///
/// Every isolate of the process shares one read-only heap (V8's isolate group
/// holds it), set up by the isolate that joins an empty group, from that
/// isolate's snapshot. V8 frees the heap when the group's last isolate leaves,
/// so this happens again each time the process runs out of isolates. Two things
/// go wrong when isolates are created regardless:
///
/// 1. **A heap a snapshot creator is still writing.** When the isolate that
///    joins the empty group is a `SnapshotCreator`, V8 leaves the heap writable
///    so the snapshot can extend it, and only seals it when the blob is created
///    (or the creator is destroyed): it appends pages, then detaches them into
///    the shared space and deletes the space the creator started with. An
///    isolate created in that window attaches to the creator's writable space:
///    it allocates into it (a SIGSEGV in `ReadOnlySpace::EnsureSpaceForAllocation`),
///    reads its page list while the creator appends to it, and keeps a pointer
///    to the space V8 deletes on sealing.
/// 2. **A heap from another snapshot.** An isolate's startup snapshot refers to
///    read-only objects by page and offset in the read-only image it was made
///    with, but V8 attaches every isolate to the group's heap whatever its
///    snapshot holds. A snapshot whose creator extended the read-only heap has
///    an image of its own, so an isolate made from it while the group holds
///    another image reads past the heap's pages (libc++'s "vector[] index out
///    of bounds" in `Deserializer::ReadReadOnlyHeapRef`), and the reverse
///    resolves references to the wrong objects. V8 only checks this in debug
///    builds (`ReadOnlyArtifacts::VerifyChecksum`).
///
/// V8 expects snapshots to be made in a process of their own (as `mksnapshot`
/// does), and one snapshot per process. The binding enforces both: isolate
/// creation waits while a creator's window is open, and an isolate whose
/// snapshot has another read-only image than the group's waits until the group
/// is empty, then sets the heap up from its own. Isolates of the image the group
/// holds are held back meanwhile, so the group drains, unless their thread
/// already owns an isolate (which must not wait on the others). Creating an
/// isolate on the creator's own thread, or one of another image on a thread
/// that owns an isolate of the group, would wait forever and panics instead, as
/// does a wait for the group to drain that outlasts [`DRAIN_TIMEOUT`].
///
/// An image is known by the read-only checksum in the snapshot blob's header.
/// V8's built-in snapshot is told apart without one. A creator that joins a
/// sealed heap cannot extend it, so the blob it makes refers to that heap's
/// image: the guard records the blob's checksum as that image. A blob from
/// elsewhere whose creator did not extend the heap either is taken for an
/// image of its own, so its isolates wait for the group to drain when they did
/// not need to, which is safe. A process whose isolates all come from one
/// snapshot never waits.
///
/// A snapshot creator must know whether it is joining an empty group, which
/// `live` only answers while no isolate is being created or disposed: a creator
/// therefore starts alone, while the others may overlap each other (V8 sets up
/// a sealed heap under the group's mutex).
pub(crate) mod read_only_heap_guard {
  use super::*;
  use std::time::Duration;
  use std::time::Instant;

  /// How long an isolate waits for the isolates of another read-only image to
  /// be disposed before it panics.
  pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(120);
  /// How long an isolate of the group's image is held back while another
  /// image waits for the group to drain. Past it, it proceeds: holding it
  /// longer could deadlock a thread that waits for it while it owns an isolate.
  const HOLD_BACK: Duration = Duration::from_secs(5);

  /// The read-only image a snapshot holds, or that the group's heap was set
  /// up from.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum Image {
    /// V8's built-in snapshot.
    Default,
    /// A snapshot blob, by the checksum of its read-only part.
    Blob(u32),
    /// A heap a snapshot creator has extended without producing a blob: no
    /// snapshot matches it.
    Unknown,
  }

  /// `kReadOnlySnapshotChecksumOffset` of V8's snapshot blob header
  /// (`src/snapshot/snapshot.cc`, `SnapshotImpl`): after the number of
  /// contexts, the rehashability flag and the blob's checksum, each a
  /// `uint32_t`. The same in V8 15.2 and 15.6.
  const READ_ONLY_CHECKSUM_OFFSET: usize = 12;

  /// The read-only image of the snapshot `blob` points to; null is V8's
  /// built-in snapshot.
  pub(crate) fn image_of(blob: *const RawStartupData) -> Image {
    if blob.is_null() {
      return Image::Default;
    }
    let blob = unsafe { *blob };
    blob_image(blob.data, blob.raw_size as usize)
  }

  fn blob_image(data: *const char, len: usize) -> Image {
    if data.is_null() || len < READ_ONLY_CHECKSUM_OFFSET + 4 {
      // V8 refuses such a blob; it never sets up a heap.
      return Image::Unknown;
    }
    let mut bytes = [0u8; 4];
    unsafe {
      std::ptr::copy_nonoverlapping(
        (data as *const u8).add(READ_ONLY_CHECKSUM_OFFSET),
        bytes.as_mut_ptr(),
        4,
      );
    }
    Image::Blob(u32::from_ne_bytes(bytes))
  }

  struct State {
    /// Isolates that V8's isolate group holds, while `in_flight` is zero.
    live: usize,
    /// Regular isolates being created and isolates being disposed.
    in_flight: usize,
    /// A snapshot creator is being constructed.
    constructing_creator: bool,
    /// The snapshot creator that owns the writable read-only heap: its thread
    /// and its `v8::SnapshotCreator`'s implementation pointer.
    open: Option<(ThreadId, usize)>,
    /// The image the group's heap holds, while the group is not empty.
    image: Option<Image>,
    /// Threads waiting for the group to drain, to set up another image.
    draining: usize,
    /// Each live isolate and the thread that created it.
    owners: Vec<(usize, ThreadId)>,
    /// Snapshot creators that joined a sealed heap, by
    /// [`SnapshotCreator::id`], and that heap's image.
    attached_creators: Vec<(usize, Image)>,
    /// Images of blobs this process made on a sealed heap: such a creator
    /// cannot extend the heap, so its blob refers to the heap's image.
    aliases: Vec<(u32, Image)>,
  }

  impl State {
    fn empty(&self) -> bool {
      self.live == 0 && self.in_flight == 0 && !self.constructing_creator
    }

    fn owns_an_isolate(&self, thread: ThreadId) -> bool {
      self.owners.iter().any(|(_, owner)| *owner == thread)
    }

    /// The image a heap set up from `image` holds.
    fn canonical(&self, image: Image) -> Image {
      match image {
        Image::Blob(checksum) => self
          .aliases
          .iter()
          .find(|(c, _)| *c == checksum)
          .map_or(image, |(_, canonical)| *canonical),
        _ => image,
      }
    }
  }

  static STATE: Mutex<State> = Mutex::new(State {
    live: 0,
    in_flight: 0,
    constructing_creator: false,
    open: None,
    image: None,
    draining: 0,
    owners: Vec::new(),
    attached_creators: Vec::new(),
    aliases: Vec::new(),
  });
  static CHANGED: Condvar = Condvar::new();

  fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
  }

  fn wait(state: MutexGuard<'static, State>) -> MutexGuard<'static, State> {
    CHANGED.wait(state).unwrap_or_else(|e| e.into_inner())
  }

  fn wait_until(
    state: MutexGuard<'static, State>,
    deadline: Instant,
  ) -> MutexGuard<'static, State> {
    let timeout = deadline.saturating_duration_since(Instant::now());
    CHANGED
      .wait_timeout(state, timeout)
      .map(|(state, _)| state)
      .unwrap_or_else(|e| e.into_inner().0)
  }

  /// Waits until an isolate of `image` may join the group, and claims it:
  /// returns whether this isolate sets up the group's heap. `creator` waits,
  /// in addition, for every other creation and disposal to finish.
  fn admit(image: Image, creator: bool) -> (MutexGuard<'static, State>, bool) {
    let thread = std::thread::current().id();
    let start = Instant::now();
    let mut state = lock();
    let image = state.canonical(image);
    let mut draining = false;
    let result = loop {
      if let Some((owner, _)) = state.open {
        if owner == thread {
          if draining {
            state.draining -= 1;
          }
          drop(state);
          panic!(
            "cannot create an isolate while this thread's v8::Isolate::snapshot_creator, the only isolate of the process, is alive: call create_blob first, or create any other isolate before the snapshot creator"
          );
        }
        state = wait(state);
        continue;
      }
      if state.constructing_creator || (creator && state.in_flight > 0) {
        state = wait(state);
        continue;
      }
      if state.empty() {
        break true;
      }
      if state.image == Some(image) && image != Image::Unknown {
        let held_back = state.draining > 0
          && !draining
          && !state.owns_an_isolate(thread)
          && start.elapsed() < HOLD_BACK;
        if !held_back {
          break false;
        }
        state = wait_until(state, start + HOLD_BACK);
        continue;
      }
      // The group holds another image: wait for it to drain.
      if state.owns_an_isolate(thread) {
        if draining {
          state.draining -= 1;
        }
        let held = state.image;
        drop(state);
        panic!(
          "cannot create an isolate from a snapshot with another read-only heap ({image:?}) than the process's isolates have ({held:?}) while this thread owns one of them: V8 shares one read-only heap between all isolates of a process, so isolates of different snapshots cannot be alive at the same time"
        );
      }
      if start.elapsed() >= DRAIN_TIMEOUT {
        if draining {
          state.draining -= 1;
        }
        let held = state.image;
        drop(state);
        panic!(
          "waited {DRAIN_TIMEOUT:?} to create an isolate from a snapshot with another read-only heap ({image:?}) than the process's isolates have ({held:?}): V8 shares one read-only heap between all isolates of a process, so it can only be created once they are all disposed"
        );
      }
      if !draining {
        draining = true;
        state.draining += 1;
      }
      state = wait_until(state, start + DRAIN_TIMEOUT);
    };
    if draining {
      state.draining -= 1;
      CHANGED.notify_all();
    }
    if result {
      state.image = Some(image);
    }
    (state, result)
  }

  /// Runs `create`, which calls V8 to create a regular isolate from a
  /// snapshot of `image`.
  pub(crate) fn create_isolate(
    image: Image,
    create: impl FnOnce() -> *mut RealIsolate,
  ) -> *mut RealIsolate {
    let (mut state, _) = admit(image, false);
    state.in_flight += 1;
    drop(state);

    let isolate = create();

    let mut state = lock();
    state.in_flight -= 1;
    state.live += 1;
    state
      .owners
      .push((isolate as usize, std::thread::current().id()));
    CHANGED.notify_all();
    isolate
  }

  /// Runs `construct`, which calls V8 to construct a snapshot creator from a
  /// snapshot of `image`. It holds the read-only heap open when it is the
  /// group's only isolate, until [`snapshot_creator_done`] is called for it.
  pub(crate) fn create_snapshot_creator(
    image: Image,
    construct: impl FnOnce() -> SnapshotCreator,
  ) -> SnapshotCreator {
    let (mut state, first) = admit(image, true);
    state.constructing_creator = true;
    drop(state);

    let creator = construct();
    let isolate = unsafe { v8__SnapshotCreator__GetIsolate(&creator) };

    let mut state = lock();
    state.constructing_creator = false;
    state.live += 1;
    state
      .owners
      .push((isolate as usize, std::thread::current().id()));
    if first {
      state.open = Some((std::thread::current().id(), creator.id()));
      state.image = Some(Image::Unknown);
    } else if let Some(image) = state.image {
      state.attached_creators.push((creator.id(), image));
    }
    CHANGED.notify_all();
    creator
  }

  /// Runs `dispose`, which calls V8 to dispose `isolate` or destroy the
  /// snapshot creator that owns it.
  pub(crate) fn dispose_isolate(
    isolate: *mut RealIsolate,
    dispose: impl FnOnce(),
  ) {
    let mut state = lock();
    while state.constructing_creator {
      state = wait(state);
    }
    state.in_flight += 1;
    drop(state);

    dispose();

    let mut state = lock();
    state.in_flight -= 1;
    state.live -= 1;
    if let Some(i) = state
      .owners
      .iter()
      .position(|(ptr, _)| *ptr == isolate as usize)
    {
      state.owners.swap_remove(i);
    }
    if state.empty() {
      state.image = None;
    }
    CHANGED.notify_all();
  }

  /// The creator with this [`SnapshotCreator::id`] has created `blob` (or
  /// failed to, or has been destroyed): V8 has sealed the read-only heap.
  pub(crate) fn snapshot_creator_done(
    creator: usize,
    blob: Option<&RawStartupData>,
  ) {
    let mut state = lock();
    if matches!(state.open, Some((_, id)) if id == creator) {
      state.open = None;
      if !state.empty() {
        // The heap holds what the creator added: the image its blob carries.
        state.image = Some(match blob {
          Some(blob) => blob_image(blob.data, blob.raw_size as usize),
          None => Image::Unknown,
        });
      }
      CHANGED.notify_all();
    } else if let Some(i) = state
      .attached_creators
      .iter()
      .position(|(id, _)| *id == creator)
    {
      let (_, heap) = state.attached_creators.swap_remove(i);
      if let Some(Image::Blob(checksum)) =
        blob.map(|blob| blob_image(blob.data, blob.raw_size as usize))
        && heap != Image::Unknown
        && !state.aliases.iter().any(|(c, _)| *c == checksum)
      {
        state.aliases.push((checksum, heap));
      }
    }
  }
}

/// Helper class to create a snapshot data blob.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct SnapshotCreator([usize; 1]);

impl SnapshotCreator {
  /// Identifies this creator while it is alive: the address of its
  /// `v8::SnapshotCreator`'s implementation, which moving it does not change.
  fn id(&self) -> usize {
    self.0[0]
  }

  /// Create an isolate, and set it up for serialization.
  /// The isolate is created from scratch.
  #[inline(always)]
  #[allow(clippy::new_ret_no_self)]
  pub(crate) fn new(
    external_references: Option<Cow<'static, [ExternalReference]>>,
    params: Option<crate::CreateParams>,
  ) -> OwnedIsolate {
    Self::new_impl(external_references, None, params)
  }

  /// Create an isolate, and set it up for serialization.
  /// The isolate is created from scratch.
  #[inline(always)]
  #[allow(clippy::new_ret_no_self)]
  pub(crate) fn from_existing_snapshot(
    existing_snapshot_blob: StartupData,
    external_references: Option<Cow<'static, [ExternalReference]>>,
    params: Option<crate::CreateParams>,
  ) -> OwnedIsolate {
    Self::new_impl(external_references, Some(existing_snapshot_blob), params)
  }

  /// Create and enter an isolate, and set it up for serialization.
  /// The isolate is created from scratch.
  #[inline(always)]
  #[allow(clippy::new_ret_no_self)]
  fn new_impl(
    external_references: Option<Cow<'static, [ExternalReference]>>,
    existing_snapshot_blob: Option<StartupData>,
    params: Option<crate::CreateParams>,
  ) -> OwnedIsolate {
    let mut snapshot_creator: MaybeUninit<Self> = MaybeUninit::uninit();

    let mut params = params.unwrap_or_default();
    if let Some(external_refs) = external_references {
      params = params.external_references(external_refs);
    }
    if let Some(snapshot_blob) = existing_snapshot_blob {
      params = params.snapshot_blob(snapshot_blob);
    }
    let (raw_create_params, create_param_allocations) = params.finalize();

    let image = read_only_heap_guard::image_of(raw_create_params.snapshot_blob);
    let snapshot_creator =
      read_only_heap_guard::create_snapshot_creator(image, || unsafe {
        v8__SnapshotCreator__CONSTRUCT(
          &mut snapshot_creator,
          &raw_create_params,
        );
        snapshot_creator.assume_init()
      });

    let isolate_ptr =
      unsafe { v8__SnapshotCreator__GetIsolate(&snapshot_creator) };
    let mut owned_isolate = OwnedIsolate::new_already_entered(isolate_ptr);
    owned_isolate.initialize(create_param_allocations);
    owned_isolate.set_snapshot_creator(snapshot_creator);
    owned_isolate
  }
}

impl Drop for SnapshotCreator {
  fn drop(&mut self) {
    let id = self.id();
    let isolate = unsafe { v8__SnapshotCreator__GetIsolate(self) };
    read_only_heap_guard::dispose_isolate(isolate, || unsafe {
      v8__SnapshotCreator__DESTRUCT(self)
    });
    // The destructor seals the read-only heap if CreateBlob never ran.
    read_only_heap_guard::snapshot_creator_done(id, None);
  }
}

impl SnapshotCreator {
  /// Set the default context to be included in the snapshot blob.
  /// The snapshot will not contain the global proxy, and we expect one or a
  /// global object template to create one, to be provided upon deserialization.
  #[inline(always)]
  pub(crate) fn set_default_context(&mut self, context: Local<Context>) {
    unsafe { v8__SnapshotCreator__SetDefaultContext(self, &*context) };
  }

  /// Add additional context to be included in the snapshot blob.
  /// The snapshot will include the global proxy.
  ///
  /// Returns the index of the context in the snapshot blob.
  #[inline(always)]
  pub(crate) fn add_context(&mut self, context: Local<Context>) -> usize {
    unsafe { v8__SnapshotCreator__AddContext(self, &*context) }
  }

  /// Attach arbitrary `v8::Data` to the isolate snapshot, which can be
  /// retrieved via `HandleScope::get_context_data_from_snapshot_once()` after
  /// deserialization. This data does not survive when a new snapshot is created
  /// from an existing snapshot.
  #[inline(always)]
  pub(crate) fn add_isolate_data<T>(&mut self, data: Local<T>) -> usize
  where
    for<'l> Local<'l, T>: Into<Local<'l, Data>>,
  {
    unsafe { v8__SnapshotCreator__AddData_to_isolate(self, &*data.into()) }
  }

  /// Attach arbitrary `v8::Data` to the context snapshot, which can be
  /// retrieved via `HandleScope::get_context_data_from_snapshot_once()` after
  /// deserialization. This data does not survive when a new snapshot is
  /// created from an existing snapshot.
  #[inline(always)]
  pub(crate) fn add_context_data<T>(
    &mut self,
    context: Local<Context>,
    data: Local<T>,
  ) -> usize
  where
    for<'l> Local<'l, T>: Into<Local<'l, Data>>,
  {
    unsafe {
      v8__SnapshotCreator__AddData_to_context(self, &*context, &*data.into())
    }
  }

  /// Creates a snapshot data blob.
  /// This must not be called from within a handle scope.
  #[inline(always)]
  pub(crate) fn create_blob(
    &mut self,
    function_code_handling: FunctionCodeHandling,
  ) -> Option<StartupData> {
    let blob =
      unsafe { v8__SnapshotCreator__CreateBlob(self, function_code_handling) };
    // CreateBlob seals the read-only heap before it serializes it.
    read_only_heap_guard::snapshot_creator_done(
      self.id(),
      (!blob.data.is_null()).then_some(&blob),
    );
    if blob.data.is_null() {
      debug_assert!(blob.raw_size == 0);
      None
    } else {
      debug_assert!(blob.raw_size > 0);
      Some(StartupData(StartupDataInner::Cpp(blob)))
    }
  }
}
