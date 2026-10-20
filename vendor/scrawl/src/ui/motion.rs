//! The capsule's animation clock.

use std::time::Instant;

thread_local! {
    static START: Instant = Instant::now();
}

/// Animation time in seconds, the clock the native capsule and the switch chips run on.
pub(crate) fn now() -> f64 {
    START.with(|start| start.elapsed().as_secs_f64())
}
