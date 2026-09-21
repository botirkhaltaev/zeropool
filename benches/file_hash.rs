#![allow(missing_docs)]

// File-hashing benchmark: N workers stream files off the page cache in
// fixed-size chunks and CRC32 each chunk, like `sha256sum` or a backup
// scanner. Buffers are held for the duration of a `read` syscall.
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench file_hash --features bench
// ```

mod common;

use std::fs::{self, File};
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::config::WORKLOAD_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Number of files hashed per iteration.
const FILE_COUNT: usize = 8;

/// Size of each file.
const FILE_BYTES: usize = 8 * 1024 * 1024;

/// Deterministic pseudo-random fill (xorshift64*) so the page cache holds
/// realistic file contents instead of zero pages.
fn write_file(path: &Path, seed: u64) {
    let mut file = File::create(path).expect("create bench file");
    let mut state = seed | 1;
    let mut chunk = Vec::with_capacity(64 * 1024);
    while chunk.len() < FILE_BYTES {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        chunk.extend_from_slice(&state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_ne_bytes());
    }
    chunk.truncate(FILE_BYTES);
    file.write_all(&chunk).expect("write bench file");
}

/// Create the bench directory with `FILE_COUNT` files and warm the page
/// cache by reading each one back.
fn setup_files() -> (PathBuf, Vec<PathBuf>) {
    let dir = std::env::temp_dir().join(format!("zeropool-bench-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create bench dir");
    let files: Vec<PathBuf> = (0..FILE_COUNT)
        .map(|i| {
            let path = dir.join(format!("data-{i}.bin"));
            write_file(&path, i as u64 + 1);
            path
        })
        .collect();
    // Warm the page cache once, outside the timed region.
    let mut sink = vec![0u8; 64 * 1024];
    for path in &files {
        let mut file = File::open(path).expect("open for warm");
        while file.read(&mut sink).expect("warm read") > 0 {}
    }
    (dir, files)
}

fn hash_file<S: BufStrategy>(path: &Path, strategy: &S, chunk: usize) {
    let mut file = File::open(path).expect("open bench file");
    loop {
        let mut buf = strategy.get(chunk);
        let mut filled = 0;
        while filled < chunk {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read failed: {e}"),
            }
        }
        if filled == 0 {
            break;
        }
        black_box(crc32fast::hash(&buf[..filled]));
    }
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
    files: &[PathBuf],
    chunk: usize,
) {
    for &threads in WORKLOAD_THREADS {
        // Total bytes hashed is constant: workers partition the same 8 files.
        group.throughput(Throughput::Bytes((FILE_COUNT * FILE_BYTES) as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    total += run_threads(threads, |i| {
                        for (idx, path) in files.iter().enumerate() {
                            if idx % threads == i {
                                hash_file(path, strategy, chunk);
                            }
                        }
                    });
                }
                total
            });
        });
    }
}

fn file_hash(c: &mut Criterion) {
    for &(chunk, group_name) in
        &[(256 * 1024usize, "file_hash_256k"), (4 * 1024 * 1024, "file_hash_4m")]
    {
        let mut group = c.benchmark_group(group_name);
        group.sample_size(20);

        let (dir, files) = setup_files();

        let zeropool = ZeroPoolAlloc::new();
        zeropool.0.warm(8, chunk);
        bench_strategy(&mut group, &zeropool, &files, chunk);

        bench_strategy(&mut group, &VecFresh, &files, chunk);
        bench_strategy(&mut group, &VecReuse, &files, chunk);

        #[cfg(feature = "bench")]
        {
            let bytes = common::strategy::BytesMutFresh;
            bench_strategy(&mut group, &bytes, &files, chunk);

            let opool = common::strategy::OpoolClasses::new();
            bench_strategy(&mut group, &opool, &files, chunk);

            let object_pool = common::strategy::ObjectPoolClasses::new();
            bench_strategy(&mut group, &object_pool, &files, chunk);
        }

        group.finish();
        fs::remove_dir_all(&dir).expect("cleanup bench dir");
    }
}

criterion_group!(benches, file_hash);
criterion_main!(benches);
