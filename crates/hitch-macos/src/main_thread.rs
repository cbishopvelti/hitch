//! Running work on the main thread.

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;

/// Runs `f` on the main thread: now if this is the main thread, else on the
/// main dispatch queue, which the app's event loop drains. Never blocks, so
/// it is safe to call before the event loop starts or while the main thread
/// waits on the caller.
pub(crate) fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    match MainThreadMarker::new() {
        Some(mtm) => f(mtm),
        None => DispatchQueue::main().exec_async(move || {
            // The main queue only ever runs on the main thread.
            let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
            f(mtm);
        }),
    }
}
