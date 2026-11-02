// Regression test: V8 frees the isolate group's read-only heap when the
// group's last isolate is disposed, and the next isolate sets up a new one. A
// snapshot creator that becomes that next isolate keeps the new heap writable
// until its blob is created, as it does when it is the process's first isolate.
// Isolates created on other threads in that window used to attach to the
// writable heap, because the binding took the heap to stay sealed once any
// regular isolate had existed. They now wait for the creator.
//
// This must stay the only test in its binary: it needs to empty the group.

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

fn make_snapshot(mut creator: v8::OwnedIsolate) -> v8::StartupData {
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
  creator.create_blob(v8::FunctionCodeHandling::Keep).unwrap()
}

#[test]
fn isolates_created_while_a_snapshot_creator_is_the_only_isolate() {
  v8::V8::initialize_platform(
    v8::new_unprotected_default_platform(0, false).make_shared(),
  );
  v8::V8::initialize();

  const THREADS: usize = 16;
  const ISOLATES_PER_THREAD: usize = 4;

  // Several rounds, so the window opens on a heap V8 has set up anew each time.
  for round in 0..5 {
    // A regular isolate has existed, and now none does.
    let mut first = v8::Isolate::new(Default::default());
    assert_eq!(run_script(&mut first, "1 + 1"), 2);
    drop(first);

    let creator = v8::Isolate::snapshot_creator(None, None);

    // Waiting on the creator's own thread would never end.
    let same_thread = std::panic::catch_unwind(|| {
      v8::Isolate::new(Default::default());
    });
    assert!(same_thread.is_err(), "round {round}");

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
    thread::sleep(Duration::from_millis(20));
    let blob = make_snapshot(creator);

    for worker in workers {
      worker.join().unwrap();
    }

    let mut isolate =
      v8::Isolate::new(v8::CreateParams::default().snapshot_blob(blob));
    assert_eq!(run_script(&mut isolate, "answer"), 42);
  }

  // Beside a regular isolate the heap is sealed, so a creator holds nothing up:
  // an isolate on the creator's own thread does not wait, or panic.
  let bystander = v8::Isolate::new(Default::default());
  let creator = v8::Isolate::snapshot_creator(None, None);
  let on_creator_thread = v8::Isolate::new(Default::default());
  drop(on_creator_thread);
  let blob = make_snapshot(creator);
  assert!(!blob.is_empty());
  drop(bystander);
}
