//! Shared configuration for the workload benchmarks.

#![allow(dead_code)]

use zeropool::ZeroPool;

/// Number of power-of-two size classes, matching ZeroPool's layout
/// (4 KiB through 64 MiB, ×4 per step).
pub const NUM_CLASSES: usize = 8;

/// Smallest class size in bytes (4 KiB = 2^12).
pub const MIN_CLASS_SIZE: usize = 4 * 1024;

/// Bit-width of the smallest class size.
const MIN_CLASS_BITS: u32 = 12;

/// Shared pool capacity per size class.
pub const POOL_CAP_PER_CLASS: usize = 64;

/// Per-thread stash capacity for the `vec_reuse` strategy.
pub const REUSE_STASH_CAP: usize = 8;

/// Total thread counts for the pipeline benchmark
/// (threads = producers + consumers, split evenly).
pub const PIPELINE_THREADS: &[usize] = &[2, 4, 8, 16];

/// Build a `ZeroPool` configured for workload benchmarks.
pub fn zeropool() -> ZeroPool {
    ZeroPool::new().min_buffer_size(0).max_buffers_per_class(POOL_CAP_PER_CLASS)
}

/// Route a requested length to the smallest power-of-two class that fits.
/// Saturates at the largest class.
pub fn class_index(len: usize) -> usize {
    let bits = len.max(MIN_CLASS_SIZE).next_power_of_two().trailing_zeros();
    let idx = (bits - MIN_CLASS_BITS).div_ceil(2) as usize;
    idx.min(NUM_CLASSES - 1)
}

/// Buffer size of a class index.
pub fn class_size(idx: usize) -> usize {
    MIN_CLASS_SIZE << (2 * idx)
}
