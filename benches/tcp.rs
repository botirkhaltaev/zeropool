#![allow(missing_docs)]

// TCP echo server benchmark: N concurrent clients send length-prefixed
// requests over loopback; a handler thread per connection reads the frame
// into a strategy buffer, checksums it, and replies with the CRC32.
// Buffers live for the duration of a socket read on the handler thread.
//
// The workload is generic over `BufStrategy`, so ZeroPool, raw `Vec`,
// per-thread `Vec` reuse, `bytes::BytesMut`, `opool`, and `object_pool` all
// run identical code. The client side is deliberately plain `Vec` — it is
// not under test.
//
// Run with full comparison crates:
//
// ```text
// cargo bench --bench tcp --features bench
// ```

mod common;

use std::hint::black_box;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::config::WORKLOAD_THREADS;
use common::mt::run_threads;
use common::strategy::{BufStrategy, VecFresh, VecReuse, ZeroPoolAlloc};

/// Requests each client sends per iteration.
const REQUESTS_PER_CLIENT: usize = 64;

fn handle_connection<S: BufStrategy>(mut stream: TcpStream, strategy: &S) {
    stream.set_nodelay(true).ok();
    loop {
        let mut header = [0u8; 4];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) => return,
        }
        let len = u32::from_le_bytes(header) as usize;
        let mut buf = strategy.get(len);
        if stream.read_exact(&mut buf[..]).is_err() {
            return;
        }
        let crc = crc32fast::hash(&buf[..]);
        if stream.write_all(&crc.to_le_bytes()).is_err() {
            return;
        }
    }
}

fn bench_strategy<S: BufStrategy>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    strategy: &S,
    size: usize,
) {
    for &threads in WORKLOAD_THREADS {
        group.throughput(Throughput::Bytes((threads * REQUESTS_PER_CLIENT * size) as u64));

        group.bench_with_input(BenchmarkId::new(S::name(), threads), &threads, |b, _| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
                    let addr = listener.local_addr().expect("addr");
                    let listener = &listener;

                    total += thread::scope(|s| {
                        // Accept `threads` connections, one scoped handler each.
                        s.spawn(move || {
                            for _ in 0..threads {
                                let (stream, _) = listener.accept().expect("accept");
                                s.spawn(move || handle_connection(stream, strategy));
                            }
                        });

                        // Connect clients before the timed region.
                        let clients: Vec<TcpStream> = (0..threads)
                            .map(|_| {
                                let stream = TcpStream::connect(addr).expect("connect");
                                stream.set_nodelay(true).ok();
                                stream
                            })
                            .collect();
                        let clients_ref = &clients;

                        let elapsed = run_threads(threads, |i| {
                            let mut stream = &clients_ref[i];
                            let mut payload = vec![i as u8; size];
                            for j in 0..REQUESTS_PER_CLIENT {
                                payload.fill((i as u8).wrapping_add(j as u8));
                                stream
                                    .write_all(&(size as u32).to_le_bytes())
                                    .expect("write header");
                                stream.write_all(&payload).expect("write payload");
                                let mut reply = [0u8; 4];
                                stream.read_exact(&mut reply).expect("read reply");
                                black_box(u32::from_le_bytes(reply));
                            }
                        });

                        // Clients finish; dropping their streams EOFs the handlers.
                        drop(clients);
                        elapsed
                    });
                }
                total
            });
        });
    }
}

fn tcp_echo(c: &mut Criterion) {
    for &(size, group_name) in &[(16 * 1024usize, "tcp_echo_16k"), (64 * 1024, "tcp_echo_64k")] {
        let mut group = c.benchmark_group(group_name);
        group.sample_size(20);

        let zeropool = ZeroPoolAlloc::new();
        zeropool.0.warm(8, size);
        bench_strategy(&mut group, &zeropool, size);

        bench_strategy(&mut group, &VecFresh, size);
        bench_strategy(&mut group, &VecReuse, size);

        #[cfg(feature = "bench")]
        {
            let bytes = common::strategy::BytesMutFresh;
            bench_strategy(&mut group, &bytes, size);

            let opool = common::strategy::OpoolClasses::new();
            bench_strategy(&mut group, &opool, size);

            let object_pool = common::strategy::ObjectPoolClasses::new();
            bench_strategy(&mut group, &object_pool, size);
        }

        group.finish();
    }
}

criterion_group!(benches, tcp_echo);
criterion_main!(benches);
