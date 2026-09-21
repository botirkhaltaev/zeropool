#![allow(missing_docs)]

// RPC framing benchmark: workers encode and decode length-prefixed frames
// with a Zipf-distributed payload size (small frames dominate, large frames
// are rare) — the mixed-size pattern of a real RPC or message-broker hot
// path. Each frame is a `len`-prefixed copy of a source payload followed by
// a CRC32 decode check.
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench framing --features bench
// ```

mod common;

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::config::WORKLOAD_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Frames per worker per iteration.
const FRAMES_PER_WORKER: usize = 512;

/// Bucket sizes: powers of two from 1 KiB to 256 KiB.
const BUCKET_SIZES: [usize; 9] = [
    1024,
    2 * 1024,
    4 * 1024,
    8 * 1024,
    16 * 1024,
    32 * 1024,
    64 * 1024,
    128 * 1024,
    256 * 1024,
];

/// Zipf(1.0) cumulative weights over the 9 buckets: weight(k) = 1/k.
const CUM_WEIGHTS: [f64; 9] = {
    let mut cum = [0.0; 9];
    let mut acc = 0.0;
    let mut k = 0;
    while k < 9 {
        acc += 1.0 / (k + 1) as f64;
        cum[k] = acc;
        k += 1;
    }
    cum
};

/// Deterministic per-worker sample of frame sizes (LCG over the Zipf CDF).
fn frame_sizes(worker: usize) -> Vec<usize> {
    let mut sizes = Vec::with_capacity(FRAMES_PER_WORKER);
    let mut state = (worker as u64).wrapping_add(1);
    let total = CUM_WEIGHTS[8];
    for _ in 0..FRAMES_PER_WORKER {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let u = (state >> 11) as f64 / (1u64 << 53) as f64 * total;
        let bucket = CUM_WEIGHTS.iter().position(|&c| u < c).unwrap_or(8);
        sizes.push(BUCKET_SIZES[bucket]);
    }
    sizes
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
) {
    // Shared source payload: 256 KiB of deterministic bytes.
    let source: Vec<u8> = (0..256 * 1024)
        .map(|i| (i as u32).wrapping_mul(2_654_435_761).to_le_bytes()[0])
        .collect();
    let source = &source;

    for &threads in WORKLOAD_THREADS {
        // Pre-generate per-worker size streams and sum them for throughput.
        let per_worker: Vec<Vec<usize>> = (0..threads).map(frame_sizes).collect();
        let total_bytes: usize =
            per_worker.iter().flat_map(|sizes| sizes.iter().map(|s| s + 4)).sum();
        let per_worker = &per_worker;
        group.throughput(Throughput::Bytes(total_bytes as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_threads(threads, |i| {
                        for &size in &per_worker[i] {
                            let mut buf = strategy.get(size + 4);
                            // Encode: LE u32 length header + payload copy.
                            buf[..4].copy_from_slice(&(size as u32).to_le_bytes());
                            buf[4..4 + size].copy_from_slice(&source[..size]);
                            // Decode: parse header, verify length, checksum payload.
                            let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
                            assert_eq!(len, size);
                            let crc = crc32fast::hash(&buf[4..4 + len]);
                            black_box(crc);
                        }
                    });
                }
                total
            });
        });
    }
}

fn framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("framing_zipf");
    group.sample_size(20);

    let zeropool = ZeroPoolAlloc::new();
    zeropool.0.warm(8, 256 * 1024 + 4);
    bench_strategy(&mut group, &zeropool);

    bench_strategy(&mut group, &VecFresh);
    bench_strategy(&mut group, &VecReuse);

    #[cfg(feature = "bench")]
    {
        let bytes = common::strategy::BytesMutFresh;
        bench_strategy(&mut group, &bytes);

        let opool = common::strategy::OpoolClasses::new();
        bench_strategy(&mut group, &opool);

        let object_pool = common::strategy::ObjectPoolClasses::new();
        bench_strategy(&mut group, &object_pool);
    }

    group.finish();
}

criterion_group!(benches, framing);
criterion_main!(benches);
