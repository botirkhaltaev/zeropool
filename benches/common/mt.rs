//! Multi-threaded timing helper for `iter_custom` benchmarks.
//!
//! Threads are spawned inside a `thread::scope`, so the strategy and any
//! channels can be borrowed for the duration of the call. Timing starts
//! when a shared barrier releases all workers and ends when every worker
//! reports done at a second barrier — spawn/teardown cost is excluded.

#![allow(dead_code)]

use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

/// Spawn `threads` scoped workers, release them together, and return the
/// elapsed time from barrier release until the last worker finishes.
pub fn run_threads<F>(threads: usize, f: F) -> Duration
where
    F: Fn(usize) + Sync,
{
    let start = Barrier::new(threads + 1);
    let done = Barrier::new(threads + 1);
    let start = &start;
    let done = &done;
    let f = &f;

    thread::scope(|s| {
        for i in 0..threads {
            s.spawn(move || {
                start.wait();
                f(i);
                done.wait();
            });
        }
        start.wait();
        let t = Instant::now();
        done.wait();
        t.elapsed()
    })
}
