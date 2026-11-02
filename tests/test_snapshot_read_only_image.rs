// Regression test: every isolate of a process shares one read-only heap, set
// up from the snapshot of the isolate that joins an empty isolate group. A
// snapshot whose creator was the group's only isolate carries a read-only image
// of its own, and an isolate made from it used to attach to the heap of V8's
// built-in snapshot when another isolate held that heap: its deserializer read
// past the heap's pages and the process aborted ("vector[] index out of
// bounds" in `Deserializer::ReadReadOnlyHeapRef`), on one thread. Such an
// isolate now waits until the isolates of the other image are disposed, and
// panics where that wait could never end.
//
// This must stay the only test in its binary: it needs to empty the group.

use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

fn run_script(isolate: &mut v8::Isolate, code: &str) -> String {
  let scope = std::pin::pin!(v8::HandleScope::new(isolate));
  let scope = &mut scope.init();
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let code = v8::String::new(scope, code).unwrap();
  let script = v8::Script::compile(scope, code, None).unwrap();
  script.run(scope).unwrap().to_rust_string_lossy(scope)
}

/// A snapshot made while its creator is the group's only isolate, so V8
/// extends the read-only heap with the creator's strings.
fn make_snapshot() -> Vec<u8> {
  let mut creator = v8::Isolate::snapshot_creator(None, None);
  {
    let scope = std::pin::pin!(v8::HandleScope::new(&mut creator));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let code = v8::String::new(
      scope,
      "globalThis.table = {}; \
       for (let i = 0; i < 20000; i++) table['key' + i] = 'value' + i;",
    )
    .unwrap();
    v8::Script::compile(scope, code, None)
      .unwrap()
      .run(scope)
      .unwrap();
    scope.set_default_context(context);
  }
  creator
    .create_blob(v8::FunctionCodeHandling::Keep)
    .unwrap()
    .to_vec()
}

fn from_snapshot(blob: &[u8]) -> v8::OwnedIsolate {
  v8::Isolate::new(
    v8::Isolate::create_params()
      .snapshot_blob(v8::StartupData::from(blob.to_vec())),
  )
}

const FROM_SNAPSHOT: &str = "Object.keys(table).length + ':' + table.key7";
const BUILT_IN: &str = "typeof table";

#[test]
fn isolates_of_different_read_only_images() {
  v8::V8::initialize_platform(
    v8::new_unprotected_default_platform(0, false).make_shared(),
  );
  v8::V8::initialize();

  let blob = Arc::new(make_snapshot());
  // The group is empty again.

  // Isolates of one snapshot share the heap without waiting.
  {
    let mut a = from_snapshot(&blob);
    assert_eq!(run_script(&mut a, FROM_SNAPSHOT), "20000:value7");
    let mut b = from_snapshot(&blob);
    assert_eq!(run_script(&mut b, FROM_SNAPSHOT), "20000:value7");
    drop(b);
    drop(a);
  }

  // A built-in isolate holds the heap on another thread; one from the
  // snapshot waits for it to be disposed. (This order aborted the process.)
  for round in 0..3 {
    let barrier = Arc::new(Barrier::new(2));
    let disposed = Arc::new(AtomicBool::new(false));
    let holder = {
      let barrier = barrier.clone();
      let disposed = disposed.clone();
      thread::spawn(move || {
        let mut isolate = v8::Isolate::new(Default::default());
        assert_eq!(run_script(&mut isolate, BUILT_IN), "undefined");
        barrier.wait();
        thread::sleep(Duration::from_millis(300));
        disposed.store(true, Ordering::SeqCst);
        drop(isolate);
      })
    };
    barrier.wait();
    let mut isolate = from_snapshot(&blob);
    assert!(disposed.load(Ordering::SeqCst), "round {round}");
    assert_eq!(run_script(&mut isolate, FROM_SNAPSHOT), "20000:value7");
    drop(isolate);
    holder.join().unwrap();
  }

  // The reverse: the snapshot's heap is held, a built-in isolate waits.
  {
    let barrier = Arc::new(Barrier::new(2));
    let disposed = Arc::new(AtomicBool::new(false));
    let holder = {
      let barrier = barrier.clone();
      let disposed = disposed.clone();
      let blob = blob.clone();
      thread::spawn(move || {
        let mut isolate = from_snapshot(&blob);
        assert_eq!(run_script(&mut isolate, FROM_SNAPSHOT), "20000:value7");
        barrier.wait();
        thread::sleep(Duration::from_millis(300));
        disposed.store(true, Ordering::SeqCst);
        drop(isolate);
      })
    };
    barrier.wait();
    let mut isolate = v8::Isolate::new(Default::default());
    assert!(disposed.load(Ordering::SeqCst));
    assert_eq!(run_script(&mut isolate, BUILT_IN), "undefined");
    drop(isolate);
    holder.join().unwrap();
  }

  // Many threads mixing both kinds: each isolate runs on a heap of its own
  // image.
  let threads: Vec<_> = (0..8)
    .map(|i| {
      let blob = blob.clone();
      thread::spawn(move || {
        for _ in 0..20 {
          if i % 2 == 0 {
            let mut isolate = from_snapshot(&blob);
            assert_eq!(run_script(&mut isolate, FROM_SNAPSHOT), "20000:value7");
          } else {
            let mut isolate = v8::Isolate::new(Default::default());
            assert_eq!(run_script(&mut isolate, BUILT_IN), "undefined");
          }
        }
      })
    })
    .collect();
  for t in threads {
    t.join().unwrap();
  }

  // A snapshot made while a built-in isolate holds the heap cannot extend it:
  // its isolates share the built-in heap and do not wait.
  {
    let built_in = v8::Isolate::new(Default::default());
    let attached = make_snapshot();
    let mut isolate = from_snapshot(&attached);
    assert_eq!(run_script(&mut isolate, FROM_SNAPSHOT), "20000:value7");
    drop(isolate);
    drop(built_in);
  }

  // On a thread that owns an isolate of the other image, the wait could
  // never end: it panics instead, and the binding stays usable.
  {
    let _built_in = v8::Isolate::new(Default::default());
    let blob = blob.clone();
    let result = std::panic::catch_unwind(move || from_snapshot(&blob));
    let Err(message) = result else {
      panic!("expected a panic");
    };
    let message = message
      .downcast_ref::<String>()
      .cloned()
      .unwrap_or_default();
    assert!(message.contains("another read-only heap"), "{message}");
  }
  let mut isolate = from_snapshot(&blob);
  assert_eq!(run_script(&mut isolate, FROM_SNAPSHOT), "20000:value7");
}
