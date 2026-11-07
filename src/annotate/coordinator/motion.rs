use std::time::Instant;

thread_local! {
    static START: Instant = Instant::now();
}

pub(crate) fn now() -> f64 {
    START.with(|start| start.elapsed().as_secs_f64())
}
