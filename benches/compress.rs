#![allow(missing_docs)]

// LZ4 compression benchmark: workers compress a text-ish block into one
// strategy buffer and decompress back into another, like a storage engine
// or log pipeline's write path. Two buffers are live per block (compressed
// output + decompressed output).
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench compress --features bench
// ```

mod common;

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::config::WORKLOAD_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Blocks each worker compresses+decompresses per iteration.
const BLOCKS_PER_WORKER: usize = 32;

/// Build a compressible block (~4x ratio) by repeating a JSON-like
/// sentence with a running counter.
fn make_input(len: usize) -> Vec<u8> {
    let mut input = String::with_capacity(len + 256);
    let mut i = 0u64;
    while input.len() < len {
        let _ = writeln!(
            input,
            "{{\"ts\":1700000000,\"level\":\"info\",\"service\":\"api\",\"req\":{i},\"latency_us\":{}}}",
            (i * 7919) % 50_000
        );
        i += 1;
    }
    input.truncate(len);
    input.into_bytes()
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
    len: usize,
) {
    let input = make_input(len);
    let input = &input;
    let max_out = lz4_flex::block::get_maximum_output_size(len);

    for &threads in WORKLOAD_THREADS {
        group.throughput(Throughput::Bytes((threads * BLOCKS_PER_WORKER * len) as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_threads(threads, |_i| {
                        for _ in 0..BLOCKS_PER_WORKER {
                            let mut out = strategy.get(max_out);
                            let n = lz4_flex::block::compress_into(input, &mut out)
                                .expect("compress_into fits max_out");
                            let mut dec = strategy.get(len);
                            let back = lz4_flex::block::decompress_into(&out[..n], &mut dec)
                                .expect("decompress round-trip");
                            debug_assert_eq!(back, len);
                            black_box(n);
                            black_box(&dec[..len.min(64)]);
                        }
                    });
                }
                total
            });
        });
    }
}

fn compress(c: &mut Criterion) {
    for &(len, group_name) in &[(256 * 1024usize, "lz4_256k"), (1024 * 1024, "lz4_1m")] {
        let mut group = c.benchmark_group(group_name);
        group.sample_size(20);

        let zeropool = ZeroPoolAlloc::new();
        zeropool.0.warm(8, lz4_flex::block::get_maximum_output_size(len));
        bench_strategy(&mut group, &zeropool, len);

        bench_strategy(&mut group, &VecFresh, len);
        bench_strategy(&mut group, &VecReuse, len);

        #[cfg(feature = "bench")]
        {
            let bytes = common::strategy::BytesMutFresh;
            bench_strategy(&mut group, &bytes, len);

            let opool = common::strategy::OpoolClasses::new();
            bench_strategy(&mut group, &opool, len);

            let object_pool = common::strategy::ObjectPoolClasses::new();
            bench_strategy(&mut group, &object_pool, len);
        }

        group.finish();
    }
}

criterion_group!(benches, compress);
criterion_main!(benches);
