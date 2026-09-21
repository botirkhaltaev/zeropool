#![allow(missing_docs)]

// Image tile benchmark: workers run a horizontal 3-tap box blur over
// 512x512 RGBA tiles, writing the result into a strategy buffer, like a
// thumbnailer or codec pipeline stage. One full-frame buffer per tile.
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench tiles --features bench
// ```

mod common;

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::config::WORKLOAD_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Tile edge length in pixels.
const TILE_DIM: usize = 512;

/// Bytes per tile: 512 x 512 RGBA = 1 MiB.
const TILE_BYTES: usize = TILE_DIM * TILE_DIM * 4;

/// Tiles each worker processes per iteration.
const TILES_PER_WORKER: usize = 16;

/// Deterministic gradient input tile.
fn make_input() -> Vec<u8> {
    (0..TILE_DIM * TILE_DIM)
        .flat_map(|p| {
            let (x, y) = (p % TILE_DIM, p / TILE_DIM);
            [x as u8, y as u8, (x + y) as u8 / 2, 255]
        })
        .collect()
}

/// Horizontal 3-tap box blur per channel; edge pixels copied verbatim.
fn blur(input: &[u8], out: &mut [u8]) {
    for y in 0..TILE_DIM {
        let row = y * TILE_DIM * 4;
        for x in 0..TILE_DIM {
            let px = row + x * 4;
            if x == 0 || x == TILE_DIM - 1 {
                out[px..px + 4].copy_from_slice(&input[px..px + 4]);
                continue;
            }
            for c in 0..4 {
                let sum = u32::from(input[px - 4 + c])
                    + u32::from(input[px + c])
                    + u32::from(input[px + 4 + c]);
                out[px + c] = (sum / 3) as u8;
            }
        }
    }
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
) {
    let input = make_input();
    let input = &input;

    for &threads in WORKLOAD_THREADS {
        group.throughput(Throughput::Bytes((threads * TILES_PER_WORKER * TILE_BYTES) as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_threads(threads, |_i| {
                        for _ in 0..TILES_PER_WORKER {
                            let mut out = strategy.get(TILE_BYTES);
                            blur(input, &mut out);
                            black_box(out[0]);
                            black_box(crc32fast::hash(&out[..TILE_DIM * 4]));
                        }
                    });
                }
                total
            });
        });
    }
}

fn tiles(c: &mut Criterion) {
    let mut group = c.benchmark_group("tiles_rgba");
    group.sample_size(20);

    let zeropool = ZeroPoolAlloc::new();
    zeropool.0.warm(8, TILE_BYTES);
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

criterion_group!(benches, tiles);
criterion_main!(benches);
