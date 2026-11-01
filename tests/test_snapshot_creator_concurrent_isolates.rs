// Regression test: a SnapshotCreator that is the first isolate of the process
// owns the isolate group's read-only heap and keeps it writable until its blob
// is created. Isolates created on other threads in that window used to attach
// to the creator's writable read-only space, then read its page list while the
// creator appended to it (libc++ "vector[] index out of bounds" in
// Deserializer::ReadReadOnlyHeapRef) and kept a pointer to the space V8
// deletes when it seals the heap. They now wait until the creator is done.
//
// This must stay the only test in its binary: it needs the snapshot creator to
// be the process's first isolate.

use std::sync::Arc;
use std::sync::Barrier;
use std::thread;
use std::time::Duration;

fn run_script(isolate: &mut v8::Isolate, code: &str) -> i32 {
  let scope = std::pin::pin!(v8::HandleScope::new(isolate));
  let scope = &mut scope.init();
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let code = v8::String::new(scope, code).unwrap();
  let script = v8::Script::compile(scope, code, None).unwrap();
  script.run(scope).unwrap().int32_value(scope).unwrap()
}

#[test]
fn isolates_created_while_the_first_snapshot_creator_is_alive() {
  v8::V8::initialize_platform(
    v8::new_unprotected_default_platform(0, false).make_shared(),
  );
  v8::V8::initialize();

  const THREADS: usize = 16;
  const ISOLATES_PER_THREAD: usize = 4;

  // The first isolate of the process.
  let mut creator = v8::Isolate::snapshot_creator(None, None);

  // Creating a regular isolate on the creator's own thread would wait for the
  // creator forever, so it panics instead (and leaves the creator usable).
  let same_thread = std::panic::catch_unwind(|| {
    v8::Isolate::new(Default::default());
  });
  assert!(same_thread.is_err());

  let start = Arc::new(Barrier::new(THREADS + 1));
  let workers = (0..THREADS)
    .map(|i| {
      let start = start.clone();
      thread::spawn(move || {
        start.wait();
        for j in 0..ISOLATES_PER_THREAD {
          let mut isolate = v8::Isolate::new(Default::default());
          let n = (i * ISOLATES_PER_THREAD + j) as i32;
          let code =
            format!("[1, 2, 3].map((x) => x * {n}).reduce((a, b) => a + b)");
          assert_eq!(run_script(&mut isolate, &code), 6 * n);
        }
      })
    })
    .collect::<Vec<_>>();
  start.wait();

  // Give the workers time to reach isolate creation while the creator still
  // extends its read-only space.
  thread::sleep(Duration::from_millis(50));
  {
    let scope = std::pin::pin!(v8::HandleScope::new(&mut creator));
    let scope = &mut scope.init();
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let code = v8::String::new(scope, "globalThis.answer = 42").unwrap();
    v8::Script::compile(scope, code, None)
      .unwrap()
      .run(scope)
      .unwrap();
    scope.set_default_context(context);
  }
  let blob = creator.create_blob(v8::FunctionCodeHandling::Keep).unwrap();

  for worker in workers {
    worker.join().unwrap();
  }

  // The blob is usable, and so is the process's read-only heap.
  let mut isolate =
    v8::Isolate::new(v8::CreateParams::default().snapshot_blob(blob));
  let scope = std::pin::pin!(v8::HandleScope::new(&mut isolate));
  let scope = &mut scope.init();
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let code = v8::String::new(scope, "answer").unwrap();
  let answer = v8::Script::compile(scope, code, None)
    .unwrap()
    .run(scope)
    .unwrap();
  assert_eq!(answer.int32_value(scope), Some(42));
}
