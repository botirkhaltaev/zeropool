#![allow(missing_docs)]

// Log-shipper pipeline benchmark: P producers serialize newline-delimited
// JSON log records into fixed-size batches and hand them to P consumers over
// a bounded channel. Consumers checksum the payload and drop the buffer, so
// every buffer is freed on a different thread than the one that filled it.
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench pipeline --features bench
// ```

mod common;

use std::hint::black_box;
use std::io::Write as _;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde::Serialize;

use common::config::PIPELINE_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Batches each producer sends per iteration.
const BATCHES_PER_PRODUCER: usize = 64;

/// Upper bound on one serialized record (incl. newline); used to decide
/// whether the next record still fits in the batch buffer.
const REC_WORST: usize = 512;

#[derive(Serialize)]
struct LogRecord<'a> {
    ts: u64,
    level: &'a str,
    service: &'a str,
    request_id: u64,
    latency_us: u32,
    msg: &'a str,
}

fn fill_batch(buf: &mut [u8], producer: usize, seq: usize) -> usize {
    let mut cursor = std::io::Cursor::new(buf);
    let mut request_id = (producer * BATCHES_PER_PRODUCER + seq) as u64 * 1000;

    while (cursor.position() as usize) + REC_WORST <= cursor.get_ref().len() {
        let rec = LogRecord {
            ts: 1_700_000_000_000 + request_id,
            level: "info",
            service: "api-gateway",
            request_id,
            latency_us: (request_id % 50_000) as u32,
            msg: "GET /v1/orders?status=open completed",
        };
        serde_json::to_writer(&mut cursor, &rec).expect("record fits");
        cursor.write_all(b"\n").expect("newline fits");
        request_id += 1;
    }

    cursor.position() as usize
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
    batch_bytes: usize,
) {
    for &threads in PIPELINE_THREADS {
        let p = threads / 2;
        group.throughput(Throughput::Bytes((p * BATCHES_PER_PRODUCER * batch_bytes) as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let (tx, rx) = crossbeam_channel::bounded::<Option<(S::Buf<'_>, usize)>>(2 * p);
                    total += run_threads(threads, |i| {
                        if i < p {
                            // Producer
                            for batch in 0..BATCHES_PER_PRODUCER {
                                let mut buf = strategy.get(batch_bytes);
                                let filled = fill_batch(&mut buf, i, batch);
                                black_box(filled);
                                tx.send(Some((buf, filled))).expect("consumer alive");
                            }
                            tx.send(None).expect("consumer alive");
                        } else {
                            // Consumer
                            while let Ok(Some((buf, filled))) = rx.recv() {
                                let crc = crc32fast::hash(&buf[..filled]);
                                #[allow(clippy::naive_bytecount)]
                                let lines = buf[..filled].iter().filter(|&&b| b == b'\n').count();
                                black_box((crc, lines));
                            }
                        }
                    });
                }
                total
            });
        });
    }
}

fn pipeline(c: &mut Criterion) {
    for &(batch_bytes, group_name) in
        &[(16 * 1024usize, "pipeline_16k"), (256 * 1024, "pipeline_256k")]
    {
        let mut group = c.benchmark_group(group_name);
        group.sample_size(20);

        let zeropool = ZeroPoolAlloc::new();
        zeropool.0.warm(PIPELINE_THREADS[PIPELINE_THREADS.len() - 1], batch_bytes);
        bench_strategy(&mut group, &zeropool, batch_bytes);

        bench_strategy(&mut group, &VecFresh, batch_bytes);
        bench_strategy(&mut group, &VecReuse, batch_bytes);

        #[cfg(feature = "bench")]
        {
            let bytes = common::strategy::BytesMutFresh;
            bench_strategy(&mut group, &bytes, batch_bytes);

            let opool = common::strategy::OpoolClasses::new();
            bench_strategy(&mut group, &opool, batch_bytes);

            let object_pool = common::strategy::ObjectPoolClasses::new();
            bench_strategy(&mut group, &object_pool, batch_bytes);
        }

        group.finish();
    }
}

criterion_group!(benches, pipeline);
criterion_main!(benches);
