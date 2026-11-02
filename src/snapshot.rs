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

/// Keeps isolates away from a read-only heap that a [`SnapshotCreator`] is
/// still writing.
///
/// Every isolate of the process shares one read-only heap (V8's isolate group
/// holds it), set up by the isolate that joins an empty group. V8 frees the
/// heap when the group's last isolate leaves, so this happens again each time
/// the process runs out of isolates. When that isolate is a `SnapshotCreator`,
/// V8 leaves the heap writable so the snapshot can extend it, and only seals it
/// when the blob is created (or the creator is destroyed): it appends pages,
/// then detaches them into the shared space and deletes the space the creator
/// started with. An isolate created in that window attaches to the creator's
/// writable space: on another thread it reads the page list while the creator
/// appends to it (libc++'s "vector[] index out of bounds" in
/// `Deserializer::ReadReadOnlyHeapRef`) or allocates into it, and on any thread
/// it keeps a pointer to the space V8 deletes on sealing. V8 expects snapshots
/// to be made in a process of their own (as `mksnapshot` does), so the binding
/// enforces the window: isolate creation waits while it is open, and creating
/// an isolate on the creator's own thread, which would wait forever, panics.
///
/// A snapshot creator must know whether it is joining an empty group, which
/// `live` only answers while no isolate is being created or disposed: a creator
/// therefore starts alone, while the others may overlap each other (V8 sets up
/// a sealed heap under the group's mutex).
pub(crate) mod read_only_heap_guard {
  use super::*;

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
  }

  static STATE: Mutex<State> = Mutex::new(State {
    live: 0,
    in_flight: 0,
    constructing_creator: false,
    open: None,
  });
  static CHANGED: Condvar = Condvar::new();

  fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
  }

  fn wait(state: MutexGuard<'static, State>) -> MutexGuard<'static, State> {
    CHANGED.wait(state).unwrap_or_else(|e| e.into_inner())
  }

  /// Waits for the window of an open snapshot creator to close.
  fn wait_for_open_creator(
    mut state: MutexGuard<'static, State>,
  ) -> MutexGuard<'static, State> {
    while let Some((owner, _)) = state.open {
      if owner == std::thread::current().id() {
        drop(state);
        panic!(
          "cannot create an isolate while this thread's v8::Isolate::snapshot_creator, the only isolate of the process, is alive: call create_blob first, or create any other isolate before the snapshot creator"
        );
      }
      state = wait(state);
    }
    state
  }

  /// Runs `create`, which calls V8 to create a regular isolate.
  pub(crate) fn create_isolate<T>(create: impl FnOnce() -> T) -> T {
    let mut state = wait_for_open_creator(lock());
    while state.constructing_creator {
      state = wait_for_open_creator(wait(state));
    }
    state.in_flight += 1;
    drop(state);

    let isolate = create();

    let mut state = lock();
    state.in_flight -= 1;
    state.live += 1;
    CHANGED.notify_all();
    isolate
  }

  /// Runs `construct`, which calls V8 to construct a snapshot creator. It
  /// holds the read-only heap open when it is the group's only isolate, until
  /// [`snapshot_creator_done`] is called for it.
  pub(crate) fn create_snapshot_creator(
    construct: impl FnOnce() -> SnapshotCreator,
  ) -> SnapshotCreator {
    let mut state = wait_for_open_creator(lock());
    while state.constructing_creator || state.in_flight > 0 {
      state = wait_for_open_creator(wait(state));
    }
    state.constructing_creator = true;
    let first = state.live == 0;
    drop(state);

    let creator = construct();

    let mut state = lock();
    state.constructing_creator = false;
    state.live += 1;
    if first {
      state.open = Some((std::thread::current().id(), creator.id()));
    }
    CHANGED.notify_all();
    creator
  }

  /// Runs `dispose`, which calls V8 to dispose an isolate or destroy a
  /// snapshot creator.
  pub(crate) fn dispose_isolate(dispose: impl FnOnce()) {
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
    CHANGED.notify_all();
  }

  /// The creator with this [`SnapshotCreator::id`] has created its blob or has
  /// been destroyed: V8 has sealed the read-only heap.
  pub(crate) fn snapshot_creator_done(creator: usize) {
    let mut state = lock();
    if matches!(state.open, Some((_, id)) if id == creator) {
      state.open = None;
      CHANGED.notify_all();
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

    let snapshot_creator =
      read_only_heap_guard::create_snapshot_creator(|| unsafe {
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
    read_only_heap_guard::dispose_isolate(|| unsafe {
      v8__SnapshotCreator__DESTRUCT(self)
    });
    // The destructor seals the read-only heap if CreateBlob never ran.
    read_only_heap_guard::snapshot_creator_done(id);
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
    read_only_heap_guard::snapshot_creator_done(self.id());
    if blob.data.is_null() {
      debug_assert!(blob.raw_size == 0);
      None
    } else {
      debug_assert!(blob.raw_size > 0);
      Some(StartupData(StartupDataInner::Cpp(blob)))
    }
  }
}
